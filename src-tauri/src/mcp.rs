//! MCP server cho Claude Code: `plancouncil --mcp` (stdio, JSON-RPC 2.0, mỗi message 1 dòng).
//!
//! Chạy cuộc họp ngay trong tiến trình này (không cần mở app desktop), dùng chung config, API key
//! (chỉ đọc) và thư mục lịch sử với app → phiên vẫn hiện trong tab Lịch sử. Xem `docs/MCP.md`.
//!
//! - stdout CHỈ chứa JSON-RPC; log ra stderr.
//! - 1 phiên chạy tại 1 thời điểm trong mỗi tiến trình MCP (như app desktop).
//! - Workspace: chỉ những workspace đã khai báo trong Cài đặt của app (đọc qua `WorkspaceFs`, chặn traversal).
//! - Trong lúc chạy, phiên giữ "lease" (`Storage::touch_lease`) để app desktop không đánh dấu nhầm là gián đoạn.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{self, AppConfig, PlanDepth, SessionOptions};
use crate::history::{HistoryIndex, SessionWriter};
use crate::llm_client::{self, LlmClient};
use crate::models::{
    CouncilEvent, EventEnvelope, RoundRecord, SessionRecord, SessionStatus, Verdict, MAIN_SCOPE,
};
use crate::orchestrator::{Council, EventSink, MeetingControl};
use crate::secrets::{self, Secrets};
use crate::storage::{Storage, LEASE_REFRESH};
use crate::{build_extras, check_secrets_present, CheckpointSink, CommandError, MAX_PROMPT_CHARS};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Bối cảnh Claude Code gửi kèm (ghép vào yêu cầu; tổng vẫn ≤ `MAX_PROMPT_CHARS`).
const MAX_CONTEXT_CHARS: usize = 30_000;
/// Số cập nhật giữ trong bộ nhớ cho 1 phiên (cũ hơn bị bỏ, `get_council_updates` báo `missed`).
const MAX_UPDATES_KEPT: usize = 1_000;
const DEFAULT_UPDATES_LIMIT: usize = 50;
const MAX_UPDATES_LIMIT: usize = 200;
/// Long-poll tối đa (giữ dưới timeout gọi tool thường gặp của client).
const MAX_WAIT_SECS: u64 = 55;
const DEFAULT_PLAN_CHARS: usize = 60_000;
const MAX_PLAN_CHARS: usize = 200_000;
/// Giới hạn từng chuỗi trong cập nhật / kết quả (tóm tắt review, issue…).
const MAX_ITEM_CHARS: usize = 600;
/// Chờ phiên đang chạy dừng hẳn + lưu bản ghi cuối khi Claude Code đóng kết nối.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(15);

// ───────────────────────────── Entry ─────────────────────────────

pub fn serve() -> i32 {
    init_logger();
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("plancouncil --mcp: không tạo được tokio runtime: {e}");
            return 1;
        }
    };
    rt.block_on(async {
        match Server::new() {
            Ok(server) => {
                Arc::new(server).run_stdio().await;
                0
            }
            Err(e) => {
                eprintln!("plancouncil --mcp: {e}");
                1
            }
        }
    })
}

/// Log ra stderr (stdout dành riêng cho JSON-RPC). Crate này: Info; thư viện khác: Warn.
fn init_logger() {
    struct StderrLog;
    impl log::Log for StderrLog {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            let max = if m.target().starts_with("plancouncil") { log::Level::Info } else { log::Level::Warn };
            m.level() <= max
        }
        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                eprintln!("[{}] {}: {}", r.level(), r.target(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: StderrLog = StderrLog;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

// ───────────────────────────── Feed: cập nhật của 1 phiên ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct Update {
    seq: u64,
    ts: DateTime<Utc>,
    kind: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct RoundBrief {
    round: u32,
    plan_version: u32,
    approvals: u32,
    total: u32,
    converged: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Progress {
    /// "starting" | "departments" | "drafting" | "reviewing" | "revising" | "finished"
    phase: &'static str,
    round: u32,
    max_rounds: u32,
    plan_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_round: Option<RoundBrief>,
    departments_total: u32,
    departments_done: u32,
    tokens: u64,
    /// Chi phí cộng dồn theo bảng giá lúc bắt đầu phiên (None = có model chưa có giá).
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_state: Option<String>,
}

struct FeedState {
    updates: VecDeque<Update>,
    next_seq: u64,
    /// Con trỏ mặc định của `get_council_updates` khi không truyền `since`.
    cursor: u64,
    progress: Progress,
    names: HashMap<String, String>,
    /// Bản ghi cuối (đã lưu) — có khi phiên kết thúc.
    record: Option<Arc<SessionRecord>>,
}

struct Feed {
    state: Mutex<FeedState>,
    /// Tăng mỗi khi có cập nhật / kết thúc → đánh thức long-poll.
    tick: watch::Sender<u64>,
}

impl Feed {
    fn new(max_rounds: u32) -> Self {
        Self {
            state: Mutex::new(FeedState {
                updates: VecDeque::new(),
                next_seq: 0,
                cursor: 0,
                progress: Progress {
                    phase: "starting",
                    round: 0,
                    max_rounds,
                    plan_version: 0,
                    last_round: None,
                    departments_total: 0,
                    departments_done: 0,
                    tokens: 0,
                    cost_usd: None,
                    budget_state: None,
                },
                names: HashMap::new(),
                record: None,
            }),
            tick: watch::channel(0).0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FeedState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(&self) {
        self.tick.send_modify(|v| *v += 1);
    }

    fn push(&self, kind: &'static str, text: String, details: Option<Value>) {
        {
            let mut st = self.lock();
            push_locked(&mut st, kind, text, details);
        }
        self.bump();
    }

    fn finish(&self, record: SessionRecord) {
        {
            let mut st = self.lock();
            st.progress.phase = "finished";
            st.progress.tokens = record.usage.total.total();
            st.record = Some(Arc::new(record));
        }
        self.bump();
    }

    fn finished(&self) -> bool {
        self.lock().record.is_some()
    }

    /// Chờ tới khi `done(feed)` đúng hoặc hết `wait` (thức dậy mỗi khi có cập nhật).
    async fn wait_until(&self, wait: Duration, done: impl Fn(&Feed) -> bool) {
        if wait.is_zero() || done(self) {
            return;
        }
        let mut rx = self.tick.subscribe();
        let _ = tokio::time::timeout(wait, async {
            while !done(self) {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
    }

    fn on_event(&self, ev: &CouncilEvent) {
        let mut st = self.lock();
        let name = |st: &FeedState, id: &str| st.names.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let place = |scope: &str| if scope == MAIN_SCOPE { String::new() } else { format!(" [{scope}]") };
        match ev {
            CouncilEvent::SessionStarted { chair_id, reviewer_ids, cast, departments, max_rounds, .. } => {
                for c in cast {
                    st.names.insert(c.id.clone(), c.name.clone());
                }
                st.progress.max_rounds = *max_rounds;
                st.progress.departments_total = departments.len() as u32;
                st.progress.phase = if departments.is_empty() { "drafting" } else { "departments" };
                let reviewers: Vec<String> = reviewer_ids.iter().map(|id| name(&st, id)).collect();
                let text = format!(
                    "Bắt đầu họp: chủ trì {}, {} reviewer ({}){}",
                    name(&st, chair_id),
                    reviewers.len(),
                    reviewers.join(", "),
                    if departments.is_empty() {
                        String::new()
                    } else {
                        format!(", {} phòng ban họp trước", departments.len())
                    }
                );
                push_locked(&mut st, "started", text, None);
            }
            CouncilEvent::StaffingDecided { staffing } => {
                let text = format!(
                    "AI chọn thành phần: chủ trì {}, reviewer: {}{}",
                    name(&st, &staffing.chair_id),
                    staffing.reviewer_ids.iter().map(|id| name(&st, id)).collect::<Vec<_>>().join(", "),
                    if staffing.fallback { " (dự phòng: dùng cấu hình mặc định)" } else { "" }
                );
                push_locked(&mut st, "staffing", text, None);
            }
            CouncilEvent::DepartmentsAssigned { assignments } => {
                let details = json!(assignments
                    .iter()
                    .map(|a| json!({ "department": a.department_id, "task": clip(&a.task, MAX_ITEM_CHARS) }))
                    .collect::<Vec<_>>());
                push_locked(&mut st, "departments_assigned", format!("Chủ trì giao việc cho {} phòng ban", assignments.len()), Some(details));
            }
            CouncilEvent::MembershipChanged { change, member } => {
                if let Some(m) = member {
                    st.names.insert(m.id.clone(), m.name.clone());
                }
                let who = name(&st, &change.agent);
                let what = match change.action.as_str() {
                    "invited" => "được mời vào họp",
                    "dismissed" => "được mời ra khỏi cuộc họp",
                    _ => "tự rút khỏi cuộc họp",
                };
                let text = format!("{who} {what}{} — {}", place(&change.scope), clip(&change.reason, MAX_ITEM_CHARS));
                push_locked(&mut st, "membership", text, None);
            }
            CouncilEvent::ToolUsed { activity } => {
                let text = format!("{} dùng {}{}: {}", name(&st, &activity.agent), activity.tool, place(&activity.scope), clip(&activity.summary, 200));
                push_locked(&mut st, "tool", text, None);
            }
            CouncilEvent::PlanReady { scope, version, markdown } => {
                if scope == MAIN_SCOPE {
                    st.progress.plan_version = *version;
                    st.progress.phase = "reviewing";
                    st.progress.round = st.progress.last_round.as_ref().map_or(1, |r| r.round + 1).min(st.progress.max_rounds.max(1));
                }
                let text = format!("Plan v{version}{} sẵn sàng ({} ký tự)", place(scope), markdown.chars().count());
                push_locked(&mut st, "plan_ready", text, None);
            }
            CouncilEvent::ReviewReady { scope, round, review } => {
                let verdict = verdict_str(review.verdict);
                let text = format!(
                    "R{round}{} {}: {verdict}{} — {}",
                    place(scope),
                    review.role,
                    if review.carried_over { " (giữ nguyên)" } else { "" },
                    clip(&review.summary_bubble, MAX_ITEM_CHARS)
                );
                let details = (!review.critical_issues.is_empty()).then(|| {
                    json!({ "critical_issues": review.critical_issues.iter().take(10).map(|s| clip(s, MAX_ITEM_CHARS)).collect::<Vec<_>>() })
                });
                push_locked(&mut st, "review", text, details);
            }
            CouncilEvent::ReviewFailed { scope, round, agent, error } => {
                let text = format!("R{round}{} {} không review được: {}", place(scope), name(&st, agent), clip(error, 300));
                push_locked(&mut st, "review_failed", text, None);
            }
            CouncilEvent::RoundCompleted { scope, round, plan_version, approvals, total, converged } => {
                if scope == MAIN_SCOPE {
                    st.progress.round = *round;
                    st.progress.last_round = Some(RoundBrief {
                        round: *round,
                        plan_version: *plan_version,
                        approvals: *approvals,
                        total: *total,
                        converged: *converged,
                    });
                    if !converged {
                        st.progress.phase = "revising";
                    }
                }
                let text = format!(
                    "Round {round}{} (plan v{plan_version}): {approvals}/{total} APPROVE{}",
                    place(scope),
                    if *converged { " — hội tụ" } else { "" }
                );
                push_locked(&mut st, "round", text, None);
            }
            CouncilEvent::Truncated { scope, agent, call_kind, recovered: false, reason } => {
                let text = format!("Output {call_kind} của {}{} bị cắt ({reason})", name(&st, agent), place(scope));
                push_locked(&mut st, "truncated", text, None);
            }
            CouncilEvent::Consistency { scope, result, conflicts } if result != "ok" => {
                let text = format!("Kiểm tra nhất quán{}: {result} ({} mâu thuẫn)", place(scope), conflicts.len());
                push_locked(&mut st, "consistency", text, None);
            }
            CouncilEvent::Budget { state, used, limit, .. } => {
                if st.progress.budget_state.as_deref() != Some(state.as_str()) {
                    st.progress.budget_state = Some(state.clone());
                    if state != "ok" {
                        push_locked(&mut st, "budget", format!("Ngân sách token: {state} ({used}/{limit})"), None);
                    }
                }
            }
            CouncilEvent::UsageUpdated { total, cost_usd, .. } => {
                st.progress.tokens = total.total();
                st.progress.cost_usd = *cost_usd;
            }
            CouncilEvent::DepartmentFinished { department_id, status, error, .. } => {
                st.progress.departments_done += 1;
                if st.progress.departments_done >= st.progress.departments_total {
                    st.progress.phase = "drafting";
                }
                let text = format!(
                    "Phòng ban {department_id} xong: {}{}",
                    status_str(*status),
                    error.as_deref().map(|e| format!(" — {}", clip(e, 300))).unwrap_or_default()
                );
                push_locked(&mut st, "department_finished", text, None);
            }
            CouncilEvent::Finished { status, final_version, .. } => {
                let text = format!("Kết thúc: {} — Final là plan v{final_version}", status_str(*status));
                push_locked(&mut st, "finished", text, None);
            }
            CouncilEvent::Cancelled { round } => push_locked(&mut st, "cancelled", format!("Phiên đã dừng ở round {round}"), None),
            CouncilEvent::Failed { error } => push_locked(&mut st, "failed", format!("Phiên lỗi: {}", clip(error, 600)), None),
            // Delta stream, nghĩ, tiết kiệm token…: không đáng làm 1 cập nhật.
            _ => return,
        }
        drop(st);
        self.bump();
    }
}

fn push_locked(st: &mut FeedState, kind: &'static str, text: String, details: Option<Value>) {
    let seq = st.next_seq;
    st.next_seq += 1;
    st.updates.push_back(Update { seq, ts: Utc::now(), kind, text, details });
    while st.updates.len() > MAX_UPDATES_KEPT {
        st.updates.pop_front();
    }
}

struct FeedSink(Arc<Feed>);

impl EventSink for FeedSink {
    fn emit(&self, envelope: &EventEnvelope) {
        self.0.on_event(&envelope.event);
    }
}

// ───────────────────────────── Server ─────────────────────────────

struct Live {
    id: Uuid,
    cancel: CancellationToken,
    control: Arc<MeetingControl>,
}

struct Server {
    storage: Storage,
    history: HistoryIndex,
    secrets: Arc<Secrets>,
    http: reqwest::Client,
    active: Mutex<Option<Live>>,
    feeds: Mutex<HashMap<Uuid, Arc<Feed>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

/// Lỗi của 1 tool → kết quả `isError: true` (model đọc được và tự sửa đầu vào).
struct ToolError(String);

impl From<CommandError> for ToolError {
    fn from(e: CommandError) -> Self {
        let kind = serde_json::to_value(&e).ok().and_then(|v| v["kind"].as_str().map(str::to_owned)).unwrap_or_default();
        ToolError(format!("[{kind}] {e}"))
    }
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError(s)
    }
}

impl From<&str> for ToolError {
    fn from(s: &str) -> Self {
        ToolError(s.to_owned())
    }
}

type ToolResult = Result<Vec<Value>, ToolError>;

struct RpcError {
    code: i64,
    message: String,
}

/// RAII: luôn trả slot phiên kể cả khi task họp panic.
struct ActiveGuard(Arc<Server>, Uuid);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut a = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        if a.as_ref().is_some_and(|l| l.id == self.1) {
            *a = None;
        }
    }
}

impl Server {
    fn new() -> Result<Self, String> {
        let storage = Storage::new().map_err(|e| format!("không mở được thư mục dữ liệu: {e}"))?;
        log::info!("PlanCouncil MCP: data dir {}", storage.root().display());
        let history = HistoryIndex::open(&storage);
        let secrets = Arc::new(Secrets::new(Arc::new(secrets::KeyringStore), storage.root()));
        let http = llm_client::build_http_client().map_err(|e| format!("không tạo được HTTP client: {e}"))?;
        Ok(Self {
            storage,
            history,
            secrets,
            http,
            active: Mutex::new(None),
            feeds: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Vec::new()),
        })
    }

    async fn run_stdio(self: Arc<Self>) {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let writer = tokio::spawn(async move {
            let mut out = tokio::io::stdout();
            while let Some(line) = rx.recv().await {
                if out.write_all(line.as_bytes()).await.is_err() || out.write_all(b"\n").await.is_err() || out.flush().await.is_err() {
                    break;
                }
            }
        });
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (server, tx) = (self.clone(), tx.clone());
                    // Mỗi request 1 task: long-poll không chặn request khác.
                    tokio::spawn(async move {
                        if let Some(resp) = server.handle_line(&line).await {
                            let _ = tx.send(resp);
                        }
                    });
                }
                Ok(None) => break,
                Err(e) => {
                    log::error!("đọc stdin lỗi: {e}");
                    break;
                }
            }
        }
        log::info!("stdin đóng → dừng MCP server");
        self.shutdown().await;
        drop(tx);
        let _ = writer.await;
    }

    /// Claude Code đóng kết nối: dừng phiên đang chạy, chờ lưu bản ghi cuối (tối đa `SHUTDOWN_GRACE`).
    async fn shutdown(&self) {
        if let Some(l) = self.active.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            log::warn!("dừng phiên {} vì MCP server tắt", l.id);
            l.cancel.cancel();
        }
        let tasks: Vec<_> = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()));
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, futures_util::future::join_all(tasks)).await;
    }

    async fn handle_line(self: &Arc<Self>, line: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let Some(obj) = msg.as_object() else {
            return Some(rpc_error(Value::Null, -32600, "invalid request: cần 1 JSON object (không hỗ trợ batch)"));
        };
        // Không có id = notification (vd. notifications/initialized) hoặc response của client → bỏ qua.
        let id = obj.get("id").cloned().filter(|v| !v.is_null())?;
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return Some(rpc_error(id, -32600, "invalid request: thiếu method"));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let out = match self.dispatch(method, params).await {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(e) => return Some(rpc_error(id, e.code, &e.message)),
        };
        Some(out.to_string())
    }

    async fn dispatch(self: &Arc<Self>, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "plancouncil", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_defs() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let res = match name.as_str() {
                    "start_council" => self.start_council(args).await,
                    "get_council_status" => self.get_status(args).await,
                    "get_council_updates" => self.get_updates(args).await,
                    "get_council_result" => self.get_result(args).await,
                    "cancel_council" => self.cancel_council(args).await,
                    "send_council_message" => self.send_message(args),
                    "list_councils" => self.list_councils(args).await,
                    _ => return Err(RpcError { code: -32602, message: format!("unknown tool: {name}") }),
                };
                Ok(match res {
                    Ok(content) => json!({ "content": content, "isError": false }),
                    Err(ToolError(msg)) => json!({ "content": [text_block(msg)], "isError": true }),
                })
            }
            _ => Err(RpcError { code: -32601, message: format!("method not found: {method}") }),
        }
    }

    fn feed(&self, id: Uuid) -> Option<Arc<Feed>> {
        self.feeds.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()
    }

    fn active_id(&self) -> Option<Uuid> {
        self.active.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|l| l.id)
    }

    /// Config mới nhất trên đĩa + key (chỉ đọc keyring / secrets.json, không ghi gì).
    async fn load_config(&self) -> Result<AppConfig, ToolError> {
        let (storage, secrets) = (self.storage.clone(), self.secrets.clone());
        tokio::task::spawn_blocking(move || {
            let mut cfg = storage.load_config();
            secrets.hydrate_readonly(&mut cfg);
            cfg
        })
        .await
        .map_err(|e| ToolError(format!("lỗi nội bộ: {e}")))
    }

    async fn load_record(&self, id: Uuid) -> Result<SessionRecord, ToolError> {
        let storage = self.storage.clone();
        match tokio::task::spawn_blocking(move || storage.load_session(id)).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(crate::storage::StorageError::NotFound(_))) => Err(ToolError(format!("[not_found] không tìm thấy phiên {id}"))),
            Ok(Err(e)) => Err(ToolError(format!("[storage] {e}"))),
            Err(e) => Err(ToolError(format!("lỗi nội bộ: {e}"))),
        }
    }

    // ── start_council ──

    async fn start_council(self: &Arc<Self>, args: Value) -> ToolResult {
        let a: StartArgs = parse_args(args)?;
        if let Some(id) = self.active_id() {
            return Err(ToolError(format!(
                "[already_running] MCP server đang chạy phiên {id}. Chờ phiên đó xong (get_council_result với wait_seconds) hoặc cancel_council trước."
            )));
        }
        let prompt = compose_prompt(&a.prompt, a.context.as_deref())?;

        let mut cfg = self.load_config().await?;
        if let Some(n) = a.max_rounds {
            if !(1..=10).contains(&n) {
                return Err("max_rounds phải trong khoảng 1..=10".into());
            }
            cfg.max_rounds = n;
        }
        if let Some(mut ids) = a.departments {
            let mut seen = std::collections::HashSet::new();
            ids.retain(|id| seen.insert(id.clone()));
            if let Some(bad) = ids.iter().find(|id| !cfg.departments.iter().any(|d| &d.id == *id)) {
                let known: Vec<&str> = cfg.departments.iter().map(|d| d.id.as_str()).collect();
                return Err(ToolError(format!("phòng ban '{bad}' không có trong cấu hình. Có: {}", known.join(", "))));
            }
            cfg.meeting.department_ids = ids;
        }
        let workspace_id = match a.workspace.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(w) => Some(resolve_workspace(&cfg, w)?),
            None => None,
        };
        let opts = SessionOptions {
            workspace_id,
            web_search: a.web_search.unwrap_or(cfg.web_search.enabled_by_default),
            company_context: a.company_context.unwrap_or(cfg.company_context.enabled_by_default),
            chair_id: a.chair_id.filter(|s| !s.trim().is_empty()),
            reviewer_ids: a.reviewer_ids,
            auto_staff: a.auto_staff.unwrap_or(cfg.meeting.staffing == config::StaffingMode::Auto),
            plan_depth: a.plan_depth,
        };
        if let Some(chair) = &opts.chair_id {
            if cfg.role(chair).is_none() {
                return Err(ToolError(format!("role chủ trì '{chair}' không tồn tại")));
            }
            cfg.meeting.chair_id = chair.clone();
        }
        if let Some(ids) = &opts.reviewer_ids {
            let mut seen = std::collections::HashSet::new();
            cfg.meeting.reviewer_ids = ids.iter().filter(|id| seen.insert(id.as_str())).cloned().collect();
        }
        if let Some(depth) = opts.plan_depth {
            cfg.meeting.plan_depth = depth;
        }

        cfg.validate_ready().map_err(|e| ToolError(format!("[invalid] {e}")))?;
        check_secrets_present(&self.secrets, &cfg, opts.web_search)?;
        let mut extras = build_extras(&cfg, &opts).await?;
        extras.auto_staff = opts.auto_staff;
        extras.info.origin = Some("mcp".into());
        let control = extras.control.clone();

        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        {
            let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(l) = active.as_ref() {
                return Err(ToolError(format!("[already_running] MCP server đang chạy phiên {}", l.id)));
            }
            *active = Some(Live { id, cancel: cancel.clone(), control });
        }
        let guard = ActiveGuard(self.clone(), id);
        // Lease trước bản ghi đầu tiên → app desktop mở lúc này không đánh dấu phiên là gián đoạn.
        if let Err(e) = self.storage.touch_lease(id) {
            log::warn!("không tạo được lease cho phiên {id}: {e}");
        }

        let feed = Arc::new(Feed::new(cfg.max_rounds));
        self.feeds.lock().unwrap_or_else(|e| e.into_inner()).insert(id, feed.clone());
        let writer = Arc::new(SessionWriter::new(self.storage.clone(), self.history.clone()));
        extras.checkpoint = Some(Arc::new(CheckpointSink { writer: writer.clone(), seq: Default::default() }));

        let summary = json!({
            "session_id": id,
            "state": "running",
            "chair": if opts.auto_staff { "auto".to_owned() } else { role_name(&cfg, &cfg.meeting.chair_id) },
            "reviewers": if opts.auto_staff { json!("auto") } else { json!(cfg.meeting.reviewer_ids.iter().map(|r| role_name(&cfg, r)).collect::<Vec<_>>()) },
            "departments": cfg.meeting.department_ids,
            "max_rounds": cfg.max_rounds,
            "workspace": extras.info.workspace,
            "web_search": extras.info.web_search,
            "company_context": extras.info.company_context,
            "next": "Gọi get_council_updates (wait_seconds ~30) để theo dõi, hoặc get_council_result với wait_seconds để chờ kết quả cuối.",
        });

        let llm = LlmClient::new(self.http.clone(), &cfg);
        let council = Council::with_extras(llm, cfg, Arc::new(FeedSink(feed.clone())), id, cancel, extras);
        let storage = self.storage.clone();
        let task = tokio::spawn(async move {
            let _guard = guard;
            let lease_stop = CancellationToken::new();
            let lease = {
                let (storage, stop) = (storage.clone(), lease_stop.clone());
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = stop.cancelled() => break,
                            _ = tokio::time::sleep(LEASE_REFRESH) => {
                                if let Err(e) = storage.touch_lease(id) {
                                    log::warn!("không làm mới được lease phiên {id}: {e}");
                                }
                            }
                        }
                    }
                })
            };
            let record = council.run(prompt).await;
            let to_save = record.clone();
            match tokio::task::spawn_blocking(move || writer.write_final(&to_save)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::error!("lưu phiên {id} thất bại: {e}"),
                Err(e) => log::error!("task lưu phiên {id} panic: {e}"),
            }
            lease_stop.cancel();
            let _ = lease.await;
            storage.release_lease(id);
            feed.finish(record);
            log::info!("phiên MCP {id} kết thúc");
        });
        {
            let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
            tasks.retain(|t| !t.is_finished());
            tasks.push(task);
        }
        log::info!("bắt đầu phiên MCP {id}");
        Ok(vec![json_block(&summary)])
    }

    // ── get_council_status ──

    async fn get_status(&self, args: Value) -> ToolResult {
        let a: IdArgs = parse_args(args)?;
        let id = parse_id(&a.session_id)?;
        if let Some(feed) = self.feed(id) {
            let st = feed.lock();
            let (state, status) = match &st.record {
                Some(r) => (state_of(r.status), Some(status_str(r.status))),
                None => ("running", None),
            };
            let latest = st.updates.back().map(|u| u.text.clone());
            return Ok(vec![json_block(&json!({
                "session_id": id,
                "state": state,
                "status": status,
                "progress": st.progress,
                "latest_update": latest,
                "updates_available": st.next_seq.saturating_sub(st.cursor),
                "running_in": "this_mcp_server",
            }))]);
        }
        let rec = self.load_record(id).await?;
        let state = if rec.status == SessionStatus::Running {
            if self.storage.lease_alive(id) { "running" } else { "interrupted" }
        } else {
            state_of(rec.status)
        };
        Ok(vec![json_block(&json!({
            "session_id": id,
            "state": state,
            "status": status_str(rec.status),
            "origin": rec.options.origin.as_deref().unwrap_or("desktop"),
            "created_at": rec.created_at,
            "finished_at": rec.finished_at,
            "plan_versions": rec.plans.len(),
            "rounds": rec.rounds.len(),
            "last_round": rec.rounds.last().map(round_brief),
            "tokens": rec.usage.total.total(),
            "error": rec.error,
            "running_in": if state == "running" { "another_process" } else { "none" },
        }))])
    }

    // ── get_council_updates ──

    async fn get_updates(&self, args: Value) -> ToolResult {
        let a: UpdatesArgs = parse_args(args)?;
        let id = parse_id(&a.session_id)?;
        let feed = self.feed(id).ok_or_else(|| {
            ToolError(format!(
                "Phiên {id} không chạy trong MCP server này (đã chạy ở app desktop hoặc trước khi server khởi động lại). Dùng get_council_status / get_council_result."
            ))
        })?;
        let limit = a.limit.unwrap_or(DEFAULT_UPDATES_LIMIT).clamp(1, MAX_UPDATES_LIMIT);
        let since = a.since.unwrap_or_else(|| feed.lock().cursor);
        let wait = Duration::from_secs(a.wait_seconds.unwrap_or(0).min(MAX_WAIT_SECS));
        feed.wait_until(wait, |f| {
            let st = f.lock();
            st.next_seq > since || st.record.is_some()
        })
        .await;

        let mut st = feed.lock();
        let oldest = st.updates.front().map_or(st.next_seq, |u| u.seq);
        let updates: Vec<Update> = st.updates.iter().filter(|u| u.seq >= since).take(limit).cloned().collect();
        let next_since = updates.last().map_or(since.max(oldest), |u| u.seq + 1);
        st.cursor = st.cursor.max(next_since);
        let more = st.next_seq > next_since;
        let finished = st.record.is_some();
        Ok(vec![json_block(&json!({
            "session_id": id,
            "updates": updates,
            "next_since": next_since,
            "more": more,
            "missed": since < oldest,
            "finished": finished && !more,
            "state": st.record.as_ref().map_or("running", |r| state_of(r.status)),
            "progress": st.progress,
        }))])
    }

    // ── get_council_result ──

    async fn get_result(&self, args: Value) -> ToolResult {
        let a: ResultArgs = parse_args(args)?;
        let id = parse_id(&a.session_id)?;
        let max_chars = a.max_plan_chars.unwrap_or(DEFAULT_PLAN_CHARS).clamp(1_000, MAX_PLAN_CHARS);
        let include_plan = a.include_plan.unwrap_or(true);
        let feed = self.feed(id);
        if let Some(f) = &feed {
            f.wait_until(Duration::from_secs(a.wait_seconds.unwrap_or(0).min(MAX_WAIT_SECS)), Feed::finished).await;
        }
        let (rec, finished) = match feed.as_ref().and_then(|f| f.lock().record.clone()) {
            Some(r) => ((*r).clone(), true),
            None => {
                // Đang chạy (ở đây hoặc tiến trình khác) → checkpoint gần nhất; hoặc phiên cũ trên đĩa.
                let r = self.load_record(id).await.map_err(|e| match &feed {
                    Some(_) => ToolError(format!("Phiên {id} đang chạy, chưa có checkpoint nào — thử lại sau.")),
                    None => e,
                })?;
                let done = r.status != SessionStatus::Running;
                (r, done)
            }
        };
        let pricing = self.load_config().await.map(|c| c.pricing).unwrap_or_default();
        let (summary, plan) = build_result(&rec, finished, &pricing, include_plan, max_chars);
        let mut out = vec![json_block(&summary)];
        if let Some(p) = plan {
            out.push(text_block(p));
        }
        Ok(out)
    }

    // ── cancel_council ──

    async fn cancel_council(&self, args: Value) -> ToolResult {
        let a: IdArgs = parse_args(args)?;
        let id = parse_id(&a.session_id)?;
        if let Some(l) = self.active.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|l| l.id == id) {
            l.cancel.cancel();
            return Ok(vec![json_block(&json!({
                "session_id": id,
                "cancelled": true,
                "message": "Đang dừng. Plan hợp lệ gần nhất được giữ — gọi get_council_result (wait_seconds ~10) để lấy.",
            }))]);
        }
        if self.feed(id).is_some() {
            return Ok(vec![json_block(&json!({ "session_id": id, "cancelled": false, "message": "Phiên đã kết thúc." }))]);
        }
        let rec = self.load_record(id).await?;
        let msg = if rec.status == SessionStatus::Running && self.storage.lease_alive(id) {
            "Phiên đang chạy ở tiến trình khác (MCP server khác) — không dừng được từ đây."
        } else if rec.status == SessionStatus::Running {
            "Phiên chạy ở app desktop hoặc đã bị gián đoạn — dừng từ app."
        } else {
            "Phiên đã kết thúc."
        };
        Ok(vec![json_block(&json!({ "session_id": id, "cancelled": false, "message": msg }))])
    }

    // ── send_council_message ──

    fn send_message(&self, args: Value) -> ToolResult {
        let a: MessageArgs = parse_args(args)?;
        let id = parse_id(&a.session_id)?;
        let control = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|l| l.id == id)
            .map(|l| l.control.clone())
            .ok_or_else(|| ToolError(format!("Phiên {id} không còn chạy trong MCP server này — không gửi được thông tin bổ sung.")))?;
        let note = control.add_note(&a.message).map_err(ToolError)?;
        let n = control.notes().len();
        if let Some(feed) = self.feed(id) {
            feed.push("user_note", format!("Thông tin bổ sung #{n}: {}", clip(&note.text, 200)), None);
        }
        Ok(vec![json_block(&json!({
            "session_id": id,
            "accepted": true,
            "note_number": n,
            "applies": "Từ lần gọi model kế tiếp: người viết plan và reviewer đều nhận thông tin này. Nếu cuộc họp đã hội tụ ngay sau đó thì có thể không còn lượt chỉnh sửa nào dùng tới — khi đó hãy chạy phiên mới.",
        }))])
    }

    // ── list_councils ──

    async fn list_councils(&self, args: Value) -> ToolResult {
        let a: ListArgs = parse_args(args)?;
        let limit = a.limit.unwrap_or(10).clamp(1, 50);
        let (storage, history) = (self.storage.clone(), self.history.clone());
        let list = tokio::task::spawn_blocking(move || history.list_sessions(&storage, limit))
            .await
            .map_err(|e| ToolError(format!("lỗi nội bộ: {e}")))?
            .map_err(|e| ToolError(format!("[storage] {e}")))?;
        let active = self.active_id();
        let rows: Vec<Value> = list
            .iter()
            .map(|s| {
                json!({
                    "session_id": s.id,
                    "created_at": s.created_at,
                    "status": status_str(s.status),
                    "prompt_preview": clip(&s.prompt_preview, 200),
                    "plan_versions": s.versions,
                    "rounds": s.rounds,
                    "tokens": s.total_tokens,
                    "running_here": active == Some(s.id),
                })
            })
            .collect();
        Ok(vec![json_block(&json!({ "sessions": rows }))])
    }
}

// ───────────────────────────── Tham số tool ─────────────────────────────

#[derive(Deserialize)]
struct StartArgs {
    prompt: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    web_search: Option<bool>,
    #[serde(default)]
    company_context: Option<bool>,
    #[serde(default)]
    max_rounds: Option<u32>,
    #[serde(default)]
    departments: Option<Vec<String>>,
    #[serde(default)]
    chair_id: Option<String>,
    #[serde(default)]
    reviewer_ids: Option<Vec<String>>,
    #[serde(default)]
    auto_staff: Option<bool>,
    #[serde(default)]
    plan_depth: Option<PlanDepth>,
}

#[derive(Deserialize)]
struct IdArgs {
    session_id: String,
}

#[derive(Deserialize)]
struct UpdatesArgs {
    session_id: String,
    #[serde(default)]
    since: Option<u64>,
    #[serde(default)]
    wait_seconds: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ResultArgs {
    session_id: String,
    #[serde(default)]
    wait_seconds: Option<u64>,
    #[serde(default)]
    include_plan: Option<bool>,
    #[serde(default)]
    max_plan_chars: Option<usize>,
}

#[derive(Deserialize)]
struct MessageArgs {
    session_id: String,
    message: String,
}

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    limit: Option<usize>,
}

fn parse_args<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError(format!("[invalid] tham số không hợp lệ: {e}")))
}

fn parse_id(s: &str) -> Result<Uuid, ToolError> {
    Uuid::parse_str(s.trim()).map_err(|_| ToolError(format!("[invalid] session_id không hợp lệ: {s:?}")))
}

/// Yêu cầu + bối cảnh Claude Code gửi kèm → prompt của phiên (giới hạn độ dài).
fn compose_prompt(prompt: &str, context: Option<&str>) -> Result<String, ToolError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("[invalid] prompt không được để trống".into());
    }
    let context = context.map(str::trim).filter(|c| !c.is_empty());
    if context.is_some_and(|c| c.chars().count() > MAX_CONTEXT_CHARS) {
        return Err(ToolError(format!("[invalid] context vượt quá {MAX_CONTEXT_CHARS} ký tự — tóm tắt bớt hoặc chỉ gửi phần liên quan")));
    }
    let full = match context {
        Some(c) => format!("{prompt}\n\n## Bối cảnh bổ sung (từ Claude Code)\n\n{c}"),
        None => prompt.to_owned(),
    };
    if full.chars().count() > MAX_PROMPT_CHARS {
        return Err(ToolError(format!("[invalid] prompt + context vượt quá {MAX_PROMPT_CHARS} ký tự")));
    }
    Ok(full)
}

/// Chỉ cho phép workspace đã khai báo trong Cài đặt (khớp id, tên, hoặc đường dẫn thật).
fn resolve_workspace(cfg: &AppConfig, wanted: &str) -> Result<String, ToolError> {
    let canon = |p: &str| std::fs::canonicalize(p.trim()).ok();
    let wanted_path = canon(wanted);
    let hit = cfg.workspaces.iter().find(|w| {
        w.id == wanted || w.name.eq_ignore_ascii_case(wanted) || (wanted_path.is_some() && canon(&w.path) == wanted_path)
    });
    match hit {
        Some(w) => Ok(w.id.clone()),
        None => {
            let known: Vec<String> = cfg.workspaces.iter().map(|w| format!("{} ({})", w.name, w.path)).collect();
            Err(ToolError(format!(
                "[invalid] workspace '{wanted}' chưa được cấp quyền. PlanCouncil chỉ đọc workspace đã khai báo trong app (Cài đặt → Bối cảnh & Công cụ → Workspace). Đang có: {}",
                if known.is_empty() { "(chưa có)".to_owned() } else { known.join("; ") }
            )))
        }
    }
}

fn role_name(cfg: &AppConfig, id: &str) -> String {
    cfg.role(id).map_or_else(|| id.to_owned(), |r| r.name.clone())
}

// ───────────────────────────── Kết quả ─────────────────────────────

fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Approve => "APPROVE",
        Verdict::Reject => "REJECT",
        Verdict::Abstain => "ABSTAIN",
    }
}

fn status_str(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Running => "running",
        SessionStatus::Converged => "converged",
        SessionStatus::MaxRoundsReached => "max_rounds_reached",
        SessionStatus::PartiallyApproved => "partially_approved",
        SessionStatus::Truncated => "truncated",
        SessionStatus::BudgetExhausted => "budget_exhausted",
        SessionStatus::Unresolved => "unresolved",
        SessionStatus::Interrupted => "interrupted",
        SessionStatus::Cancelled => "cancelled",
        SessionStatus::Failed => "failed",
    }
}

/// Trạng thái gọn cho client: running | completed | failed | cancelled | interrupted.
fn state_of(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Running => "running",
        SessionStatus::Failed => "failed",
        SessionStatus::Cancelled => "cancelled",
        SessionStatus::Interrupted => "interrupted",
        _ => "completed",
    }
}

fn round_brief(r: &RoundRecord) -> RoundBrief {
    let total = r.slots.iter().filter(|s| !s.review.as_ref().is_some_and(|rv| rv.verdict == Verdict::Abstain)).count() as u32;
    RoundBrief { round: r.round, plan_version: r.plan_version, approvals: r.approvals, total, converged: r.converged }
}

/// JSON tóm tắt + (tuỳ chọn) plan markdown riêng 1 khối text cho dễ đọc.
fn build_result(
    rec: &SessionRecord,
    finished: bool,
    pricing: &config::Pricing,
    include_plan: bool,
    max_chars: usize,
) -> (Value, Option<String>) {
    let status = rec.status;
    let conclusion = match status {
        SessionStatus::Running => "Phiên đang chạy — đây là checkpoint gần nhất, chưa phải kết quả cuối.",
        SessionStatus::Converged => "Hội tụ: mọi reviewer đã APPROVE plan cuối.",
        SessionStatus::PartiallyApproved => "Reviewer còn hoạt động đều APPROVE, nhưng có reviewer lỗi / bị cắt output — nên tự kiểm tra thêm phần của họ.",
        SessionStatus::MaxRoundsReached => "Hết số vòng mà vẫn còn REJECT — xem open_issues; plan cuối có phụ lục Open Issues.",
        SessionStatus::Unresolved => "Mọi reviewer APPROVE nhưng kiểm tra nhất quán giữa các section chưa đạt — xem phụ lục trong plan.",
        SessionStatus::Truncated => "Plan mới bị cắt do giới hạn token và không nối tiếp được — giữ phiên bản hợp lệ gần nhất.",
        SessionStatus::BudgetExhausted => "Hết ngân sách token của phiên — giữ phiên bản gần nhất.",
        SessionStatus::Interrupted => "Phiên bị gián đoạn — kết quả là phiên bản hợp lệ gần nhất (nếu có).",
        SessionStatus::Cancelled => "Phiên đã bị dừng — kết quả là phiên bản hợp lệ gần nhất (nếu có).",
        SessionStatus::Failed => "Phiên lỗi — xem error.",
    };
    let confidence = match status {
        SessionStatus::Converged => "high",
        SessionStatus::PartiallyApproved | SessionStatus::Unresolved => "medium",
        _ => "low",
    };
    let last = rec.rounds.last();
    let mut reviewers = Vec::new();
    let mut open_issues = Vec::new();
    let mut dissent = Vec::new();
    let mut suggestions = Vec::new();
    for slot in last.map(|r| r.slots.as_slice()).unwrap_or_default() {
        match (&slot.review, &slot.error) {
            (Some(r), _) => {
                reviewers.push(json!({
                    "reviewer": r.role,
                    "verdict": verdict_str(r.verdict),
                    "summary": clip(&r.summary_bubble, MAX_ITEM_CHARS),
                }));
                if r.verdict == Verdict::Reject {
                    dissent.push(r.role.clone());
                    for i in r.critical_issues.iter().take(10) {
                        open_issues.push(json!({ "reviewer": r.role, "issue": clip(i, MAX_ITEM_CHARS) }));
                    }
                }
                for s in r.suggestions.iter().take(5) {
                    suggestions.push(json!({ "reviewer": r.role, "suggestion": clip(s, MAX_ITEM_CHARS) }));
                }
            }
            (None, Some(e)) => {
                let name = rec.cast.iter().find(|c| c.id == slot.agent).map_or(slot.agent.as_str(), |c| c.name.as_str());
                reviewers.push(json!({ "reviewer": name, "verdict": "ERROR", "summary": clip(e, MAX_ITEM_CHARS) }));
            }
            _ => {}
        }
    }
    let usage = crate::cost::usage_breakdown(rec, pricing);
    let by_model: serde_json::Map<String, Value> =
        rec.usage.by_model.iter().map(|(m, u)| (m.clone(), json!({ "tokens": u.total(), "calls": u.calls }))).collect();
    let plan_md = rec.final_markdown.clone().or_else(|| rec.plans.last().map(|p| p.markdown.clone()));
    let plan_version = rec.plans.last().map(|p| p.version);
    let plan_chars = plan_md.as_deref().map_or(0, |p| p.chars().count());
    let truncated = plan_chars > max_chars;
    let summary = json!({
        "session_id": rec.id,
        "finished": finished,
        "state": state_of(status),
        "status": status_str(status),
        "conclusion": conclusion,
        "confidence": confidence,
        "approval": last.map(round_brief),
        "plan_version": plan_version,
        "plan_chars": plan_chars,
        "plan_included": include_plan && plan_md.is_some(),
        "plan_truncated": include_plan && truncated,
        "dissent": dissent,
        "open_issues": open_issues,
        "suggestions": suggestions,
        "reviewers": reviewers,
        "departments": rec.departments.iter().map(|d| json!({
            "name": d.name,
            "status": status_str(d.status),
            "error": d.error,
        })).collect::<Vec<_>>(),
        "user_notes": rec.user_notes.iter().map(|n| clip(&n.text, MAX_ITEM_CHARS)).collect::<Vec<_>>(),
        "usage": {
            "tokens": rec.usage.total.total(),
            "calls": rec.usage.total.calls,
            "cost_usd": usage.totals.cost_usd,
            "by_model": by_model,
        },
        "error": rec.error,
    });
    let plan = plan_md.filter(|_| include_plan).map(|md| {
        let label = if finished { "Final plan" } else { "Plan (checkpoint, chưa xong)" };
        let head = format!("# {label} v{}\n\n", plan_version.unwrap_or(0));
        if truncated {
            let kept: String = md.chars().take(max_chars).collect();
            format!(
                "{head}{kept}\n\n…[cắt bớt: còn {} ký tự — gọi lại với max_plan_chars lớn hơn (tối đa {MAX_PLAN_CHARS}) hoặc xem trong app]",
                plan_chars - max_chars
            )
        } else {
            format!("{head}{md}")
        }
    });
    (summary, plan)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn text_block(text: String) -> Value {
    json!({ "type": "text", "text": text })
}

fn json_block(v: &Value) -> Value {
    text_block(serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
}

fn rpc_error(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

// ───────────────────────────── Mô tả tool ─────────────────────────────

const INSTRUCTIONS: &str = "PlanCouncil runs a multi-agent review meeting: a chair writes a plan, reviewers critique it \
over several rounds until they all APPROVE (or rounds run out). Typical flow: start_council → get_council_updates \
(wait_seconds ~30, repeat) or get_council_result(wait_seconds ~50, repeat) → act on the result. Meetings take minutes. \
One meeting runs at a time per server. Pass only the context the council needs; it can read files only inside workspaces \
the user configured in the PlanCouncil app.";

fn tool_defs() -> Value {
    let id = json!({ "type": "string", "description": "session_id returned by start_council (UUID)." });
    let wait = json!({ "type": "integer", "minimum": 0, "maximum": MAX_WAIT_SECS, "description": "Long-poll: wait up to this many seconds for news before returning (0 = return immediately)." });
    json!([
        {
            "name": "start_council",
            "title": "Start a PlanCouncil meeting",
            "description": "Start a multi-agent review meeting on a problem. Returns session_id immediately; the meeting runs in the background (usually several minutes). Uses the roles, models and API keys configured in the PlanCouncil app.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "The problem / request and goals for the council. Be specific about the expected outcome.", "maxLength": MAX_PROMPT_CHARS },
                    "context": { "type": "string", "description": "Optional extra context (relevant code excerpts, constraints, decisions already made). Do not dump whole files.", "maxLength": MAX_CONTEXT_CHARS },
                    "workspace": { "type": "string", "description": "Optional workspace the council may read: id, name or path of a workspace already configured in the app. Other paths are rejected." },
                    "web_search": { "type": "boolean", "description": "Allow web search (default: app setting)." },
                    "company_context": { "type": "boolean", "description": "Apply the company context from app settings (default: app setting)." },
                    "max_rounds": { "type": "integer", "minimum": 1, "maximum": 10, "description": "Max review rounds of the main meeting (default: app setting)." },
                    "departments": { "type": "array", "items": { "type": "string" }, "description": "Department ids that meet first (default: app setting; [] = none)." },
                    "chair_id": { "type": "string", "description": "Role id of the chair (plan author). Default: app setting." },
                    "reviewer_ids": { "type": "array", "items": { "type": "string" }, "description": "Role ids of reviewers. Default: app setting." },
                    "auto_staff": { "type": "boolean", "description": "Let AI choose chair/reviewers/departments for this request." },
                    "plan_depth": { "type": "string", "enum": ["mvp", "standard", "enterprise"], "description": "Level of detail of the plan." }
                },
                "required": ["prompt"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_council_status",
            "title": "Council status",
            "description": "Current state and progress of a meeting (phase, round, plan version, last round votes, tokens). Works for past sessions too.",
            "inputSchema": { "type": "object", "properties": { "session_id": id }, "required": ["session_id"], "additionalProperties": false },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_council_updates",
            "title": "Council updates",
            "description": "New events of a running meeting since the last call (plan versions, reviews with verdicts, rounds, membership changes). Use next_since as `since` next time, or omit `since` to continue from the server-side cursor.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": id,
                    "since": { "type": "integer", "minimum": 0, "description": "Return updates with seq >= since." },
                    "wait_seconds": wait,
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_UPDATES_LIMIT }
                },
                "required": ["session_id"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_council_result",
            "title": "Council result",
            "description": "Structured outcome: conclusion, confidence, approval votes, dissent, open issues, suggestions, usage — plus the final plan markdown as a second text block. While running, returns the latest checkpoint (finished=false); use wait_seconds to wait for the end.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": id,
                    "wait_seconds": wait,
                    "include_plan": { "type": "boolean", "description": "Include the plan markdown (default true)." },
                    "max_plan_chars": { "type": "integer", "minimum": 1000, "maximum": MAX_PLAN_CHARS, "description": "Truncate the plan to this many characters (default 60000)." }
                },
                "required": ["session_id"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "cancel_council",
            "title": "Cancel council",
            "description": "Stop a meeting running in this server. The latest valid plan version is kept as the result.",
            "inputSchema": { "type": "object", "properties": { "session_id": id }, "required": ["session_id"], "additionalProperties": false },
            "annotations": { "destructiveHint": true, "idempotentHint": true }
        },
        {
            "name": "send_council_message",
            "title": "Send info to council",
            "description": "Add information or answer an open question during a running meeting (max 20 per session, 4000 chars each). It is given to the plan author and reviewers from the next model call on; it does not restart finished rounds.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": id, "message": { "type": "string", "maxLength": 4000 } },
                "required": ["session_id", "message"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_councils",
            "title": "List councils",
            "description": "Most recent meetings (from the app's history), newest first.",
            "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 50 } }, "additionalProperties": false },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Review, ReviewSlot};

    fn review(agent: &str, verdict: Verdict, issues: &[&str]) -> Review {
        Review {
            agent: agent.into(),
            role: agent.to_uppercase(),
            verdict,
            summary_bubble: format!("{agent} nói"),
            critical_issues: issues.iter().map(|s| s.to_string()).collect(),
            suggestions: vec!["gợi ý".into()],
            out_of_scope: vec![],
            invites: vec![],
            sections: vec![],
            carried_over: false,
            policy_override: None,
        }
    }

    #[test]
    fn compose_prompt_validates_and_appends_context() {
        assert!(compose_prompt("  ", None).is_err());
        assert_eq!(compose_prompt(" làm X ", None).ok().unwrap(), "làm X");
        let p = compose_prompt("làm X", Some("code: fn a()")).ok().unwrap();
        assert!(p.starts_with("làm X\n\n## Bối cảnh bổ sung"));
        assert!(p.ends_with("fn a()"));
        assert!(compose_prompt("x", Some(&"a".repeat(MAX_CONTEXT_CHARS + 1))).is_err());
    }

    #[test]
    fn workspace_must_be_configured() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::default();
        cfg.workspaces.push(config::Workspace { id: "w1".into(), name: "Game".into(), path: dir.path().display().to_string() });
        assert_eq!(resolve_workspace(&cfg, "w1").ok().unwrap(), "w1");
        assert_eq!(resolve_workspace(&cfg, "game").ok().unwrap(), "w1");
        assert_eq!(resolve_workspace(&cfg, &format!("{}/.", dir.path().display())).ok().unwrap(), "w1", "khớp theo đường dẫn thật");
        let err = resolve_workspace(&cfg, "/etc").err().unwrap().0;
        assert!(err.contains("chưa được cấp quyền"), "{err}");
    }

    #[test]
    fn result_reports_dissent_and_open_issues() {
        let mut rec = SessionRecord::new(Uuid::new_v4(), "x".into(), 3);
        rec.status = SessionStatus::MaxRoundsReached;
        rec.plans.push(crate::models::PlanVersion::new(2, "# Plan\nnội dung".into(), Default::default()));
        rec.final_markdown = Some("# Plan\nnội dung\n## Open Issues".into());
        rec.rounds.push(RoundRecord {
            round: 2,
            plan_version: 2,
            slots: vec![
                ReviewSlot { agent: "a".into(), review: Some(review("a", Verdict::Approve, &[])), error: None },
                ReviewSlot { agent: "b".into(), review: Some(review("b", Verdict::Reject, &["thiếu rollback"])), error: None },
                ReviewSlot { agent: "c".into(), review: None, error: Some("timeout".into()) },
            ],
            approvals: 1,
            converged: false,
        });
        let (v, plan) = build_result(&rec, true, &Default::default(), true, 1_000);
        assert_eq!(v["state"], "completed");
        assert_eq!(v["status"], "max_rounds_reached");
        assert_eq!(v["confidence"], "low");
        assert_eq!(v["dissent"], json!(["B"]));
        assert_eq!(v["open_issues"][0]["issue"], "thiếu rollback");
        assert_eq!(v["reviewers"][2]["verdict"], "ERROR");
        assert_eq!(v["approval"]["approvals"], 1);
        assert_eq!(v["approval"]["total"], 3);
        assert!(plan.unwrap().contains("## Open Issues"));

        let (v, plan) = build_result(&rec, true, &Default::default(), false, 1_000);
        assert!(plan.is_none());
        assert_eq!(v["plan_included"], false);
    }

    #[test]
    fn long_plans_are_truncated() {
        let mut rec = SessionRecord::new(Uuid::new_v4(), "x".into(), 3);
        rec.status = SessionStatus::Converged;
        rec.final_markdown = Some("é".repeat(5_000));
        let (v, plan) = build_result(&rec, true, &Default::default(), true, 1_000);
        assert_eq!(v["plan_truncated"], true);
        assert!(plan.unwrap().contains("cắt bớt: còn 4000 ký tự"));
    }

    #[tokio::test]
    async fn feed_tracks_progress_and_long_polls() {
        let feed = Arc::new(Feed::new(3));
        let f2 = feed.clone();
        let waiter = tokio::spawn(async move {
            f2.wait_until(Duration::from_secs(5), |f| f.lock().next_seq > 0).await;
            f2.lock().next_seq
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        feed.on_event(&CouncilEvent::PlanReady { scope: MAIN_SCOPE.into(), version: 1, markdown: "abc".into() });
        assert_eq!(waiter.await.unwrap(), 1);
        feed.on_event(&CouncilEvent::PlanDelta { scope: MAIN_SCOPE.into(), version: 2, delta: "x".into() });
        assert_eq!(feed.lock().next_seq, 1, "delta không thành cập nhật");
        feed.on_event(&CouncilEvent::RoundCompleted {
            scope: MAIN_SCOPE.into(),
            round: 1,
            plan_version: 1,
            approvals: 1,
            total: 2,
            converged: false,
        });
        let st = feed.lock();
        assert_eq!(st.progress.phase, "revising");
        assert_eq!(st.progress.plan_version, 1);
        assert_eq!(st.progress.last_round.as_ref().unwrap().approvals, 1);
        assert!(st.updates.back().unwrap().text.contains("1/2 APPROVE"));
    }

    struct NoKeys;
    impl crate::secrets::SecretStore for NoKeys {
        fn get(&self, _: &str) -> Result<Option<String>, crate::secrets::SecretError> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> Result<(), crate::secrets::SecretError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), crate::secrets::SecretError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn rpc_handshake_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf()).unwrap();
        let server = Arc::new(Server {
            history: HistoryIndex::open(&storage),
            secrets: Arc::new(Secrets::new(Arc::new(NoKeys), storage.root())),
            storage,
            http: reqwest::Client::new(),
            active: Mutex::new(None),
            feeds: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Vec::new()),
        });
        let init = server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{}}}"#)
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&init).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
        assert!(server.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await.is_none());

        let list: Value = serde_json::from_str(&server.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).await.unwrap()).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 7);
        assert!(names.contains(&"send_council_message"));

        let bad: Value = serde_json::from_str(&server.handle_line("{nope").await.unwrap()).unwrap();
        assert_eq!(bad["error"]["code"], -32700);
        let unknown: Value = serde_json::from_str(&server.handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"x/y"}"#).await.unwrap()).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);

        // Lỗi của tool → isError (model tự sửa được), không phải lỗi JSON-RPC.
        let call = |args: &str| format!(r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"get_council_status","arguments":{args}}}}}"#);
        let r: Value = serde_json::from_str(&server.handle_line(&call(r#"{"session_id":"abc"}"#)).await.unwrap()).unwrap();
        assert_eq!(r["result"]["isError"], true);
        let missing = Uuid::new_v4();
        let r: Value = serde_json::from_str(&server.handle_line(&call(&format!(r#"{{"session_id":"{missing}"}}"#))).await.unwrap()).unwrap();
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("not_found"));
        let r: Value = serde_json::from_str(
            &server
                .handle_line(&format!(r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"send_council_message","arguments":{{"session_id":"{missing}","message":"hi"}}}}}}"#))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
    }

    /// Keyring giả: mọi provider đều có key.
    struct AllKeys;
    impl crate::secrets::SecretStore for AllKeys {
        fn get(&self, _: &str) -> Result<Option<String>, crate::secrets::SecretError> {
            Ok(Some("sk-test".into()))
        }
        fn set(&self, _: &str, _: &str) -> Result<(), crate::secrets::SecretError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), crate::secrets::SecretError> {
            Ok(())
        }
    }

    async fn call(server: &Arc<Server>, name: &str, args: Value) -> (bool, Vec<String>) {
        let line = json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": name, "arguments": args } }).to_string();
        let v: Value = serde_json::from_str(&server.handle_line(&line).await.unwrap()).unwrap();
        let texts = v["result"]["content"].as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap().to_owned()).collect();
        (v["result"]["isError"].as_bool().unwrap(), texts)
    }

    /// Trọn vòng qua MCP: start → updates (long-poll) → send_council_message → result.
    /// Reviewer REJECT cho tới khi thấy thông tin bổ sung của người yêu cầu.
    #[tokio::test]
    async fn end_to_end_meeting_with_mid_meeting_message() {
        use crate::test_support::{completion, spawn_mock, Handler};
        let handler: Handler = Arc::new(|body: &Value| {
            let sys = body["messages"][0]["content"].as_str().unwrap_or_default().to_owned();
            let all: String = body["messages"].as_array().unwrap().iter().filter_map(|m| m["content"].as_str()).collect::<Vec<_>>().join("\n");
            if sys.contains("[PLAN AUTHOR]") {
                let mut r = completion("## 1. Overview\nPlan\n## 2. Architecture\nA\n## 3. Data Flow\nB\n## 4. Tech Stack & Schema\nC\n## 5. Edge Cases & Trade-offs\nD\n");
                r.delay = Duration::from_millis(150);
                return r;
            }
            let verdict = if all.contains("<requester_updates>") { "APPROVE" } else { "REJECT" };
            completion(&json!({"verdict": verdict, "summary_bubble": format!("{verdict} nhé"), "critical_issues": if verdict == "REJECT" { json!(["Chưa rõ yêu cầu offline"]) } else { json!([]) }, "suggestions": []}).to_string())
        });
        let base = spawn_mock(handler).await;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf()).unwrap();
        let mut cfg = crate::config::tests::with_default_provider("sk-test", "gpt-4.1-mini");
        cfg.providers[0].base_url = base;
        cfg.max_rounds = 4;
        cfg.meeting.department_ids.clear();
        cfg.meeting.staffing = config::StaffingMode::Manual;
        storage.save_config(&cfg).unwrap();
        let server = Arc::new(Server {
            history: HistoryIndex::open(&storage),
            secrets: Arc::new(Secrets::new(Arc::new(AllKeys), storage.root())),
            storage: storage.clone(),
            http: llm_client::build_http_client().unwrap(),
            active: Mutex::new(None),
            feeds: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Vec::new()),
        });

        let (err, out) = call(&server, "start_council", json!({ "prompt": "Thiết kế đồng bộ dữ liệu", "workspace": "/etc" })).await;
        assert!(err && out[0].contains("chưa được cấp quyền"), "workspace lạ bị từ chối: {out:?}");
        let (err, out) = call(&server, "start_council", json!({ "prompt": "Thiết kế đồng bộ dữ liệu", "context": "app mobile" })).await;
        assert!(!err, "{out:?}");
        let started: Value = serde_json::from_str(&out[0]).unwrap();
        let id = started["session_id"].as_str().unwrap().to_owned();
        assert!(storage.lease_alive(Uuid::parse_str(&id).unwrap()), "phiên đang chạy giữ lease");
        let (err, out) = call(&server, "start_council", json!({ "prompt": "phiên thứ hai" })).await;
        assert!(err && out[0].contains("already_running"));

        // Theo dõi tới khi round 1 xong (toàn REJECT), rồi bổ sung thông tin.
        let mut texts = Vec::new();
        for _ in 0..50 {
            let (_, out) = call(&server, "get_council_updates", json!({ "session_id": id, "wait_seconds": 5 })).await;
            let v: Value = serde_json::from_str(&out[0]).unwrap();
            texts.extend(v["updates"].as_array().unwrap().iter().map(|u| u["text"].as_str().unwrap().to_owned()));
            if v["updates"].as_array().unwrap().iter().any(|u| u["kind"] == "round") || v["finished"] == true {
                break;
            }
        }
        assert!(texts.iter().any(|t| t.contains("Bắt đầu họp")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("REJECT")), "{texts:?}");
        let (err, out) = call(&server, "send_council_message", json!({ "session_id": id, "message": "Phải chạy offline, đồng bộ khi có mạng" })).await;
        assert!(!err, "{out:?}");

        let (err, out) = call(&server, "get_council_status", json!({ "session_id": id })).await;
        assert!(!err);
        let st: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(st["running_in"], "this_mcp_server");

        let mut result = None;
        for _ in 0..10 {
            let (err, out) = call(&server, "get_council_result", json!({ "session_id": id, "wait_seconds": 10 })).await;
            assert!(!err, "{out:?}");
            let v: Value = serde_json::from_str(&out[0]).unwrap();
            if v["finished"] == true {
                result = Some((v, out));
                break;
            }
        }
        let (v, out) = result.expect("phiên kết thúc");
        assert_eq!(v["status"], "converged", "{v}");
        assert_eq!(v["confidence"], "high");
        assert_eq!(v["user_notes"], json!(["Phải chạy offline, đồng bộ khi có mạng"]));
        assert!(out[1].starts_with("# Final plan v2"), "{}", out[1]);

        // Đã lưu vào lịch sử (đánh dấu nguồn MCP) và trả lease.
        let uid = Uuid::parse_str(&id).unwrap();
        let rec = storage.load_session(uid).unwrap();
        assert_eq!(rec.options.origin.as_deref(), Some("mcp"));
        assert!(rec.prompt.contains("## Bối cảnh bổ sung (từ Claude Code)"));
        assert_eq!(rec.user_notes.len(), 1);
        assert!(!storage.lease_alive(uid));
        let (err, out) = call(&server, "send_council_message", json!({ "session_id": id, "message": "muộn" })).await;
        assert!(err && out[0].contains("không còn chạy"));
        let (_, out) = call(&server, "cancel_council", json!({ "session_id": id })).await;
        assert!(out[0].contains("\"cancelled\": false"));
    }

    #[tokio::test]
    async fn cancel_stops_a_running_meeting() {
        use crate::test_support::{completion, spawn_mock, Handler};
        let handler: Handler = Arc::new(|_: &Value| {
            let mut r = completion("## 1. Overview\nPlan\n");
            r.delay = Duration::from_secs(5);
            r
        });
        let base = spawn_mock(handler).await;
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf()).unwrap();
        let mut cfg = crate::config::tests::with_default_provider("sk-test", "gpt-4.1-mini");
        cfg.providers[0].base_url = base;
        cfg.meeting.department_ids.clear();
        cfg.meeting.staffing = config::StaffingMode::Manual;
        storage.save_config(&cfg).unwrap();
        let server = Arc::new(Server {
            history: HistoryIndex::open(&storage),
            secrets: Arc::new(Secrets::new(Arc::new(AllKeys), storage.root())),
            storage: storage.clone(),
            http: llm_client::build_http_client().unwrap(),
            active: Mutex::new(None),
            feeds: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Vec::new()),
        });
        let (_, out) = call(&server, "start_council", json!({ "prompt": "x" })).await;
        let id = serde_json::from_str::<Value>(&out[0]).unwrap()["session_id"].as_str().unwrap().to_owned();
        let (err, out) = call(&server, "cancel_council", json!({ "session_id": id })).await;
        assert!(!err && out[0].contains("\"cancelled\": true"), "{out:?}");
        let (_, out) = call(&server, "get_council_result", json!({ "session_id": id, "wait_seconds": 10 })).await;
        let v: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(v["finished"], true);
        assert_eq!(v["state"], "cancelled");
        assert!(server.active_id().is_none(), "slot được trả sau khi dừng");
    }
}
