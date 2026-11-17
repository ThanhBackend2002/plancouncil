//! Hội đồng nhiều tầng: phòng ban họp nội bộ → cuộc họp chính tổng hợp.
//!
//! ```text
//! if có phòng ban tham gia:
//!     tasks = Chair.assign(request, departments)                 // giao việc (JSON)
//!     join_all(dept → maker_checker(lead, members, task))        // các phòng ban họp song song
//! v1 = Chair.draft(request, department_reports)
//! maker_checker(chair, reviewers):                               // cuộc họp chính
//!     for round in 1..=max_rounds:
//!         reviews = join_all(reviewers.review(v))                // song song
//!         all APPROVE → Converged; round cuối → MaxRoundsReached (+ Open Issues)
//!         v = author.refine(v, feedback)
//! ```
//!
//! Orchestrator **không phụ thuộc Tauri**: nó chỉ biết `EventSink` → test được bằng mock HTTP server.
//! Số request LLM đồng thời bị giới hạn bởi semaphore (`max_parallel_calls`).
//!
//! Mọi lần gọi LLM đều giữ chỗ ngân sách token trước (`BudgetLedger`, giới hạn mềm) và settle
//! sau khi xong — kể cả khi lỗi / huỷ (`Hold` tự trả chỗ khi bị drop).
//! Plan bị cắt (`finish_reason=length`) → nối tiếp tối đa `MAX_CONTINUATIONS` lần; không cứu được
//! → dừng vòng họp với `Truncated`, giữ phiên bản hợp lệ gần nhất.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use serde_json::{json, Value};

use crate::budget::{BudgetLedger, ReservationId};
use crate::config::{AppConfig, Department, ResolvedEndpoint, ReviseMode, RoleDef, ToolPolicy, CONSISTENCY_CHECKER_AUTHOR};
use crate::context_builder::{self as cb, FallbackReason};
use crate::llm_client::{
    unwrap_markdown_fence, ChatMessage, Completion, Expect, Finish, LlmClient, LlmError, StreamSignal, Usage,
    DEFAULT_OUTPUT_TOKENS, OUTPUT_TOKENS_CAP,
};
use crate::models::*;
use crate::prompts::{self, CompanyMode, PlanContext, PromptOpts};
use crate::sections::Section;
use crate::{routing, sections, staffing};
use crate::tools::Toolbox;

/// Độ dài tối đa (ký tự, ~100k token) của hội thoại 1 lần nghiên cứu trước khi buộc model viết ghi chú.
/// Chốt an toàn cho chế độ "không giới hạn số lượt công cụ".
const MAX_RESEARCH_CONTEXT_CHARS: usize = 300_000;

/// Số lần thử nối tiếp tối đa khi plan bị cắt do `max_tokens`.
const MAX_CONTINUATIONS: u32 = 2;
/// Chặn an toàn khi `max_tool_steps = 0` (không giới hạn): tránh model gọi công cụ vô tận.
pub const UNLIMITED_TOOL_STEPS_CAP: u32 = 50;

/// Ước lượng để giữ chỗ ngân sách: token prompt (×1.1 bù sai số tokenizer) + trần output.
fn budget_estimate(prompt_tokens: usize, max_tokens: Option<u32>) -> u64 {
    (prompt_tokens as u64) * 11 / 10 + u64::from(max_tokens.unwrap_or(DEFAULT_OUTPUT_TOKENS))
}

fn estimate_call(msgs: &[ChatMessage], max_tokens: Option<u32>) -> u64 {
    budget_estimate(msgs.iter().map(|m| sections::approx_tokens(&m.content)).sum(), max_tokens)
}

/// Như `estimate_call` cho hội thoại có công cụ (message là JSON thô).
fn estimate_raw_call(msgs: &[Value], max_tokens: Option<u32>) -> u64 {
    budget_estimate(msgs.iter().map(|m| sections::approx_tokens(&m.to_string())).sum(), max_tokens)
}

/// Kiểm tra phần nối tiếp của plan bị cắt. Chỉ nhận khi: model tự dừng (`Stop`), dòng đầu
/// (sau khi bỏ dòng trống) đúng là heading được yêu cầu, và bản ghép không còn code fence mở.
/// Trả về markdown đã ghép.
fn accept_continuation(kept: &str, heading: &str, c: &Completion) -> Option<String> {
    if c.finish != Finish::Stop {
        return None;
    }
    let body = unwrap_markdown_fence(&c.text);
    let first = body.lines().next()?;
    if first.trim() != heading.trim() {
        return None;
    }
    let joined = format!("{kept}{body}");
    let (_, fence_open) = sections::scan_lines(&joined);
    (!fence_open).then(|| joined.trim().to_owned())
}

/// Kết quả 1 lần người viết ra plan.
enum Authored {
    Plan(PlanVersion),
    /// Không tạo được phiên bản mới → vòng họp dừng với trạng thái này
    /// (`Truncated` / `BudgetExhausted`), phiên bản trước vẫn là Final.
    Stopped(SessionStatus),
}

/// Kết quả 1 lần thử section revise.
enum SectionRevise {
    /// Patch hợp lệ → plan đầy đủ đã ghép + các section (id giữ nguyên).
    Merged(PlanVersion, Vec<Section>),
    /// Không dùng được → viết lại toàn bộ.
    Fallback(FallbackReason),
    /// Không gọi được (hết ngân sách) → vòng họp dừng.
    Stopped(SessionStatus),
}

/// Kết quả bước kiểm tra nhất quán.
enum Gate {
    Ok,
    Conflicts(Vec<ConsistencyConflict>),
    /// Trả sai định dạng 2 lần / bị cắt / hết ngân sách / lỗi → không được coi là hội tụ.
    Unverified,
}

/// Tên lý do fallback theo serde ("missing_ids", "ratio"…).
fn fallback_name(r: FallbackReason) -> String {
    serde_json::to_value(r).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// Prompt review đã dựng sẵn (để giữ chỗ ngân sách cho cả nhóm trước khi gọi).
struct PreparedReview {
    ep: ResolvedEndpoint,
    messages: Vec<ChatMessage>,
    strict: bool,
    /// Token tiết kiệm nhờ chỉ gửi phần plan được gắn tag (ghi nhận khi thực sự gọi).
    saved: Option<u64>,
}

/// Việc của 1 reviewer trong round: prompt + chỗ giữ, hoặc lỗi dựng prompt.
type ReviewWork<'a> = Result<(PreparedReview, Hold<'a>), LlmError>;

/// Chỗ giữ ngân sách của 1 lần gọi. Không settle tường minh (lỗi / huỷ / `?`) → Drop tự trả chỗ, cộng 0 token.
struct Hold<'a> {
    council: &'a Council,
    id: Option<ReservationId>,
    est: u64,
}

impl Hold<'_> {
    fn settle(mut self, usage: Option<Usage>, fallback: u64) {
        if let Some(id) = self.id.take() {
            self.council.settle_budget(id, usage, fallback);
        }
    }

    /// Lần gọi bị cắt (usage bị mất theo lỗi) → tính bằng số ước lượng lúc giữ chỗ.
    fn settle_truncated(self) {
        let est = self.est;
        self.settle(None, est);
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.council.settle_budget(id, Some(Usage::default()), 0);
        }
    }
}

/// Điểm xuất phát của phiên chạy lại (`rerun_from_version`): bỏ bước nháp, cuộc họp chính
/// bắt đầu bằng việc review đúng plan này với số hiệu `version`.
#[derive(Debug, Clone)]
pub struct Seed {
    pub version: u32,
    pub markdown: String,
    /// Review REJECT của vòng gần nhất đã bác phiên bản này (issue còn mở) — reviewer nhận lại
    /// như "review vòng trước" để kiểm chứng.
    pub reviews: Vec<Review>,
}

/// Nơi nhận ảnh chụp `SessionRecord` (trạng thái `running`) sau mỗi vòng / mỗi phiên bản plan hợp lệ.
/// Không bao giờ chứa phần plan đang stream dở.
pub trait Checkpoint: Send + Sync + 'static {
    fn save(&self, record: SessionRecord);
}

/// Ngữ cảnh bổ sung của 1 phiên: công cụ, bối cảnh công ty, tổng quan workspace.
#[derive(Clone, Default)]
pub struct SessionExtras {
    pub toolbox: Toolbox,
    pub company: CompanyMode,
    /// Tóm tắt workspace (cây thư mục + manifest + README) — đưa vào prompt người viết plan.
    pub overview: Option<String>,
    pub info: SessionInfo,
    /// AI tự chọn thành phần họp ngay khi bắt đầu.
    pub auto_staff: bool,
    /// Lệnh mời / mời ra của người dùng trong lúc họp (áp dụng từ vòng kế tiếp).
    pub control: Arc<MeetingControl>,
    /// Phiên chạy lại: plan xuất phát cho cuộc họp chính.
    pub seed: Option<Seed>,
    /// Nguồn gốc (ghi vào bản ghi).
    pub parent: Option<ParentRef>,
    /// Lưu tiến độ giữa chừng (None = không lưu, như test).
    pub checkpoint: Option<Arc<dyn Checkpoint>>,
}

/// Thông tin của 1 lần gọi LLM để ghi `CallUsage`.
struct CallMeta<'a> {
    scope: &'a str,
    agent: &'a str,
    kind: &'a str,
    model: &'a str,
    round: u32,
    t0: Instant,
}

impl<'a> CallMeta<'a> {
    fn new(scope: &'a str, agent: &'a str, kind: &'a str, model: &'a str, round: u32) -> Self {
        Self { scope, agent, kind, model, round, t0: Instant::now() }
    }
}

fn finish_name(f: Finish) -> &'static str {
    match f {
        Finish::Stop => "stop",
        Finish::Length => "length",
        Finish::Error => "error",
    }
}

/// Sổ ghi từng lần gọi + chi phí cộng dồn (theo bảng giá lúc bắt đầu phiên).
#[derive(Default)]
struct CallLog {
    calls: Vec<CallUsage>,
    next_id: u32,
    cost: f64,
    unpriced: BTreeSet<String>,
}

/// Chép phần đuôi mới của `src` sang `dst` (plan / round chỉ được thêm vào cuối).
fn sync_tail<T: Clone>(dst: &mut Vec<T>, src: &[T]) {
    dst.truncate(src.len());
    let n = dst.len();
    dst.extend_from_slice(&src[n..]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAction {
    Invite { scope: String, role_id: String },
    Dismiss { scope: String, role_id: String },
}

/// Số thông tin bổ sung tối đa trong 1 phiên và độ dài mỗi cái (giữ prompt trong tầm kiểm soát).
pub const MAX_USER_NOTES: usize = 20;
pub const MAX_USER_NOTE_CHARS: usize = 4_000;

/// Hàng đợi lệnh điều khiển thành viên từ UI → orchestrator (đọc ở đầu mỗi vòng review),
/// cộng thông tin bổ sung của người yêu cầu (áp vào mọi lần gọi model sau khi nhận).
#[derive(Debug, Default)]
pub struct MeetingControl {
    pending: Mutex<Vec<ControlAction>>,
    notes: Mutex<Vec<UserNote>>,
}

impl MeetingControl {
    pub fn push(&self, a: ControlAction) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).push(a);
    }

    /// Thêm thông tin bổ sung. Lỗi khi rỗng / quá dài / đã đủ số lượng.
    pub fn add_note(&self, text: &str) -> Result<UserNote, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Nội dung trống".into());
        }
        if text.chars().count() > MAX_USER_NOTE_CHARS {
            return Err(format!("Nội dung vượt quá {MAX_USER_NOTE_CHARS} ký tự"));
        }
        let mut notes = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        if notes.len() >= MAX_USER_NOTES {
            return Err(format!("Đã đạt tối đa {MAX_USER_NOTES} thông tin bổ sung cho 1 phiên"));
        }
        let note = UserNote { ts: Utc::now(), text: text.to_owned() };
        notes.push(note.clone());
        Ok(note)
    }

    pub fn notes(&self) -> Vec<UserNote> {
        self.notes.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn take(&self, scope: &str) -> Vec<ControlAction> {
        let mut q = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let (mine, rest): (Vec<_>, Vec<_>) = q.drain(..).partition(|a| match a {
            ControlAction::Invite { scope: s, .. } | ControlAction::Dismiss { scope: s, .. } => s == scope,
        });
        *q = rest;
        mine
    }
}

fn cast_member(r: &RoleDef) -> CastMember {
    CastMember {
        id: r.id.clone(),
        name: r.name.clone(),
        title: r.title.clone(),
        color: r.color.clone(),
        icon: r.icon.clone(),
        model: r.model.clone(),
        level: r.level,
        appearance: r.appearance.clone(),
    }
}

/// Review "giữ nguyên": phần reviewer phụ trách không đổi so với lần họ đã APPROVE.
fn carried(prev: &Review) -> Review {
    let mut r = prev.clone();
    r.carried_over = true;
    r.summary_bubble = "Phần tôi phụ trách không đổi — giữ APPROVE.".into();
    r.invites.clear();
    r.policy_override = None;
    r
}

/// Ước lượng token khi provider không trả `usage` (~3 ký tự/token, tính cả tiếng Việt có dấu).
fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(3)
}

fn msg_chars(msgs: &[ChatMessage]) -> usize {
    msgs.iter().map(|m| m.content.len()).sum()
}

/// Throttle delta để không flood IPC (mỗi token 1 event ≈ hàng nghìn event/plan).
const DELTA_FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const DELTA_FLUSH_BYTES: usize = 4 * 1024;

pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, envelope: &EventEnvelope);
}

/// Gán `seq` và emit trong cùng 1 critical section → thứ tự event tuyệt đối,
/// kể cả khi nhiều phòng ban / reviewer chạy song song cùng emit.
struct Emitter {
    sink: Arc<dyn EventSink>,
    session_id: Uuid,
    seq: Mutex<u64>,
}

impl Emitter {
    fn send(&self, event: CouncilEvent) {
        let mut seq = self.seq.lock().unwrap_or_else(|e| e.into_inner());
        let env = EventEnvelope {
            session_id: self.session_id,
            seq: *seq,
            ts: Utc::now(),
            event,
        };
        *seq += 1;
        self.sink.emit(&env);
    }
}

enum Halt {
    Cancelled,
    Failed(String),
}

impl From<LlmError> for Halt {
    fn from(e: LlmError) -> Self {
        match e {
            LlmError::Cancelled => Halt::Cancelled,
            other => Halt::Failed(other.to_string()),
        }
    }
}

/// 1 vòng lặp viết–phản biện (cuộc họp chính hoặc 1 phòng ban).
struct LoopSpec<'a> {
    scope: &'a str,
    ctx: PlanContext<'a>,
    author: &'a RoleDef,
    reviewers: Vec<&'a RoleDef>,
    max_rounds: u32,
    /// Báo cáo phòng ban (chỉ dùng cho bản nháp đầu của cuộc họp chính).
    reports: Vec<(String, String)>,
}

#[derive(Default)]
struct LoopResult {
    plans: Vec<PlanVersion>,
    rounds: Vec<RoundRecord>,
    status: Option<SessionStatus>,
    final_markdown: Option<String>,
    /// Mâu thuẫn còn lại khi kiểm tra nhất quán không đạt (đưa vào phụ lục của Final).
    conflicts: Vec<ConsistencyConflict>,
}

#[derive(Deserialize)]
struct RawAssignments {
    #[serde(default)]
    assignments: Vec<RawAssignment>,
}

#[derive(Deserialize)]
struct RawAssignment {
    #[serde(default)]
    department_id: String,
    #[serde(default)]
    task: String,
}

pub struct Council {
    llm: LlmClient,
    cfg: AppConfig,
    emitter: Emitter,
    cancel: CancellationToken,
    limiter: Semaphore,
    extras: SessionExtras,
    usage: Mutex<UsageReport>,
    activities: Mutex<Vec<ToolActivity>>,
    /// Model đã từ chối `tools` → không thử nghiên cứu lại trong phiên.
    no_tool_models: Mutex<HashSet<String>>,
    /// Ghi chú nghiên cứu của reviewer theo "scope/role" (dùng lại ở các round sau).
    reviewer_notes: Mutex<HashMap<String, String>>,
    membership: Mutex<Vec<MembershipChange>>,
    /// Role được mời giữa chừng (bổ sung vào cast của bản ghi).
    extra_cast: Mutex<Vec<CastMember>>,
    /// Ngân sách token của phiên (`meeting.max_total_tokens`).
    budget: BudgetLedger,
    /// Có reviewer bị cắt output ở bất kỳ nơi họp nào → phiên không bao giờ kết thúc `Converged`.
    review_truncated: AtomicBool,
    calls: Mutex<CallLog>,
    /// Bản ghi đang dựng dở (chỉ khi có `extras.checkpoint`) — cập nhật tăng dần ở mỗi checkpoint.
    progress: Mutex<Option<SessionRecord>>,
}

impl Council {
    /// `cfg.meeting.department_ids` = phòng ban tham gia phiên này (caller đã validate).
    pub fn with_extras(
        llm: LlmClient,
        cfg: AppConfig,
        sink: Arc<dyn EventSink>,
        session_id: Uuid,
        cancel: CancellationToken,
        extras: SessionExtras,
    ) -> Self {
        let mut cfg = cfg;
        if extras.seed.is_some() {
            // Plan xuất phát đã qua review → vòng 1 của phiên chạy lại không phải "lần đọc đầu".
            cfg.meeting.strict_first_round = false;
        }
        let permits = cfg.max_parallel_calls.clamp(1, 16) as usize;
        let budget = BudgetLedger::new(cfg.meeting.max_total_tokens);
        Self {
            budget,
            review_truncated: AtomicBool::new(false),
            llm,
            cfg,
            emitter: Emitter {
                sink,
                session_id,
                seq: Mutex::new(0),
            },
            cancel,
            limiter: Semaphore::new(permits),
            extras,
            usage: Mutex::new(UsageReport::default()),
            activities: Mutex::new(Vec::new()),
            no_tool_models: Mutex::new(HashSet::new()),
            reviewer_notes: Mutex::new(HashMap::new()),
            membership: Mutex::new(Vec::new()),
            extra_cast: Mutex::new(Vec::new()),
            calls: Mutex::new(CallLog::default()),
            progress: Mutex::new(None),
        }
    }

    // ── checkpoint ──

    /// Ghi ảnh chụp tiến độ của `spec.scope` (plan + round đã chốt) qua `extras.checkpoint`.
    fn checkpoint(&self, spec: &LoopSpec<'_>, out: &LoopResult) {
        if self.extras.checkpoint.is_none() {
            return;
        }
        self.checkpoint_with(|rec| {
            let (plans, rounds) = match spec.ctx {
                PlanContext::Main => (&mut rec.plans, &mut rec.rounds),
                PlanContext::Department { dept, task } => {
                    let i = match rec.departments.iter().position(|d| d.id == dept.id) {
                        Some(i) => i,
                        None => {
                            rec.departments.push(DepartmentRecord {
                                id: dept.id.clone(),
                                name: dept.name.clone(),
                                task: task.to_owned(),
                                lead_id: dept.lead_id.clone(),
                                member_ids: dept.member_ids.clone(),
                                max_rounds: dept.max_rounds,
                                status: SessionStatus::Running,
                                plans: vec![],
                                rounds: vec![],
                                final_markdown: None,
                                error: None,
                            });
                            rec.departments.len() - 1
                        }
                    };
                    let d = &mut rec.departments[i];
                    (&mut d.plans, &mut d.rounds)
                }
            };
            sync_tail(plans, &out.plans);
            sync_tail(rounds, &out.rounds);
        });
    }

    /// Cập nhật bản ghi dựng dở bằng `f` + số liệu chung (usage, công cụ, thành viên) rồi gửi bản sao.
    fn checkpoint_with(&self, f: impl FnOnce(&mut SessionRecord)) {
        let Some(sink) = self.extras.checkpoint.as_ref() else { return };
        let snapshot = {
            let mut guard = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            let Some(rec) = guard.as_mut() else { return };
            f(rec);
            rec.status = SessionStatus::Running;
            rec.usage = self.usage.lock().unwrap_or_else(|e| e.into_inner()).clone();
            rec.calls = self.calls.lock().unwrap_or_else(|e| e.into_inner()).calls.clone();
            rec.activities = self.activities.lock().unwrap_or_else(|e| e.into_inner()).clone();
            rec.membership = self.membership.lock().unwrap_or_else(|e| e.into_inner()).clone();
            rec.user_notes = self.extras.control.notes();
            for c in self.extra_cast.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                if !rec.cast.iter().any(|x| x.id == c.id) {
                    rec.cast.push(c.clone());
                }
            }
            rec.clone()
        };
        sink.save(snapshot);
    }

    fn prompt_opts(&self) -> PromptOpts {
        let m = &self.cfg.meeting;
        PromptOpts { depth: m.plan_depth, routing: m.section_routing, strict_first_round: m.strict_first_round }
    }

    /// Ước lượng token cho lần chọn thành phần (dựng đúng prompt mà `staffing::propose` sẽ gửi).
    fn staffing_estimate(&self, request: &str) -> u64 {
        let m = &self.cfg.meeting;
        let Some(coord) = self.cfg.role(&m.chair_id) else { return 0 };
        let Some(ep) = self.cfg.resolve(&coord.id) else { return 0 };
        let msgs = prompts::staffing_messages(coord, &self.cfg, request, m.max_auto_reviewers, m.max_auto_departments, &self.extras.company);
        estimate_call(&msgs, ep.max_tokens)
    }

    /// Ghi token tiết kiệm được (review theo section / giữ nguyên APPROVE).
    fn record_saving(&self, scope: &str, agent: &str, tokens: u64, carried: bool) {
        if tokens == 0 && !carried {
            return;
        }
        let total_saved = {
            let mut r = self.usage.lock().unwrap_or_else(|e| e.into_inner());
            r.saved_tokens += tokens;
            if carried {
                r.carried_reviews += 1;
            }
            r.saved_tokens
        };
        self.emitter.send(CouncilEvent::TokensSaved {
            scope: scope.to_owned(),
            agent: agent.to_owned(),
            tokens,
            carried,
            total_saved,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn change_membership(&self, scope: &str, role: &RoleDef, action: &str, by: &str, reason: &str, round: u32) {
        let change = MembershipChange {
            scope: scope.to_owned(),
            agent: role.id.clone(),
            action: action.to_owned(),
            by: by.to_owned(),
            reason: crate::models::truncate_chars(reason.trim(), 200),
            round,
        };
        log::info!("[{scope}] {} {action} (by {by}): {}", role.name, change.reason);
        let member = (action == "invited").then(|| {
            let m = cast_member(role);
            let mut extra = self.extra_cast.lock().unwrap_or_else(|e| e.into_inner());
            if !extra.iter().any(|c| c.id == m.id) {
                extra.push(m.clone());
            }
            m
        });
        self.membership.lock().unwrap_or_else(|e| e.into_inner()).push(change.clone());
        self.emitter.send(CouncilEvent::MembershipChanged { change, member });
    }

    /// Role có thể được mời thêm vào 1 nơi họp (đã gán model, chưa có mặt, không phải người viết).
    fn available_experts<'a>(&'a self, author: &RoleDef, present: &[&RoleDef]) -> Vec<&'a RoleDef> {
        self.cfg
            .roles
            .iter()
            .filter(|r| r.id != author.id && !present.iter().any(|p| p.id == r.id) && self.cfg.resolve(&r.id).is_some())
            .collect()
    }

    // ── token ──

    /// Ghi token của 1 lần gọi (tổng + từng lần gọi + chi phí); thiếu `usage` → ước lượng theo độ dài prompt/output.
    fn record_usage(&self, m: &CallMeta<'_>, usage: Option<Usage>, finish: &str, prompt_chars: usize, output_chars: usize) {
        let (p, cached, c, estimated) = match usage {
            Some(u) if u.prompt_tokens + u.completion_tokens > 0 => (u.prompt_tokens, u.cached_tokens, u.completion_tokens, false),
            _ => (estimate_tokens(prompt_chars), 0, estimate_tokens(output_chars), true),
        };
        let (scope, agent, kind) = (m.scope, m.agent, m.kind);
        let total = {
            let mut r = self.usage.lock().unwrap_or_else(|e| e.into_inner());
            r.total.add(p, cached, c, estimated);
            r.by_role.entry(agent.to_owned()).or_default().add(p, cached, c, estimated);
            r.by_scope.entry(scope.to_owned()).or_default().add(p, cached, c, estimated);
            r.by_kind.entry(kind.to_owned()).or_default().add(p, cached, c, estimated);
            r.by_model.entry(m.model.to_owned()).or_default().add(p, cached, c, estimated);
            r.total
        };
        let (cost_usd, unpriced_models) = {
            let mut log = self.calls.lock().unwrap_or_else(|e| e.into_inner());
            let call_id = log.next_id;
            log.next_id += 1;
            match self.cfg.pricing.get(m.model) {
                Some(price) => log.cost += price.cost(p, cached, c),
                None => {
                    log.unpriced.insert(m.model.to_owned());
                }
            }
            if log.calls.len() < MAX_CALL_RECORDS {
                log.calls.push(CallUsage {
                    call_id,
                    scope: scope.to_owned(),
                    agent: agent.to_owned(),
                    round: m.round,
                    kind: kind.to_owned(),
                    model: m.model.to_owned(),
                    prompt: p,
                    cached,
                    completion: c,
                    estimated,
                    finish: finish.to_owned(),
                    latency_ms: u64::try_from(m.t0.elapsed().as_millis()).unwrap_or(u64::MAX),
                });
            }
            (log.unpriced.is_empty().then_some(log.cost), log.unpriced.iter().cloned().collect())
        };
        self.emitter.send(CouncilEvent::UsageUpdated {
            scope: scope.to_owned(),
            agent: agent.to_owned(),
            kind: kind.to_owned(),
            model: m.model.to_owned(),
            prompt_tokens: p,
            completion_tokens: c,
            cached_tokens: cached,
            estimated,
            total,
            cost_usd,
            unpriced_models,
        });
    }

    // ── ngân sách ──

    /// Giữ chỗ cho 1 lần gọi. None = hết ngân sách (đã emit `Budget { state: "exhausted" }`).
    fn hold(&self, est: u64) -> Option<Hold<'_>> {
        self.hold_group(&[est]).map(|mut v| v.remove(0))
    }

    /// Giữ chỗ cho cả nhóm lần gọi chạy song song: hoặc tất cả, hoặc không cái nào.
    fn hold_group(&self, ests: &[u64]) -> Option<Vec<Hold<'_>>> {
        match self.budget.reserve_group(ests) {
            Some(ids) => Some(
                ids.into_iter()
                    .zip(ests)
                    .map(|(id, &est)| Hold { council: self, id: Some(id), est })
                    .collect(),
            ),
            None => {
                let need: u64 = ests.iter().sum();
                log::warn!("hết ngân sách token: cần giữ ~{need} token, {:?}", self.budget.snapshot());
                self.emit_budget(Some("exhausted"));
                None
            }
        }
    }

    fn settle_budget(&self, id: ReservationId, usage: Option<Usage>, fallback: u64) {
        self.budget.settle(id, usage, fallback);
        self.emit_budget(None);
    }

    /// Emit ảnh chụp ngân sách (chỉ khi người dùng đặt giới hạn).
    fn emit_budget(&self, state: Option<&str>) {
        let s = self.budget.snapshot();
        let Some(limit) = s.limit else { return };
        self.emitter.send(CouncilEvent::Budget {
            used: s.used,
            reserved: s.reserved,
            limit,
            estimated: s.estimated,
            soft: true,
            state: state.unwrap_or(s.state()).to_owned(),
        });
    }

    /// Ghi token của lần gọi + settle chỗ giữ bằng cùng con số (thiếu usage → ước lượng theo độ dài).
    fn account(
        &self,
        hold: Hold<'_>,
        meta: &CallMeta<'_>,
        usage: Option<Usage>,
        finish: &str,
        prompt_chars: usize,
        output_chars: usize,
    ) {
        self.record_usage(meta, usage, finish, prompt_chars, output_chars);
        let real = usage.filter(|u| u.prompt_tokens + u.completion_tokens > 0);
        hold.settle(real, estimate_tokens(prompt_chars) + estimate_tokens(output_chars));
    }

    fn emit_truncated(&self, scope: &str, agent: &str, call_kind: &str, recovered: bool, reason: &str) {
        log::warn!("[{scope}] {agent}: output {call_kind} bị cắt ({reason}), recovered={recovered}");
        self.emitter.send(CouncilEvent::Truncated {
            scope: scope.to_owned(),
            agent: agent.to_owned(),
            call_kind: call_kind.to_owned(),
            recovered,
            reason: reason.to_owned(),
        });
    }

    // ── bối cảnh ──

    /// Gắn bối cảnh công ty vào system prompt và dữ liệu bổ sung vào đầu user message.
    fn decorate(&self, mut msgs: Vec<ChatMessage>, notes: Option<&str>, overview: bool) -> Vec<ChatMessage> {
        if let Some(sys) = msgs.first_mut().filter(|m| m.role == "system") {
            sys.content.push_str(&prompts::company_block(&self.extras.company));
        }
        let mut prefix = prompts::context_prefix(notes, if overview { self.extras.overview.as_deref() } else { None });
        prefix.push_str(&prompts::user_notes_block(&self.extras.control.notes()));
        if !prefix.is_empty() {
            if let Some(user) = msgs.iter_mut().find(|m| m.role == "user") {
                user.content.insert_str(0, &prefix);
            }
        }
        msgs
    }

    fn may_research(&self, as_author: bool) -> bool {
        !self.extras.toolbox.is_empty()
            && match self.cfg.tool_policy {
                ToolPolicy::Off => false,
                ToolPolicy::Authors => as_author,
                ToolPolicy::All => true,
            }
    }

    /// Vòng nghiên cứu bằng công cụ trước khi viết/review. Trả về ghi chú (None nếu không cần / không hỗ trợ).
    #[allow(clippy::too_many_arguments)]
    async fn research(
        &self,
        scope: &str,
        role: &RoleDef,
        phase: &str,
        step: u32,
        purpose: &str,
        ctx: &PlanContext<'_>,
        request: &str,
        plan: Option<&PlanVersion>,
    ) -> Result<Option<String>, Halt> {
        let Some(ep) = self.cfg.resolve(&role.id) else { return Ok(None) };
        if self.no_tool_models.lock().unwrap_or_else(|e| e.into_inner()).contains(&ep.model) {
            return Ok(None);
        }
        let tb = &self.extras.toolbox;
        let tools = tb.definitions();
        // None = không giới hạn: prompt không nêu con số. Vòng lặp vẫn có chặn an toàn: tối đa
        // `UNLIMITED_TOOL_STEPS_CAP` lượt + giới hạn ngữ cảnh (`MAX_RESEARCH_CONTEXT_CHARS`).
        let max_steps = (self.cfg.max_tool_steps != 0).then(|| self.cfg.max_tool_steps.min(12));
        let step_cap = max_steps.unwrap_or(UNLIMITED_TOOL_STEPS_CAP);
        let notes_cap = if max_steps.is_some() { 6_000 } else { 12_000 };
        let mut msgs: Vec<Value> = prompts::research_messages(
            role,
            purpose,
            ctx,
            request,
            plan,
            self.extras.overview.as_deref(),
            tb.workspace.is_some(),
            tb.web.is_some(),
            max_steps,
            &self.extras.company,
        );
        let chars = |m: &[Value]| m.iter().map(|x| x.to_string().len()).sum::<usize>();

        let _permit = self.permit().await?;
        self.emitter.send(CouncilEvent::AgentThinking {
            scope: scope.to_owned(),
            agent: role.id.clone(),
            round: if phase == "review" { step } else { 0 },
            plan_version: if phase == "author" { step } else { plan.map_or(0, |p| p.version) },
        });

        let mut used = 0u32;
        loop {
            // Hội thoại nghiên cứu quá dài → buộc viết ghi chú ngay (vượt cửa sổ ngữ cảnh = mất trắng phần đã đọc).
            let room = chars(&msgs) < MAX_RESEARCH_CONTEXT_CHARS;
            let allow = room && used < step_cap;
            // Mỗi lượt gọi công cụ giữ chỗ ngân sách riêng; hết ngân sách → bỏ nghiên cứu.
            let Some(hold) = self.hold(estimate_raw_call(&msgs, ep.max_tokens)) else {
                log::warn!("{}: hết ngân sách token → dừng bước nghiên cứu", role.name);
                return Ok(None);
            };
            let meta = CallMeta::new(scope, &role.id, "research", &ep.model, if phase == "review" { step } else { 0 });
            let turn = match self.llm.chat_tools(&ep, &msgs, &tools, allow, &self.cancel).await {
                Ok(t) => t,
                Err(LlmError::Cancelled) => return Err(Halt::Cancelled),
                Err(LlmError::Http { status: 400 | 404 | 422 | 501, body }) if used == 0 => {
                    log::warn!("model {} không hỗ trợ tool calling ({}); bỏ qua nghiên cứu", ep.model, truncate_chars(&body, 120));
                    self.no_tool_models.lock().unwrap_or_else(|e| e.into_inner()).insert(ep.model.clone());
                    self.log_activity(scope, &role.id, phase, step, "tools", "model không hỗ trợ tool calling — bỏ qua bước nghiên cứu", false);
                    return Ok(None);
                }
                Err(e) => {
                    log::warn!("{}: nghiên cứu lỗi ({e}); tiếp tục không có ghi chú", role.name);
                    return Ok(None);
                }
            };
            let out_chars = turn.content.as_deref().map_or(0, str::len)
                + turn.tool_calls.iter().map(|c| c.arguments.len() + c.name.len()).sum::<usize>();
            let finish = if turn.tool_calls.is_empty() { "stop" } else { "tool_calls" };
            self.account(hold, &meta, turn.usage, finish, chars(&msgs), out_chars);

            if turn.tool_calls.is_empty() || !allow {
                let notes = turn.content.unwrap_or_default();
                let notes = notes.trim();
                return Ok((!notes.is_empty() && !notes.starts_with(prompts::NO_RESEARCH)).then(|| truncate_chars(notes, notes_cap)));
            }
            msgs.push(turn.assistant_message.clone());
            for call in &turn.tool_calls {
                used += 1;
                let (result, summary, ok) = if used <= step_cap {
                    tb.execute(&call.name, &call.arguments).await
                } else {
                    ("ERROR: đã hết lượt gọi công cụ, hãy viết ghi chú ngay.".to_owned(), "hết lượt công cụ".to_owned(), false)
                };
                self.log_activity(scope, &role.id, phase, step, &call.name, &summary, ok);
                msgs.push(json!({ "role": "tool", "tool_call_id": call.id, "content": result }));
            }
            if self.cancel.is_cancelled() {
                return Err(Halt::Cancelled);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn log_activity(&self, scope: &str, agent: &str, phase: &str, step: u32, tool: &str, summary: &str, ok: bool) {
        let activity = ToolActivity {
            scope: scope.to_owned(),
            agent: agent.to_owned(),
            phase: phase.to_owned(),
            step,
            tool: tool.to_owned(),
            summary: summary.to_owned(),
            ok,
        };
        self.activities.lock().unwrap_or_else(|e| e.into_inner()).push(activity.clone());
        self.emitter.send(CouncilEvent::ToolUsed { activity });
    }

    fn role(&self, id: &str) -> Result<&RoleDef, Halt> {
        self.cfg
            .role(id)
            .ok_or_else(|| Halt::Failed(format!("role '{id}' không tồn tại trong cấu hình")))
    }

    fn name_of(&self, id: &str) -> String {
        self.cfg.role(id).map(|r| r.name.clone()).unwrap_or_else(|| id.to_owned())
    }

    fn selected_departments(&self) -> Vec<&Department> {
        self.cfg
            .meeting
            .department_ids
            .iter()
            .filter_map(|id| self.cfg.department(id))
            .collect()
    }

    /// Mọi role xuất hiện trong phiên (chủ trì, reviewer, trưởng phòng, thành viên).
    fn cast(&self) -> Vec<CastMember> {
        let m = &self.cfg.meeting;
        let mut ids: Vec<&str> = vec![m.chair_id.as_str()];
        ids.extend(m.reviewer_ids.iter().map(String::as_str));
        for d in self.selected_departments() {
            ids.push(&d.lead_id);
            ids.extend(d.member_ids.iter().map(String::as_str));
        }
        let mut seen = HashSet::new();
        ids.into_iter()
            .filter(|id| seen.insert(*id))
            .filter_map(|id| self.cfg.role(id))
            .map(cast_member)
            .collect()
    }

    async fn permit(&self) -> Result<SemaphorePermit<'_>, LlmError> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(LlmError::Cancelled),
            p = self.limiter.acquire() => p.map_err(|_| LlmError::Cancelled),
        }
    }

    /// Luôn trả về `SessionRecord` (kể cả khi lỗi/huỷ) để caller persist lịch sử.
    pub async fn run(mut self, prompt: String) -> SessionRecord {
        // AI tự chọn thành phần họp (chế độ Tự động) — trước khi công bố thành phần.
        let mut staffing = None;
        if self.extras.auto_staff {
            // Giữ chỗ ngân sách trực tiếp trên sổ (chưa emit gì trước `SessionStarted`); settle sau đó.
            let model = self.cfg.resolve(&self.cfg.meeting.chair_id).map(|e| e.model).unwrap_or_default();
            let t0 = Instant::now();
            match self.budget.reserve(self.staffing_estimate(&prompt)) {
                None => log::warn!("ngân sách token quá nhỏ cho bước chọn thành phần → dùng cấu hình mặc định"),
                Some(rid) => match staffing::propose(&self.llm, &self.cfg, &prompt, &self.extras.company, &self.cancel).await {
                    Ok((info, usage, pc, oc)) => {
                        let m = &mut self.cfg.meeting;
                        m.chair_id = info.chair_id.clone();
                        m.reviewer_ids = info.reviewer_ids.clone();
                        m.department_ids = info.department_ids.clone();
                        staffing = Some((info, usage, pc, oc, rid, model, t0));
                    }
                    Err(_) => self.budget.settle(rid, Some(Usage::default()), 0),
                },
            }
        }

        let m = &self.cfg.meeting;
        let mut record = SessionRecord::new(self.emitter.session_id, prompt.clone(), self.cfg.max_rounds);
        record.chair_id = m.chair_id.clone();
        record.reviewer_ids = m.reviewer_ids.clone();
        record.cast = self.cast();
        record.options = self.extras.info.clone();
        record.parent = self.extras.parent.clone();

        self.emitter.send(CouncilEvent::SessionStarted {
            prompt: prompt.clone(),
            max_rounds: self.cfg.max_rounds,
            chair_id: m.chair_id.clone(),
            reviewer_ids: m.reviewer_ids.clone(),
            cast: record.cast.clone(),
            departments: self
                .selected_departments()
                .into_iter()
                .map(|d| DepartmentBrief {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    lead_id: d.lead_id.clone(),
                    member_ids: d.member_ids.clone(),
                    max_rounds: d.max_rounds,
                })
                .collect(),
            options: self.extras.info.clone(),
        });
        if let Some((info, usage, pc, oc, rid, model, t0)) = staffing {
            let mut meta = CallMeta::new(MAIN_SCOPE, &info.chair_id, "staffing", &model, 0);
            meta.t0 = t0;
            self.record_usage(&meta, usage, "stop", pc, oc);
            let real = usage.filter(|u| u.prompt_tokens + u.completion_tokens > 0);
            if real.is_none() && pc + oc == 0 {
                // Không gọi model (chưa có chủ trì / model) → không tốn token.
                self.settle_budget(rid, Some(Usage::default()), 0);
            } else {
                self.settle_budget(rid, real, estimate_tokens(pc) + estimate_tokens(oc));
            }
            self.emitter.send(CouncilEvent::StaffingDecided { staffing: info.clone() });
            record.staffing = Some(info);
        }
        if self.extras.checkpoint.is_some() {
            // Ghi ngay bản ghi `running` đầu tiên: app tắt trước bản nháp vẫn còn yêu cầu trong lịch sử.
            *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = Some(record.clone());
            self.checkpoint_with(|_| {});
        }

        match self.run_inner(&prompt, &mut record).await {
            Ok(()) => {}
            Err(Halt::Cancelled) => {
                record.status = SessionStatus::Cancelled;
                let round = (record.rounds.len() as u32 + 1).min(self.cfg.max_rounds);
                log::info!("session {} cancelled at round {round}", record.id);
                self.emitter.send(CouncilEvent::Cancelled { round });
            }
            Err(Halt::Failed(error)) => {
                log::error!("session {} failed: {error}", record.id);
                record.status = SessionStatus::Failed;
                record.error = Some(error.clone());
                self.emitter.send(CouncilEvent::Failed { error });
            }
        }
        record.usage = self.usage.lock().unwrap_or_else(|e| e.into_inner()).clone();
        record.calls = std::mem::take(&mut self.calls.lock().unwrap_or_else(|e| e.into_inner()).calls);
        record.activities = std::mem::take(&mut *self.activities.lock().unwrap_or_else(|e| e.into_inner()));
        record.membership = std::mem::take(&mut *self.membership.lock().unwrap_or_else(|e| e.into_inner()));
        record.user_notes = self.extras.control.notes();
        for c in self.extra_cast.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            if !record.cast.iter().any(|x| x.id == c.id) {
                record.cast.push(c);
            }
        }
        record.finished_at = Some(Utc::now());
        log::info!(
            "session {} dùng {} token ({} lần gọi, {} ước lượng)",
            record.id,
            record.usage.total.total(),
            record.usage.total.calls,
            record.usage.total.estimated_calls
        );
        record
    }

    async fn run_inner(&self, prompt: &str, record: &mut SessionRecord) -> Result<(), Halt> {
        // ── Tầng 1: phòng ban ──
        let reports = self.run_departments(prompt, record).await?;
        if !record.departments.is_empty() {
            let depts = record.departments.clone();
            self.checkpoint_with(move |rec| rec.departments = depts);
        }

        // ── Tầng 2: cuộc họp chính ──
        let m = &self.cfg.meeting;
        let chair = self.role(&m.chair_id)?;
        let reviewers = m
            .reviewer_ids
            .iter()
            .map(|id| self.role(id))
            .collect::<Result<Vec<_>, _>>()?;
        let spec = LoopSpec {
            scope: MAIN_SCOPE,
            ctx: PlanContext::Main,
            author: chair,
            reviewers,
            max_rounds: self.cfg.max_rounds.max(1),
            reports,
        };
        let mut res = LoopResult::default();
        let outcome = self.run_loop(&spec, prompt, &mut res).await;
        record.plans = res.plans;
        record.rounds = res.rounds;
        outcome?;

        let status = self.aggregate_status(res.status.unwrap_or(SessionStatus::MaxRoundsReached), &record.departments);
        let final_md = res.final_markdown.unwrap_or_default();
        record.status = status;
        record.final_markdown = Some(final_md.clone());
        self.emitter.send(CouncilEvent::Finished {
            status,
            final_version: record.plans.last().map_or(0, |p| p.version),
            final_markdown: final_md,
        });
        Ok(())
    }

    /// Trạng thái cuối của phiên = kết quả tệ nhất giữa cuộc họp chính và các phòng ban dừng sớm
    /// (`Truncated` / `BudgetExhausted`). Phòng ban lỗi / hết vòng không hạ trạng thái phiên
    /// (báo cáo của họ vẫn vào cuộc họp chính như trước). Có reviewer bị cắt → không bao giờ `Converged`.
    fn aggregate_status(&self, main: SessionStatus, depts: &[DepartmentRecord]) -> SessionStatus {
        let mut status = depts
            .iter()
            .map(|d| d.status)
            .filter(|s| matches!(s, SessionStatus::Truncated | SessionStatus::BudgetExhausted))
            .fold(main, SessionStatus::worst);
        if status == SessionStatus::Converged && self.review_truncated.load(AtomicOrdering::SeqCst) {
            status = SessionStatus::PartiallyApproved;
        }
        status
    }

    /// Giao việc + cho các phòng ban họp song song. Trả về `(tên phòng ban, final plan)` cho chủ trì.
    async fn run_departments(
        &self,
        prompt: &str,
        record: &mut SessionRecord,
    ) -> Result<Vec<(String, String)>, Halt> {
        let depts = self.selected_departments();
        if depts.is_empty() {
            return Ok(vec![]);
        }
        let chair = self.role(&self.cfg.meeting.chair_id)?;
        let tasks = self.assign_tasks(chair, prompt, &depts).await?;
        self.emitter.send(CouncilEvent::DepartmentsAssigned {
            assignments: depts
                .iter()
                .map(|d| Assignment {
                    department_id: d.id.clone(),
                    task: tasks[&d.id].clone(),
                })
                .collect(),
        });

        let runs = depts.iter().map(|&d| {
            let task = tasks[&d.id].as_str();
            async move {
                let mut res = LoopResult::default();
                let outcome = async {
                    let lead = self.role(&d.lead_id)?;
                    let members = d
                        .member_ids
                        .iter()
                        .map(|id| self.role(id))
                        .collect::<Result<Vec<_>, _>>()?;
                    let spec = LoopSpec {
                        scope: &d.id,
                        ctx: PlanContext::Department { dept: d, task },
                        author: lead,
                        reviewers: members,
                        max_rounds: d.max_rounds.clamp(1, 5),
                        reports: vec![],
                    };
                    self.run_loop(&spec, prompt, &mut res).await
                }
                .await;
                (d, task.to_owned(), res, outcome)
            }
        });
        let results = join_all(runs).await;

        let mut reports = Vec::new();
        let mut cancelled = false;
        for (d, task, res, outcome) in results {
            let (status, error) = match outcome {
                Ok(()) => (res.status.unwrap_or(SessionStatus::MaxRoundsReached), None),
                Err(Halt::Cancelled) => {
                    cancelled = true;
                    (SessionStatus::Cancelled, None)
                }
                Err(Halt::Failed(e)) => {
                    log::warn!("phòng ban {} lỗi: {e}", d.name);
                    (SessionStatus::Failed, Some(e))
                }
            };
            if let Some(md) = &res.final_markdown {
                reports.push((d.name.clone(), md.clone()));
            }
            if !cancelled {
                self.emitter.send(CouncilEvent::DepartmentFinished {
                    department_id: d.id.clone(),
                    status,
                    final_markdown: res.final_markdown.clone(),
                    error: error.clone(),
                });
            }
            record.departments.push(DepartmentRecord {
                id: d.id.clone(),
                name: d.name.clone(),
                task,
                lead_id: d.lead_id.clone(),
                member_ids: d.member_ids.clone(),
                max_rounds: d.max_rounds,
                status,
                plans: res.plans,
                rounds: res.rounds,
                final_markdown: res.final_markdown,
                error,
            });
        }
        if cancelled || self.cancel.is_cancelled() {
            return Err(Halt::Cancelled);
        }
        Ok(reports)
    }

    /// Chủ trì chia việc. Lỗi/thiếu → phòng ban nhận nguyên yêu cầu (không làm hỏng phiên).
    async fn assign_tasks(
        &self,
        chair: &RoleDef,
        prompt: &str,
        depts: &[&Department],
    ) -> Result<HashMap<String, String>, Halt> {
        let fallback = |d: &Department| {
            format!(
                "Analyse the request from the perspective of the {} department (mission: {}) and propose the department's plan.",
                d.name.trim(),
                d.mission.trim()
            )
        };
        let mut tasks: HashMap<String, String> = HashMap::new();

        let ready = self.cfg.resolve(&chair.id).and_then(|ep| {
            let msgs = self.decorate(prompts::assignment_messages(chair, prompt, depts, self.cfg.meeting.plan_depth), None, false);
            match self.hold(estimate_call(&msgs, ep.max_tokens)) {
                Some(hold) => Some((ep, msgs, hold)),
                None => {
                    log::warn!("hết ngân sách token → bỏ bước giao việc, phòng ban nhận nguyên yêu cầu");
                    None
                }
            }
        });
        if let Some((ep, msgs, hold)) = ready {
            self.emitter.send(CouncilEvent::AgentThinking {
                scope: MAIN_SCOPE.into(),
                agent: chair.id.clone(),
                round: 0,
                plan_version: 0,
            });
            let meta = CallMeta::new(MAIN_SCOPE, &chair.id, "assign", &ep.model, 0);
            let result = {
                let _permit = self.permit().await?;
                self.llm.chat_json::<RawAssignments>(&ep, &msgs, &self.cancel).await
            };
            match result {
                Ok((raw, text, usage)) => {
                    self.account(hold, &meta, usage, "stop", msg_chars(&msgs), text.len());
                    for a in raw.assignments {
                        let task = a.task.trim();
                        if !task.is_empty() && depts.iter().any(|d| d.id == a.department_id) {
                            tasks.insert(a.department_id, truncate_chars(task, 2_000));
                        }
                    }
                }
                Err(LlmError::Cancelled) => return Err(Halt::Cancelled),
                Err(e) => {
                    if matches!(e, LlmError::Truncated { .. }) {
                        hold.settle_truncated();
                    }
                    log::warn!("chủ trì giao việc lỗi ({e}); phòng ban nhận nguyên yêu cầu");
                }
            }
        }
        for d in depts {
            tasks.entry(d.id.clone()).or_insert_with(|| fallback(d));
        }
        Ok(tasks)
    }

    /// Maker–checker loop cho 1 scope. Ghi tiến độ vào `out` kể cả khi lỗi giữa chừng.
    ///
    /// Thành viên thay đổi được giữa các vòng: người dùng mời / mời ra, reviewer đề nghị mời
    /// chuyên gia còn thiếu (giới hạn `max_invites`), reviewer tự rút (ABSTAIN).
    /// Review theo section: reviewer chỉ nhận phần được gắn tag; vòng sau nếu phần của họ không
    /// đổi và lần trước đã APPROVE → giữ nguyên, không gọi model.
    async fn run_loop(&self, spec: &LoopSpec<'_>, request: &str, out: &mut LoopResult) -> Result<(), Halt> {
        let opts = self.prompt_opts();
        let mut reviewers: Vec<&RoleDef> = spec.reviewers.clone();
        let mut invites_left = if self.cfg.meeting.dynamic_membership { self.cfg.meeting.max_invites } else { 0 };
        let mut invited: HashSet<String> = HashSet::new();
        let mut reviewed_once: HashSet<String> = HashSet::new();

        // Phiên chạy lại: cuộc họp chính bắt đầu từ plan đã có (bỏ nghiên cứu + bản nháp).
        let seed = if spec.scope == MAIN_SCOPE { self.extras.seed.as_ref() } else { None };
        // Người viết plan nghiên cứu (đọc workspace / tìm web) 1 lần trước bản nháp; ghi chú dùng lại khi chỉnh sửa.
        let notes = if seed.is_none() && self.may_research(true) {
            let purpose = match spec.ctx {
                PlanContext::Main => "write the plan for the request",
                PlanContext::Department { .. } => "write your department's plan for the assigned task",
            };
            self.research(spec.scope, spec.author, "author", 1, purpose, &spec.ctx, request, None).await?
        } else {
            None
        };
        let v1 = if let Some(seed) = seed {
            self.emitter.send(CouncilEvent::PlanReady {
                scope: spec.scope.to_owned(),
                version: seed.version,
                markdown: seed.markdown.clone(),
            });
            PlanVersion::new(seed.version, seed.markdown.clone(), ReviseKind::Initial)
        } else {
            let draft = self.decorate(
                prompts::author_draft_messages(spec.author, &spec.ctx, request, &spec.reports, &reviewers, &opts),
                notes.as_deref(),
                true,
            );
            match self.call_author(spec.scope, spec.author, 1, 0, &draft, ReviseKind::Initial).await? {
                Authored::Plan(p) => p,
                Authored::Stopped(status) => {
                    self.finish_loop(out, status);
                    return Ok(());
                }
            }
        };
        if opts.routing && !reviewers.is_empty() && !routing::is_tagged(&v1.markdown) {
            log::info!("[{}] {} không gắn tag reviewer cho section → reviewer đọc toàn bộ plan", spec.scope, spec.author.name);
        }
        // Section của phiên bản hiện tại, id ổn định giữa các phiên bản (dùng cho section revise).
        let mut cur_sections = sections::split(&v1.markdown);
        out.plans.push(v1);
        self.checkpoint(spec, out);

        if reviewers.is_empty() {
            // Phòng ban không có thành viên phản biện → plan của trưởng phòng là kết quả.
            out.status = Some(SessionStatus::Converged);
            out.final_markdown = out.plans.last().map(|p| p.markdown.clone());
            return Ok(());
        }

        let mut previous: HashMap<String, Review> = HashMap::new();
        for r in seed.map(|s| s.reviews.as_slice()).unwrap_or(&[]) {
            // Issue còn mở của plan xuất phát → reviewer kiểm chứng như vòng tiếp theo.
            reviewed_once.insert(r.agent.clone());
            previous.insert(r.agent.clone(), r.clone());
        }
        let mut last_fp: HashMap<String, u64> = HashMap::new();
        // (role_id, lý do, người đề nghị) — áp dụng ở đầu vòng kế tiếp
        let mut proposed: Vec<(String, String, String)> = Vec::new();
        // Có reviewer bị cắt output trong vòng họp này → không kết thúc `Converged`.
        let mut loop_truncated = false;
        // Section revise: chế độ, số lần fallback liên tiếp, đã có vòng dùng section mode chưa.
        let revise_mode = self.cfg.meeting.revise_mode;
        let mut consecutive_fallbacks = 0u32;
        let mut force_full = false;
        let mut used_section = false;
        // Mâu thuẫn từ bước kiểm tra nhất quán → vòng chỉnh sửa kế tiếp viết lại toàn bộ để sửa.
        let mut pending_conflicts: Option<Vec<ConsistencyConflict>> = None;

        for round in 1..=spec.max_rounds {
            // ── 1. Thay đổi thành viên ──
            for action in self.extras.control.take(spec.scope) {
                match action {
                    ControlAction::Invite { role_id, .. } => proposed.insert(0, (role_id, "Người dùng mời vào cuộc họp".into(), "user".into())),
                    ControlAction::Dismiss { role_id, .. } => {
                        if let Some(i) = reviewers.iter().position(|r| r.id == role_id) {
                            let r = reviewers.remove(i);
                            self.change_membership(spec.scope, r, "dismissed", "user", "Người dùng mời ra khỏi cuộc họp", round);
                        }
                    }
                }
            }
            for (role_id, reason, by) in proposed.drain(..) {
                let by_user = by == "user";
                if (!by_user && invites_left == 0)
                    || role_id == spec.author.id
                    || reviewers.iter().any(|r| r.id == role_id)
                    || reviewers.len() >= crate::config::MAX_REVIEWERS
                    || self.cfg.resolve(&role_id).is_none()
                {
                    continue;
                }
                let Some(role) = self.cfg.role(&role_id) else { continue };
                if !by_user {
                    invites_left -= 1;
                }
                reviewers.push(role);
                invited.insert(role_id.clone());
                self.change_membership(spec.scope, role, "invited", &by, &reason, round);
            }
            if reviewers.is_empty() {
                // Mọi reviewer đã rời cuộc họp → không còn ai phản đối.
                let status = if loop_truncated { SessionStatus::PartiallyApproved } else { SessionStatus::Converged };
                self.finish_loop(out, status);
                return Ok(());
            }

            // ── 2. Review (song song; giữ nguyên APPROVE khi phần phụ trách không đổi) ──
            let plan = out.plans.last().cloned().expect("có ít nhất 1 plan");
            let available = if invites_left > 0 { self.available_experts(spec.author, &reviewers) } else { vec![] };
            // 2a. Ai giữ nguyên APPROVE (không gọi model), ai cần review.
            let fps: Vec<u64> = reviewers.iter().map(|r| routing::fingerprint(&plan.markdown, &r.id)).collect();
            let carried_reviews: Vec<Option<Review>> = reviewers
                .iter()
                .zip(&fps)
                .map(|(r, fp)| {
                    let prev = previous.get(&r.id).filter(|p| p.verdict == Verdict::Approve);
                    prev.filter(|_| opts.routing && last_fp.get(&r.id) == Some(fp)).map(carried)
                })
                .collect();
            let jobs: Vec<&RoleDef> = reviewers
                .iter()
                .zip(&carried_reviews)
                .filter(|(_, c)| c.is_none())
                .map(|(r, _)| *r)
                .collect();
            // 2b. Reviewer tự kiểm chứng (chính sách "Tất cả", 1 lần ở round 1) — trước khi dựng prompt.
            if round == 1 && self.may_research(false) {
                join_all(jobs.iter().map(|r| self.reviewer_research(spec, r, request, &plan))).await;
                if self.cancel.is_cancelled() {
                    return Err(Halt::Cancelled);
                }
            }
            // 2c. Dựng sẵn mọi prompt rồi giữ chỗ ngân sách cho cả nhóm: tất cả hoặc không ai.
            let prepared: Vec<Result<PreparedReview, LlmError>> = jobs
                .iter()
                .map(|r| {
                    let rc = prompts::ReviewCtx {
                        round,
                        first_review: !reviewed_once.contains(&r.id),
                        invited: invited.contains(&r.id),
                        routed: false,
                        final_round: round > 1 && round == spec.max_rounds,
                        available: &available,
                    };
                    self.prepare_review(spec, r, rc, request, &plan, previous.get(&r.id))
                })
                .collect();
            let ests: Vec<u64> = prepared
                .iter()
                .filter_map(|p| p.as_ref().ok())
                .map(|p| estimate_call(&p.messages, p.ep.max_tokens))
                .collect();
            let Some(holds) = self.hold_group(&ests) else {
                log::warn!("[{}] hết ngân sách token trước round {round} → dừng, giữ plan v{}", spec.scope, plan.version);
                self.finish_loop(out, SessionStatus::BudgetExhausted);
                return Ok(());
            };
            // 2d. Gọi song song (mỗi prompt hợp lệ đã có đúng 1 chỗ giữ).
            let mut holds = holds.into_iter();
            let work: Vec<(&RoleDef, ReviewWork<'_>)> = jobs
                .iter()
                .copied()
                .zip(prepared)
                .map(|(r, p)| (r, p.map(|p| (p, holds.next().expect("mỗi prompt hợp lệ có 1 chỗ giữ")))))
                .collect();
            let plan_version = plan.version;
            let mut called: HashMap<String, Result<Review, LlmError>> = join_all(work.into_iter().map(|(r, w)| async move {
                let res = match w {
                    Ok((p, hold)) => self.exec_review(spec, r, round, plan_version, p, hold).await,
                    Err(e) => Err(e),
                };
                (r.id.clone(), res)
            }))
            .await
            .into_iter()
            .collect();
            if self.cancel.is_cancelled() {
                return Err(Halt::Cancelled);
            }
            let results: Vec<Result<(Review, u64, bool), LlmError>> = reviewers
                .iter()
                .zip(fps)
                .zip(carried_reviews)
                .map(|((r, fp), c)| match c {
                    Some(rv) => Ok((rv, fp, true)),
                    None => called.remove(&r.id).expect("reviewer cần review đã được gọi").map(|rv| (rv, fp, false)),
                })
                .collect();

            // ── 3. Tổng hợp ──
            let mut slots = Vec::with_capacity(reviewers.len());
            let mut leaving: Vec<String> = Vec::new();
            for (r, res) in reviewers.iter().zip(results) {
                match res {
                    Ok((review, fp, was_carried)) => {
                        reviewed_once.insert(r.id.clone());
                        last_fp.insert(r.id.clone(), fp);
                        if was_carried {
                            // Không gọi model: tiết kiệm ≈ cả prompt review (plan + hướng dẫn).
                            self.record_saving(spec.scope, &r.id, estimate_tokens(plan.markdown.len() + 4_000), true);
                            self.emitter.send(CouncilEvent::ReviewReady { scope: spec.scope.to_owned(), round, review: review.clone() });
                        }
                        if review.verdict == Verdict::Abstain {
                            leaving.push(r.id.clone());
                            self.change_membership(spec.scope, r, "abstained", "self", &review.summary_bubble, round);
                        }
                        if self.cfg.meeting.dynamic_membership {
                            for inv in &review.invites {
                                proposed.push((inv.role_id.clone(), inv.reason.clone(), r.id.clone()));
                            }
                        }
                        slots.push(ReviewSlot { agent: r.id.clone(), review: Some(review), error: None });
                    }
                    Err(e) => {
                        if let LlmError::Truncated { reason } = &e {
                            // Review JSON bị cắt kể cả sau lần nới max_tokens → slot lỗi như mọi lỗi khác,
                            // nhưng phiên không còn được tính là hội tụ.
                            loop_truncated = true;
                            self.review_truncated.store(true, AtomicOrdering::SeqCst);
                            self.emit_truncated(spec.scope, &r.id, "review", false, reason);
                        }
                        let error = e.to_string();
                        self.emitter.send(CouncilEvent::ReviewFailed {
                            scope: spec.scope.to_owned(),
                            round,
                            agent: r.id.clone(),
                            error: error.clone(),
                        });
                        slots.push(ReviewSlot { agent: r.id.clone(), review: None, error: Some(error) });
                    }
                }
            }
            reviewers.retain(|r| !leaving.contains(&r.id));

            let ok: Vec<&Review> = slots.iter().filter_map(|s| s.review.as_ref()).collect();
            if ok.is_empty() {
                let errs: Vec<String> = slots
                    .iter()
                    .filter_map(|s| s.error.as_ref().map(|e| format!("{}: {e}", self.name_of(&s.agent))))
                    .collect();
                let n = slots.len();
                out.rounds.push(RoundRecord { round, plan_version: plan.version, slots, approvals: 0, converged: false });
                self.checkpoint(spec, out);
                return Err(Halt::Failed(format!("Toàn bộ {n} reviewer đều lỗi ở round {round} — {}", errs.join(" | "))));
            }
            let voting: Vec<&&Review> = ok.iter().filter(|r| r.verdict != Verdict::Abstain).collect();
            let failed = slots.len() - ok.len();
            let approvals = voting.iter().filter(|r| r.verdict == Verdict::Approve).count() as u32;
            let has_reject = voting.iter().any(|r| r.verdict == Verdict::Reject);
            let converged = failed == 0 && !has_reject;
            let total = (voting.len() + failed) as u32;

            for r in &voting {
                previous.insert(r.agent.clone(), (**r).clone());
            }
            self.emitter.send(CouncilEvent::RoundCompleted {
                scope: spec.scope.to_owned(),
                round,
                plan_version: plan.version,
                approvals,
                total,
                converged,
            });
            out.rounds.push(RoundRecord { round, plan_version: plan.version, slots, approvals, converged });
            self.checkpoint(spec, out);

            let status = if converged {
                // Từng có reviewer bị cắt output → không khẳng định hội tụ.
                let done = if loop_truncated { SessionStatus::PartiallyApproved } else { SessionStatus::Converged };
                if used_section {
                    // Plan được ghép từ các section sửa riêng → kiểm tra nhất quán trước khi kết luận hội tụ.
                    let rounds_left = round < spec.max_rounds;
                    let mut gate = self.consistency_gate(spec, round, &plan, &cur_sections).await?;
                    if matches!(gate, Gate::Unverified) && rounds_left {
                        gate = self.consistency_gate(spec, round, &plan, &cur_sections).await?;
                    }
                    match gate {
                        Gate::Ok => Some(done),
                        Gate::Conflicts(c) if rounds_left => {
                            pending_conflicts = Some(c);
                            None
                        }
                        Gate::Conflicts(c) => {
                            out.conflicts = c;
                            Some(SessionStatus::Unresolved)
                        }
                        Gate::Unverified => Some(SessionStatus::Unresolved),
                    }
                } else {
                    Some(done)
                }
            } else if !has_reject {
                // Reviewer còn sống đều APPROVE, reviewer lỗi không có feedback → refine vô nghĩa.
                Some(SessionStatus::PartiallyApproved)
            } else if round == spec.max_rounds {
                Some(SessionStatus::MaxRoundsReached)
            } else {
                None
            };
            if let Some(status) = status {
                self.finish_loop(out, status);
                return Ok(());
            }

            // ── 4. Chỉnh sửa plan (vN → vN+1) ──
            let last_round = out.rounds.last().cloned().expect("vừa push");
            let conflicts = pending_conflicts.take();
            let mut fallback: Option<FallbackReason> = None;
            let mut merged: Option<(PlanVersion, Vec<Section>)> = None;
            // Section revise từ vòng 2 (v1 chưa có Changelog), trừ vòng sửa mâu thuẫn.
            if revise_mode != ReviseMode::Full && !force_full && round >= 2 && conflicts.is_none() {
                match self
                    .section_revise(spec, request, notes.as_deref(), &plan, &cur_sections, &last_round, &reviewers, revise_mode)
                    .await?
                {
                    SectionRevise::Merged(p, secs) => merged = Some((p, secs)),
                    SectionRevise::Fallback(r) => fallback = Some(r),
                    SectionRevise::Stopped(status) => {
                        self.finish_loop(out, status);
                        return Ok(());
                    }
                }
            }
            if let Some(reason) = fallback {
                consecutive_fallbacks += 1;
                log::info!(
                    "[{}] section revise không dùng được ({}) → viết lại toàn bộ (liên tiếp {consecutive_fallbacks})",
                    spec.scope,
                    fallback_name(reason)
                );
                self.emitter.send(CouncilEvent::ReviseFallback {
                    scope: spec.scope.to_owned(),
                    round,
                    reason,
                    consecutive: consecutive_fallbacks,
                });
                if consecutive_fallbacks >= 2 {
                    // 2 lần liên tiếp → phần còn lại của vòng họp luôn viết lại toàn bộ.
                    force_full = true;
                }
            }
            let (next, next_sections) = match merged {
                Some(m) => {
                    consecutive_fallbacks = 0;
                    used_section = true;
                    m
                }
                None => {
                    let base = match &conflicts {
                        Some(c) => prompts::author_conflict_refine_messages(
                            spec.author, &spec.ctx, request, &plan, round, c, &reviewers, &opts,
                        ),
                        None => prompts::author_refine_messages(spec.author, &spec.ctx, request, &plan, &last_round, &reviewers, &opts),
                    };
                    let msgs = self.decorate(base, notes.as_deref(), false);
                    // `call_author` lo phần cắt cụt / ngân sách và trả `Authored::Stopped` khi phải dừng.
                    match self.call_author(spec.scope, spec.author, plan.version + 1, round, &msgs, ReviseKind::Full).await? {
                        Authored::Plan(mut p) => {
                            p.fallback_reason = fallback.map(fallback_name);
                            let secs = sections::inherit_ids(&cur_sections, sections::split(&p.markdown));
                            (p, secs)
                        }
                        Authored::Stopped(status) => {
                            self.finish_loop(out, status);
                            return Ok(());
                        }
                    }
                }
            };
            cur_sections = next_sections;
            out.plans.push(next);
            self.checkpoint(spec, out);
        }
        self.finish_loop(out, SessionStatus::MaxRoundsReached);
        Ok(())
    }

    /// Chốt kết quả 1 vòng họp. Final = phiên bản plan gần nhất (+ phụ lục khi chưa hội tụ / dừng sớm).
    /// Chưa có phiên bản nào (vd. v1 bị cắt / hết ngân sách) → `final_markdown = None`.
    fn finish_loop(&self, out: &mut LoopResult, status: SessionStatus) {
        out.status = Some(status);
        let Some(last) = out.plans.last() else {
            out.final_markdown = None;
            return;
        };
        let mut final_md = last.markdown.clone();
        let open_issues = match status {
            SessionStatus::MaxRoundsReached | SessionStatus::PartiallyApproved => out.rounds.last(),
            SessionStatus::Unresolved => {
                final_md.push_str(&prompts::stop_appendix(status, last.version));
                final_md.push_str(&prompts::conflicts_appendix(&out.conflicts));
                None
            }
            SessionStatus::Truncated | SessionStatus::BudgetExhausted => {
                final_md.push_str(&prompts::stop_appendix(status, last.version));
                // Chỉ liệt kê issue nếu round cuối review đúng phiên bản được giữ làm Final.
                out.rounds.last().filter(|r| r.plan_version == last.version)
            }
            _ => None,
        };
        if let Some(r) = open_issues {
            final_md.push_str(&prompts::open_issues_appendix(r, |id| self.name_of(id)));
        }
        out.final_markdown = Some(final_md);
    }

    async fn call_author(
        &self,
        scope: &str,
        author: &RoleDef,
        version: u32,
        round: u32,
        messages: &[ChatMessage],
        kind: ReviseKind,
    ) -> Result<Authored, Halt> {
        let ep = self.cfg.resolve(&author.id).ok_or_else(|| {
            Halt::Failed(format!("role '{}' không tồn tại", author.id))
        })?;
        let Some(hold) = self.hold(estimate_call(messages, ep.max_tokens)) else {
            log::warn!("[{scope}] hết ngân sách token trước plan v{version} → dừng");
            return Ok(Authored::Stopped(SessionStatus::BudgetExhausted));
        };
        let _permit = self.permit().await?;
        self.emitter.send(CouncilEvent::AgentThinking {
            scope: scope.to_owned(),
            agent: author.id.clone(),
            round: version,
            plan_version: version,
        });

        let meta = CallMeta::new(scope, &author.id, "plan", &ep.model, round);
        let raw = self.author_completion(&ep, scope, version, messages, true).await?;
        self.account(hold, &meta, raw.usage, finish_name(raw.finish), msg_chars(messages), raw.text.len());

        let markdown = match raw.finish {
            Finish::Stop => unwrap_markdown_fence(&raw.text),
            Finish::Error => {
                // Stream đứt giữa chừng: không biết thiếu bao nhiêu → không nối tiếp, không tạo phiên bản.
                self.emit_truncated(scope, &author.id, "plan", false, "stream_error");
                return Ok(Authored::Stopped(SessionStatus::Truncated));
            }
            Finish::Length => {
                let reason = if ep.max_tokens.unwrap_or(DEFAULT_OUTPUT_TOKENS) >= OUTPUT_TOKENS_CAP {
                    "max_tokens_cap"
                } else {
                    "length"
                };
                match self.continue_plan(&ep, scope, author, version, round, messages, &raw.text, reason).await? {
                    Ok(md) => {
                        self.emit_truncated(scope, &author.id, "plan", true, reason);
                        // UI đã nhận phần đuôi bị cắt qua stream → xoá và gửi lại toàn văn bản ghép.
                        self.emitter.send(CouncilEvent::PlanStreamReset { scope: scope.to_owned(), version });
                        self.emitter.send(CouncilEvent::PlanDelta { scope: scope.to_owned(), version, delta: md.clone() });
                        md
                    }
                    Err(why) => {
                        self.emit_truncated(scope, &author.id, "plan", false, why);
                        return Ok(Authored::Stopped(SessionStatus::Truncated));
                    }
                }
            }
        };
        if markdown.trim().is_empty() {
            return Err(Halt::Failed(format!(
                "{} trả về plan v{version} rỗng",
                author.name
            )));
        }
        self.emitter.send(CouncilEvent::PlanReady {
            scope: scope.to_owned(),
            version,
            markdown: markdown.clone(),
        });
        Ok(Authored::Plan(PlanVersion::new(version, markdown, kind)))
    }

    /// Section revise: chỉ gửi mục lục + các section cần sửa, nhận patch, kiểm tra rồi ghép thành plan đầy đủ.
    /// Không nối tiếp khi bị cắt: output không kết thúc bình thường → fallback `truncated`.
    #[allow(clippy::too_many_arguments)]
    async fn section_revise(
        &self,
        spec: &LoopSpec<'_>,
        request: &str,
        notes: Option<&str>,
        plan: &PlanVersion,
        secs: &[Section],
        round: &RoundRecord,
        reviewers: &[&RoleDef],
        mode: ReviseMode,
    ) -> Result<SectionRevise, Halt> {
        // Số mục reviewer REJECT nhắc tới: field `sections` + số mục trong critical issues.
        let mut numbers: Vec<String> = Vec::new();
        for r in round.slots.iter().filter_map(|s| s.review.as_ref()).filter(|r| r.verdict == Verdict::Reject) {
            numbers.extend(r.sections.iter().cloned());
            for issue in &r.critical_issues {
                numbers.extend(routing::mentioned_numbers(issue));
            }
        }
        let required = cb::required_set(secs, &numbers);
        let names_section = secs.iter().any(|s| required.contains(&s.id) && !sections::is_changelog(s));
        if !names_section {
            // Reviewer REJECT mà không chỉ ra section nào → không biết sửa đâu, viết lại toàn bộ.
            return Ok(SectionRevise::Fallback(FallbackReason::Ratio));
        }
        if mode == ReviseMode::Auto && !cb::section_mode_ok(secs, &required, self.cfg.meeting.section_mode_max_ratio) {
            return Ok(SectionRevise::Fallback(FallbackReason::Ratio));
        }

        let opts = self.prompt_opts();
        let msgs = self.decorate(
            prompts::author_section_refine_messages(spec.author, &spec.ctx, request, plan, secs, &required, round, reviewers, &opts),
            notes,
            false,
        );
        let version = plan.version + 1;
        let author = spec.author;
        let ep = self
            .cfg
            .resolve(&author.id)
            .ok_or_else(|| Halt::Failed(format!("role '{}' không tồn tại", author.id)))?;
        let Some(hold) = self.hold(estimate_call(&msgs, ep.max_tokens)) else {
            log::warn!("[{}] hết ngân sách token trước plan v{version} (section revise) → dừng", spec.scope);
            return Ok(SectionRevise::Stopped(SessionStatus::BudgetExhausted));
        };
        let meta = CallMeta::new(spec.scope, &author.id, "plan", &ep.model, round.round);
        let raw = {
            let _permit = self.permit().await?;
            self.emitter.send(CouncilEvent::AgentThinking {
                scope: spec.scope.to_owned(),
                agent: author.id.clone(),
                round: version,
                plan_version: version,
            });
            // Không chuyển delta lên UI: output là patch, UI chỉ nhận plan đã ghép.
            self.author_completion(&ep, spec.scope, version, &msgs, false).await?
        };
        self.account(hold, &meta, raw.usage, finish_name(raw.finish), msg_chars(&msgs), raw.text.len());
        if raw.finish != Finish::Stop {
            return Ok(SectionRevise::Fallback(FallbackReason::Truncated));
        }
        let text = unwrap_markdown_fence(&raw.text);
        let merged = match cb::parse_patch(&text).and_then(|p| cb::validate_and_merge(secs, &required, &p, plan.version)) {
            Ok(m) => m,
            Err(reason) => return Ok(SectionRevise::Fallback(reason)),
        };
        let markdown = sections::join(&merged);
        self.emitter.send(CouncilEvent::PlanStreamReset { scope: spec.scope.to_owned(), version });
        self.emitter.send(CouncilEvent::PlanDelta { scope: spec.scope.to_owned(), version, delta: markdown.clone() });
        self.emitter.send(CouncilEvent::PlanReady { scope: spec.scope.to_owned(), version, markdown: markdown.clone() });
        Ok(SectionRevise::Merged(PlanVersion::new(version, markdown, ReviseKind::Section), merged))
    }

    /// Kiểm tra nhất quán (chỉ đọc) plan đã ghép. Sai định dạng → hỏi lại 1 lần; bị cắt / hết ngân sách /
    /// lỗi → `Unverified` (không bao giờ coi là hội tụ). Mỗi lần gọi giữ chỗ ngân sách riêng.
    async fn consistency_gate(&self, spec: &LoopSpec<'_>, round: u32, plan: &PlanVersion, secs: &[Section]) -> Result<Gate, Halt> {
        let wanted = &self.cfg.meeting.consistency_checker;
        let checker = if wanted == CONSISTENCY_CHECKER_AUTHOR {
            spec.author
        } else {
            match self.cfg.role(wanted).filter(|r| self.cfg.resolve(&r.id).is_some()) {
                Some(r) => r,
                None => {
                    log::warn!("[{}] người kiểm tra nhất quán '{wanted}' chưa sẵn sàng → dùng người viết plan", spec.scope);
                    spec.author
                }
            }
        };
        let mut gate = Gate::Unverified;
        if let Some(mut ep) = self.cfg.resolve(&checker.id) {
            ep.max_tokens = Some(prompts::CONSISTENCY_MAX_TOKENS);
            let known: Vec<&str> = secs.iter().map(|s| s.id.as_str()).collect();
            let mut msgs = prompts::consistency_messages(plan.version, secs);
            for attempt in 1..=2 {
                let Some(hold) = self.hold(estimate_call(&msgs, ep.max_tokens)) else {
                    self.emit_truncated(spec.scope, &checker.id, "consistency", false, "budget");
                    break;
                };
                let meta = CallMeta::new(spec.scope, &checker.id, "consistency", &ep.model, round);
                let res = {
                    let _permit = self.permit().await?;
                    self.emitter.send(CouncilEvent::AgentThinking {
                        scope: spec.scope.to_owned(),
                        agent: checker.id.clone(),
                        round,
                        plan_version: plan.version,
                    });
                    self.llm.chat(&ep, &msgs, Expect::Text, &self.cancel).await
                };
                let c = match res {
                    Ok(c) => c,
                    Err(LlmError::Cancelled) => return Err(Halt::Cancelled),
                    Err(LlmError::Truncated { reason }) => {
                        hold.settle_truncated();
                        self.emit_truncated(spec.scope, &checker.id, "consistency", false, reason);
                        break;
                    }
                    Err(e) => {
                        log::warn!("[{}] kiểm tra nhất quán lỗi ({e}) → chưa xác minh", spec.scope);
                        break;
                    }
                };
                self.account(hold, &meta, c.usage, finish_name(c.finish), msg_chars(&msgs), c.text.len());
                if c.finish != Finish::Stop {
                    let reason = if c.finish == Finish::Error { "stream_error" } else { "length" };
                    self.emit_truncated(spec.scope, &checker.id, "consistency", false, reason);
                    break;
                }
                match cb::parse_consistency(&c.text, &known) {
                    Some(v) if v.is_empty() => {
                        gate = Gate::Ok;
                        break;
                    }
                    Some(v) => {
                        gate = Gate::Conflicts(v);
                        break;
                    }
                    None if attempt == 1 => {
                        log::info!("[{}] kiểm tra nhất quán trả sai định dạng → hỏi lại 1 lần", spec.scope);
                        msgs.push(ChatMessage::assistant(c.text));
                        msgs.push(ChatMessage::user(prompts::consistency_correction()));
                    }
                    None => log::warn!("[{}] kiểm tra nhất quán vẫn sai định dạng → chưa xác minh", spec.scope),
                }
            }
        }
        let (result, conflicts) = match &gate {
            Gate::Ok => ("ok", vec![]),
            Gate::Conflicts(c) => ("conflicts", c.clone()),
            Gate::Unverified => ("unverified", vec![]),
        };
        self.emitter.send(CouncilEvent::Consistency { scope: spec.scope.to_owned(), result: result.to_owned(), conflicts });
        Ok(gate)
    }

    /// 1 lần gọi người viết (stream hoặc không). `forward = true` → chuyển delta lên UI;
    /// lần nối tiếp dùng `false` (UI nhận toàn văn bản ghép sau khi kiểm tra xong).
    async fn author_completion(
        &self,
        ep: &ResolvedEndpoint,
        scope: &str,
        version: u32,
        messages: &[ChatMessage],
        forward: bool,
    ) -> Result<Completion, LlmError> {
        if !self.cfg.stream_plans {
            return self.llm.chat(ep, messages, Expect::Text, &self.cancel).await;
        }
        let mut pending = String::new();
        let mut last_flush = Instant::now();
        let emitter = &self.emitter;
        let completion = self
            .llm
            .chat_stream(ep, messages, &self.cancel, |sig| {
                if !forward {
                    return;
                }
                match sig {
                    StreamSignal::Reset => {
                        pending.clear();
                        emitter.send(CouncilEvent::PlanStreamReset {
                            scope: scope.to_owned(),
                            version,
                        });
                    }
                    StreamSignal::Delta(d) => {
                        pending.push_str(d);
                        if last_flush.elapsed() >= DELTA_FLUSH_INTERVAL || pending.len() >= DELTA_FLUSH_BYTES {
                            emitter.send(CouncilEvent::PlanDelta {
                                scope: scope.to_owned(),
                                version,
                                delta: std::mem::take(&mut pending),
                            });
                            last_flush = Instant::now();
                        }
                    }
                }
            })
            .await?;
        if !pending.is_empty() {
            self.emitter.send(CouncilEvent::PlanDelta {
                scope: scope.to_owned(),
                version,
                delta: pending,
            });
        }
        Ok(completion)
    }

    /// Plan bị cắt do `max_tokens`: bỏ section dở dang cuối, xin model viết tiếp từ đúng heading đó.
    /// Tối đa `MAX_CONTINUATIONS` lần thử, mỗi lần giữ chỗ ngân sách riêng. Chạy dưới permit của
    /// lần gọi gốc (không xin thêm permit → không deadlock khi `max_parallel_calls = 1`).
    /// `Ok(Ok(md))` = bản ghép hợp lệ; `Ok(Err(reason))` = không cứu được ("budget" | `reason`).
    #[allow(clippy::too_many_arguments)]
    async fn continue_plan(
        &self,
        ep: &ResolvedEndpoint,
        scope: &str,
        author: &RoleDef,
        version: u32,
        round: u32,
        messages: &[ChatMessage],
        cut_text: &str,
        reason: &'static str,
    ) -> Result<Result<String, &'static str>, Halt> {
        let Some((kept, heading)) = sections::cut_last_incomplete(cut_text) else {
            log::warn!("[{scope}] plan v{version} bị cắt nhưng không có heading để cắt lại → bỏ");
            return Ok(Err(reason));
        };
        let msgs = prompts::author_continue_messages(messages, &kept, &heading);
        for attempt in 1..=MAX_CONTINUATIONS {
            let Some(hold) = self.hold(estimate_call(&msgs, ep.max_tokens)) else {
                return Ok(Err("budget"));
            };
            let meta = CallMeta::new(scope, &author.id, "plan", &ep.model, round);
            let c = self.author_completion(ep, scope, version, &msgs, false).await?;
            self.account(hold, &meta, c.usage, finish_name(c.finish), msg_chars(&msgs), c.text.len());
            if let Some(md) = accept_continuation(&kept, &heading, &c) {
                return Ok(Ok(md));
            }
            log::warn!(
                "[{scope}] nối tiếp plan v{version} lần {attempt} không hợp lệ (finish={:?}, heading '{heading}')",
                c.finish
            );
        }
        Ok(Err(reason))
    }

    /// Chính sách "Tất cả": reviewer tự kiểm chứng (1 lần ở round 1), ghi chú dùng lại các round sau.
    async fn reviewer_research(&self, spec: &LoopSpec<'_>, reviewer: &RoleDef, request: &str, plan: &PlanVersion) {
        if self.cfg.resolve(&reviewer.id).is_none() {
            return;
        }
        let note_key = format!("{}/{}", spec.scope, reviewer.id);
        let purpose = "review the plan below from your area of expertise";
        // Lỗi / huỷ: bỏ qua ghi chú (caller kiểm tra `cancel` ngay sau đó).
        if let Ok(Some(n)) = self.research(spec.scope, reviewer, "review", 1, purpose, &spec.ctx, request, Some(plan)).await {
            self.reviewer_notes.lock().unwrap_or_else(|e| e.into_inner()).insert(note_key, n);
        }
    }

    /// Dựng prompt review (chưa gọi model) — để giữ chỗ ngân sách cho cả round trước khi gọi.
    fn prepare_review(
        &self,
        spec: &LoopSpec<'_>,
        reviewer: &RoleDef,
        mut rc: prompts::ReviewCtx<'_>,
        request: &str,
        plan: &PlanVersion,
        previous: Option<&Review>,
    ) -> Result<PreparedReview, LlmError> {
        let round = rc.round;
        let opts = self.prompt_opts();
        let ep = self
            .cfg
            .resolve(&reviewer.id)
            .ok_or_else(|| LlmError::InvalidResponse(format!("role '{}' không tồn tại", reviewer.id)))?;
        let note_key = format!("{}/{}", spec.scope, reviewer.id);
        let notes = self.reviewer_notes.lock().unwrap_or_else(|e| e.into_inner()).get(&note_key).cloned();

        // Review theo section: chỉ gửi phần được gắn tag cho reviewer này (+ phần dùng chung).
        // Mục được issue vòng trước / dòng Changelog gửi reviewer nhắc tới → gửi đầy đủ để kiểm chứng;
        // không xác định được mục nào → gửi nguyên plan.
        let routed_view = if opts.routing {
            let prev_issues = previous.map(|p| p.critical_issues.as_slice()).unwrap_or(&[]);
            routing::referenced_sections(&plan.markdown, &reviewer.id, &reviewer.name, prev_issues)
                .and_then(|keep| routing::view_for(&plan.markdown, &reviewer.id, &keep))
        } else {
            None
        };
        let saved = routed_view.as_ref().map(|v| {
            rc.routed = true;
            estimate_tokens(plan.markdown.len().saturating_sub(v.len()))
        });
        let view = routed_view.as_deref().unwrap_or(&plan.markdown);
        let strict = opts.strict_first_round && round == 1 && !rc.invited;
        let messages = self.decorate(
            prompts::reviewer_messages(reviewer, &spec.author.name, &spec.ctx, &rc, &opts, request, plan.version, view, previous),
            notes.as_deref(),
            false,
        );
        Ok(PreparedReview { ep, messages, strict, saved })
    }

    /// Gọi model review với prompt đã dựng và chỗ ngân sách đã giữ.
    async fn exec_review(
        &self,
        spec: &LoopSpec<'_>,
        reviewer: &RoleDef,
        round: u32,
        plan_version: u32,
        prep: PreparedReview,
        hold: Hold<'_>,
    ) -> Result<Review, LlmError> {
        let PreparedReview { ep, mut messages, strict, saved } = prep;
        let _permit = self.permit().await?;
        self.emitter.send(CouncilEvent::AgentThinking {
            scope: spec.scope.to_owned(),
            agent: reviewer.id.clone(),
            round,
            plan_version,
        });
        if let Some(tokens) = saved {
            self.record_saving(spec.scope, &reviewer.id, tokens, false);
        }

        let meta = CallMeta::new(spec.scope, &reviewer.id, "review", &ep.model, round);
        let (raw, raw_text, usage) = match self.llm.chat_json::<RawReview>(&ep, &messages, &self.cancel).await {
            Ok(r) => r,
            Err(e) => {
                if matches!(e, LlmError::Truncated { .. }) {
                    hold.settle_truncated();
                }
                return Err(e);
            }
        };
        self.account(hold, &meta, usage, "stop", msg_chars(&messages), raw_text.len());
        let mut review = Review::from_raw(&reviewer.id, &reviewer.name, raw);

        if strict && review.violates_round_one_policy() {
            // Cho model 1 cơ hội tự sửa (rẻ hơn & trung thực hơn việc server tự bịa issue).
            log::info!("{} vi phạm round-1 policy; yêu cầu review lại", reviewer.name);
            messages.push(ChatMessage::assistant(raw_text));
            messages.push(ChatMessage::user(prompts::ROUND_ONE_CORRECTION));
            let meta2 = CallMeta::new(spec.scope, &reviewer.id, "review", &ep.model, round);
            match self.hold(estimate_call(&messages, ep.max_tokens)) {
                None => log::warn!("{}: hết ngân sách token cho lần sửa review; giữ review gốc", reviewer.name),
                Some(hold2) => match self.llm.chat_json::<RawReview>(&ep, &messages, &self.cancel).await {
                    Ok((raw2, text2, usage2)) => {
                        self.account(hold2, &meta2, usage2, "stop", msg_chars(&messages), text2.len());
                        review = Review::from_raw(&reviewer.id, &reviewer.name, raw2);
                    }
                    Err(LlmError::Cancelled) => return Err(LlmError::Cancelled),
                    Err(e) => {
                        if matches!(e, LlmError::Truncated { .. }) {
                            hold2.settle_truncated();
                        }
                        log::warn!("{}: correction call lỗi ({e}); giữ review gốc", reviewer.name);
                    }
                },
            }
            review.enforce_round_one_policy();
        }

        self.emitter.send(CouncilEvent::ReviewReady {
            scope: spec.scope.to_owned(),
            round,
            review: review.clone(),
        });
        Ok(review)
    }
}

// ═══════════════════════════════ Tests ═══════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::test_support::{completion, completion_with, spawn_mock, sse, sse_with, tool_call, Handler, MockResp};

    // ── Mock server dùng chung nằm ở `crate::test_support`; dưới đây là helper riêng của orchestrator ──

    fn system_of(body: &Value) -> String {
        body["messages"][0]["content"].as_str().unwrap_or_default().to_owned()
    }
    fn all_text(body: &Value) -> String {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn plan_version_of(body: &Value) -> u32 {
        let t = all_text(body);
        let i = t.find("<plan version=\"").map(|i| i + 15).unwrap();
        t[i..].split('"').next().unwrap().parse().unwrap()
    }
    fn is_author(body: &Value) -> bool {
        system_of(body).contains("[PLAN AUTHOR]")
    }
    fn is_reviewer(body: &Value, name: &str) -> bool {
        let s = system_of(body);
        s.contains("[PLAN REVIEWER]") && s.contains(&format!("You are {name}"))
    }

    fn plan_md(v: u32) -> String {
        format!("## 1. Overview\nPlan v{v}\n## 2. Architecture\n```mermaid\nflowchart LR\n  A-->B\n```\n## 3. Data Flow\n## 4. Tech Stack & Schema\n## 5. Edge Cases & Trade-offs\n")
    }

    fn reject() -> String {
        json!({"role":"x","verdict":"REJECT","summary_bubble":"Thiếu idempotency và backpressure nghiêm trọng",
               "critical_issues":["Không có idempotency key","Queue không giới hạn"],"suggestions":["Thêm idempotency"]}).to_string()
    }
    fn approve() -> String {
        json!({"verdict":"APPROVE","summary_bubble":"Đã xử lý đủ","critical_issues":[],"suggestions":[]}).to_string()
    }

    #[derive(Default)]
    struct CollectSink(Mutex<Vec<EventEnvelope>>);
    impl EventSink for CollectSink {
        fn emit(&self, e: &EventEnvelope) {
            self.0.lock().unwrap().push(e.clone());
        }
    }
    impl CollectSink {
        fn types(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_owned())
                .collect()
        }
    }

    fn cfg_for(base: &str, stream: bool) -> AppConfig {
        let mut c = crate::config::tests::with_default_provider("sk-test", "gpt-4.1-mini");
        c.providers[0].base_url = base.into();
        c.stream_plans = stream;
        c.max_retries = 2;
        c
    }

    async fn run_with(
        handler: Handler,
        tweak: impl FnOnce(&mut AppConfig),
        cancel_after: Option<Duration>,
    ) -> (SessionRecord, Arc<CollectSink>) {
        run_with_extras(handler, tweak, cancel_after, SessionExtras::default()).await
    }

    async fn run_with_extras(
        handler: Handler,
        tweak: impl FnOnce(&mut AppConfig),
        cancel_after: Option<Duration>,
        extras: SessionExtras,
    ) -> (SessionRecord, Arc<CollectSink>) {
        let base = spawn_mock(handler).await;
        let mut cfg = cfg_for(&base, false);
        tweak(&mut cfg);
        cfg.validate_ready().unwrap();
        let sink = Arc::new(CollectSink::default());
        let cancel = CancellationToken::new();
        let llm = LlmClient::new(crate::llm_client::build_http_client().unwrap(), &cfg);
        let council = Council::with_extras(llm, cfg, sink.clone(), Uuid::new_v4(), cancel.clone(), extras);
        if let Some(d) = cancel_after {
            tokio::spawn(async move {
                tokio::time::sleep(d).await;
                cancel.cancel();
            });
        }
        (council.run("Thiết kế hệ thống leaderboard realtime".into()).await, sink)
    }

    async fn run_council(handler: Handler, stream: bool, cancel_after: Option<Duration>) -> (SessionRecord, Arc<CollectSink>) {
        run_with(handler, |c| c.stream_plans = stream, cancel_after).await
    }

    /// Router chuẩn: author trả plan; reviewer REJECT ở v1, APPROVE từ v2; chủ trì giao việc theo JSON.
    fn standard_router(stream_arch: bool) -> Handler {
        Arc::new(move |body: &Value| {
            let sys = system_of(body);
            if sys.contains("[TASK ASSIGNMENT]") {
                return completion(
                    &json!({"assignments":[{"department_id":"qa","task":"Viết test plan cho leaderboard"}]}).to_string(),
                );
            }
            if is_author(body) {
                let t = all_text(body);
                let v = if t.contains("Write plan v") {
                    t.split("Write plan v")
                        .nth(1)
                        .unwrap()
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .trim_end_matches('.')
                        .parse()
                        .unwrap()
                } else {
                    1
                };
                assert_eq!(body["stream"], json!(stream_arch));
                return if stream_arch { sse(&plan_md(v)) } else { completion(&plan_md(v)) };
            }
            assert!(sys.contains("[PLAN REVIEWER]"), "call không nhận diện được: {sys}");
            assert_eq!(body["response_format"]["type"], "json_object");
            if plan_version_of(body) == 1 {
                completion(&reject())
            } else {
                completion(&approve())
            }
        })
    }

    #[tokio::test]
    async fn converges_in_round_two() {
        let (rec, sink) = run_council(standard_router(false), false, None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.plans.len(), 2);
        assert_eq!(rec.rounds.len(), 2);
        assert!(!rec.rounds[0].converged && rec.rounds[1].converged);
        assert_eq!(rec.rounds[1].approvals, 3);
        assert!(rec.final_markdown.unwrap().contains("Plan v2"));
        assert_eq!(rec.chair_id, "architect");
        assert_eq!(rec.cast.len(), 4);

        let types = sink.types();
        assert_eq!(types.first().unwrap(), "session_started");
        assert_eq!(types.last().unwrap(), "finished");
        assert_eq!(types.iter().filter(|t| *t == "review_ready").count(), 6);
        let seqs: Vec<u64> = sink.0.lock().unwrap().iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[tokio::test]
    async fn token_usage_is_tracked_per_role_scope_and_kind() {
        let (rec, sink) = run_council(standard_router(false), false, None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        // 2 plan (author) + 6 review, mỗi lần 100 + 20 token
        let u = &rec.usage;
        assert_eq!(u.total.calls, 8);
        assert_eq!((u.total.prompt_tokens, u.total.completion_tokens), (800, 160));
        assert_eq!(u.total.estimated_calls, 0);
        assert_eq!(u.by_role["architect"].calls, 2);
        assert_eq!(u.by_kind["review"].calls, 6);
        assert_eq!(u.by_scope[MAIN_SCOPE].total(), 960);
        assert_eq!(rec.summary().total_tokens, 960);
        let last_total = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::UsageUpdated { total, .. } => Some(*total),
                _ => None,
            })
            .next_back()
            .unwrap();
        assert_eq!(last_total.total(), 960);
    }

    #[tokio::test]
    async fn streaming_usage_is_read_from_final_chunk() {
        let (rec, _) = run_council(standard_router(true), true, None).await;
        assert_eq!(rec.usage.by_kind["plan"].calls, 2);
        assert_eq!(rec.usage.by_kind["plan"].estimated_calls, 0);
        assert_eq!(rec.usage.by_kind["plan"].prompt_tokens, 200);
    }

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pc-orch-ws-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("README.md"), "# Game backend\nDùng Go + PostgreSQL").unwrap();
        std::fs::write(dir.join("src/score.go"), "package score\nfunc Submit() {}\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn author_researches_workspace_with_tools_before_writing() {
        let dir = temp_workspace();
        let fs = crate::tools::WorkspaceFs::open("game", dir.to_str().unwrap()).unwrap();
        let extras = SessionExtras {
            overview: Some(fs.overview()),
            toolbox: Toolbox { workspace: Some(Arc::new(fs)), web: None },
            company: CompanyMode::Enforced("Backend: Go only. DB: PostgreSQL.".into()),
            info: SessionInfo { workspace: Some("game".into()), web_search: false, company_context: true, origin: None },
            ..SessionExtras::default()
        };
        let saw = Arc::new(Mutex::new(Vec::<String>::new()));
        let saw2 = saw.clone();
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let sys = system_of(body);
            if sys.contains("[RESEARCH]") {
                assert!(body["tools"].as_array().is_some_and(|t| t.len() == 3), "chỉ có tool workspace");
                assert!(sys.contains("<company_context>"));
                let msgs = body["messages"].as_array().unwrap();
                return match msgs.iter().find(|m| m["role"] == "tool") {
                    None => tool_call("read_file", json!({"path":"src/score.go"})),
                    Some(t) => {
                        assert!(t["content"].as_str().unwrap().contains("func Submit"), "tool phải trả nội dung file");
                        completion("- src/score.go: hàm Submit hiện có (Go)")
                    }
                };
            }
            if is_author(body) && !all_text(body).contains("Write plan v") {
                saw2.lock().unwrap().push(all_text(body));
            }
            if sys.contains("[PLAN REVIEWER]") {
                assert!(sys.contains("COMPANY CONTEXT POLICY (mandatory)"), "reviewer cũng phải nhận bối cảnh công ty");
            }
            base(body)
        });
        let (rec, sink) = run_with_extras(handler, |_| {}, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        let drafts = saw.lock().unwrap();
        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].contains("<research_notes>") && drafts[0].contains("hàm Submit"));
        assert!(drafts[0].contains("<workspace_overview>") && drafts[0].contains("Go + PostgreSQL"));
        assert_eq!(rec.activities.len(), 1);
        assert_eq!(rec.activities[0].tool, "read_file");
        assert!(rec.activities[0].ok && rec.activities[0].summary.contains("src/score.go"));
        assert_eq!(rec.usage.by_kind["research"].calls, 2);
        assert!(sink.types().contains(&"tool_used".to_owned()));
        assert_eq!(rec.options.workspace.as_deref(), Some("game"));
        std::fs::remove_dir_all(dir).ok();
    }

    /// `max_tool_steps = 0`: không giới hạn số lượt — 20 lượt (> trần 12 cũ) đều được thực thi thật.
    #[tokio::test]
    async fn unlimited_tool_steps_let_research_go_past_the_old_cap() {
        const WANTED: usize = 20;
        let dir = temp_workspace();
        let fs = crate::tools::WorkspaceFs::open("game", dir.to_str().unwrap()).unwrap();
        let extras = SessionExtras { toolbox: Toolbox { workspace: Some(Arc::new(fs)), web: None }, ..SessionExtras::default() };
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let sys = system_of(body);
            if sys.contains("[RESEARCH]") && is_author_research(&sys) {
                assert!(sys.contains("no fixed limit on tool calls") && sys.contains("max 600 words"), "prompt không nêu con số giới hạn");
                let msgs = body["messages"].as_array().unwrap();
                let done = msgs.iter().filter(|m| m["role"] == "tool").count();
                assert!(msgs.iter().filter(|m| m["role"] == "tool").all(|t| !t["content"].as_str().unwrap().contains("hết lượt")));
                return if done < WANTED { tool_call("read_file", json!({"path":"src/score.go"})) } else { completion("- src/score.go: Submit") };
            }
            if sys.contains("[RESEARCH]") {
                return completion(prompts::NO_RESEARCH);
            }
            base(body)
        });
        let (rec, _) = run_with_extras(handler, |c| c.max_tool_steps = 0, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        let author_reads = rec.activities.iter().filter(|a| a.phase == "author" && a.ok).count();
        assert_eq!(author_reads, WANTED);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Prompt nghiên cứu của người viết plan (không phải reviewer).
    fn is_author_research(sys: &str) -> bool {
        sys.contains("write the plan") || sys.contains("write your department")
    }

    #[tokio::test]
    async fn unlimited_tool_steps_go_past_twelve() {
        let dir = temp_workspace();
        let fs = crate::tools::WorkspaceFs::open("game", dir.to_str().unwrap()).unwrap();
        let extras = SessionExtras {
            toolbox: Toolbox { workspace: Some(Arc::new(fs)), web: None },
            ..SessionExtras::default()
        };
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let sys = system_of(body);
            if sys.contains("[RESEARCH]") {
                assert!(sys.contains("as many tool calls as you really need") && !sys.contains("at most 0"), "{sys}");
                let used = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "tool").count();
                return if used < 20 {
                    tool_call("read_file", json!({"path":"src/score.go"}))
                } else {
                    completion("- src/score.go: hàm Submit")
                };
            }
            base(body)
        });
        let (rec, _) = run_with_extras(handler, |c| c.max_tool_steps = 0, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.activities.len(), 20, "không giới hạn → vượt mức 12 cũ");
        assert!(rec.activities.iter().all(|a| a.ok));
        const { assert!(UNLIMITED_TOOL_STEPS_CAP > 20) };
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn model_without_tool_support_skips_research() {
        let dir = temp_workspace();
        let fs = crate::tools::WorkspaceFs::open("game", dir.to_str().unwrap()).unwrap();
        let extras = SessionExtras {
            toolbox: Toolbox { workspace: Some(Arc::new(fs)), web: None },
            ..SessionExtras::default()
        };
        let research_calls = Arc::new(AtomicUsize::new(0));
        let r2 = research_calls.clone();
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if system_of(body).contains("[RESEARCH]") {
                r2.fetch_add(1, Ordering::SeqCst);
                return MockResp::http(400, "application/json", r#"{"error":"tools not supported"}"#);
            }
            base(body)
        });
        // Chính sách "Tất cả" → cả reviewer cũng thử nghiên cứu, nhưng model đã bị đánh dấu không hỗ trợ.
        let (rec, _) = run_with_extras(handler, |c| c.tool_policy = ToolPolicy::All, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(research_calls.load(Ordering::SeqCst), 1, "chỉ thử 1 lần cho mỗi model");
        assert!(!rec.activities[0].ok);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn lifted_company_context_lets_roles_think_beyond_stack() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let sys = system_of(body);
            assert!(!sys.contains("<company_context>"));
            assert!(sys.contains("intentionally lifted"));
            base(body)
        });
        let extras = SessionExtras { company: CompanyMode::Lifted, ..SessionExtras::default() };
        let (rec, _) = run_with_extras(handler, |_| {}, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
    }

    /// Plan có gắn tag: A → frontend, B → backend, C → cả hai. `b` = nội dung phần B.
    fn tagged_plan(v: u32, b: &str) -> String {
        format!(
            "## 1. Overview\nPlan v{v}, tổng quan chung.\n## 2. Architecture\n<!-- reviewers: all -->\nKiến trúc.\n\
## 3. Frontend A\n<!-- reviewers: frontend_dev -->\nNỘI DUNG A {}\n## 4. Backend B\n<!-- reviewers: backend_dev -->\nNỘI DUNG B {b} {}\n\
## 5. Shared C\n<!-- reviewers: frontend_dev, backend_dev -->\nHợp đồng API chung.\n### Out of scope\n- không có\n",
            "giao diện ".repeat(40),
            "máy chủ ".repeat(40),
        )
    }

    #[tokio::test]
    async fn section_routing_sends_only_tagged_parts_and_carries_unchanged_approval() {
        let fe_calls = Arc::new(AtomicUsize::new(0));
        let fe2 = fe_calls.clone();
        let handler: Handler = Arc::new(move |body: &Value| {
            let sys = system_of(body);
            let t = all_text(body);
            if is_author(body) {
                assert!(sys.contains("REVIEW ROUTING") && sys.contains("- frontend_dev:"), "người viết phải nhận danh sách reviewer để gắn tag");
                // v2 chỉ sửa phần B (backend), A và C giữ nguyên
                return completion(&if t.contains("Write plan v2") { tagged_plan(2, "đã thêm idempotency") } else { tagged_plan(1, "") });
            }
            let v = plan_version_of(body);
            if is_reviewer(body, "Frontend Developer") {
                fe2.fetch_add(1, Ordering::SeqCst);
                assert!(t.contains("NỘI DUNG A") && t.contains("Hợp đồng API chung"));
                assert!(!t.contains("NỘI DUNG B"), "frontend không được nhận phần backend");
                return completion(&approve());
            }
            assert!(is_reviewer(body, "Backend Developer"));
            assert!(!t.contains("NỘI DUNG A") && t.contains("NỘI DUNG B"));
            completion(&if v == 1 { reject() } else { approve() })
        });
        let (rec, sink) = run_with(handler, |c| c.meeting.reviewer_ids = vec!["frontend_dev".into(), "backend_dev".into()], None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(fe_calls.load(Ordering::SeqCst), 1, "vòng 2 phần A/C không đổi → frontend giữ nguyên APPROVE, không gọi model");
        let carried = rec.rounds[1].slots.iter().find(|s| s.agent == "frontend_dev").unwrap().review.as_ref().unwrap();
        assert!(carried.carried_over && carried.verdict == Verdict::Approve);
        assert_eq!(rec.usage.carried_reviews, 1);
        assert!(rec.usage.saved_tokens > 100, "phải ghi nhận token tiết kiệm: {}", rec.usage.saved_tokens);
        assert!(sink.types().contains(&"tokens_saved".to_owned()));
    }

    #[tokio::test]
    async fn reviewer_can_abstain_and_leave_the_meeting() {
        let tester_calls = Arc::new(AtomicUsize::new(0));
        let t2 = tester_calls.clone();
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_reviewer(body, "Tester") {
                t2.fetch_add(1, Ordering::SeqCst);
                return completion(&json!({"verdict":"ABSTAIN","summary_bubble":"Không có gì để test ở đây"}).to_string());
            }
            base(body)
        });
        let (rec, _) = run_with(
            handler,
            |c| c.meeting.reviewer_ids = vec!["scale_reviewer".into(), "chaos_reviewer".into(), "tester".into()],
            None,
        )
        .await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(tester_calls.load(Ordering::SeqCst), 1, "đã rút thì không bị gọi lại");
        assert_eq!(rec.rounds[1].slots.len(), 2);
        assert_eq!(rec.rounds[1].approvals, 2);
        assert!(rec.membership.iter().any(|m| m.agent == "tester" && m.action == "abstained" && m.by == "self"));
    }

    #[tokio::test]
    async fn reviewer_can_invite_a_missing_expert() {
        let dba_calls = Arc::new(AtomicUsize::new(0));
        let d2 = dba_calls.clone();
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let sys = system_of(body);
            if is_reviewer(body, "Performance Engineer") && plan_version_of(body) == 1 {
                assert!(sys.contains("<available_experts>") && sys.contains("- dba:"));
                return completion(
                    &json!({"verdict":"REJECT","summary_bubble":"Cần DBA","critical_issues":["Migration chưa rõ"],
                            "invite":[{"role_id":"dba","reason":"Cần DBA đánh giá migration"},{"role_id":"cfo","reason":"chi phí"}]})
                    .to_string(),
                );
            }
            if is_reviewer(body, "Database Engineer") {
                d2.fetch_add(1, Ordering::SeqCst);
                assert!(!sys.contains("ROUND 1 POLICY (mandatory)"), "người được mời không bị ép REJECT");
                return completion(&approve());
            }
            base(body)
        });
        let (rec, _) = run_with(
            handler,
            |c| {
                c.meeting.max_invites = 1;
                c.meeting.strict_first_round = true;
            },
            None,
        )
        .await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(dba_calls.load(Ordering::SeqCst), 1);
        let inv: Vec<_> = rec.membership.iter().filter(|m| m.action == "invited").collect();
        assert_eq!(inv.len(), 1, "giới hạn max_invites = 1 → CFO không được mời");
        assert_eq!((inv[0].agent.as_str(), inv[0].by.as_str()), ("dba", "scale_reviewer"));
        assert!(rec.rounds[1].slots.iter().any(|s| s.agent == "dba"));
        assert!(rec.cast.iter().any(|c| c.id == "dba"), "người được mời phải có trong cast để lịch sử hiển thị đúng");
    }

    #[tokio::test]
    async fn user_can_dismiss_and_invite_during_the_meeting() {
        let extras = SessionExtras::default();
        extras.control.push(ControlAction::Dismiss { scope: MAIN_SCOPE.into(), role_id: "chaos_reviewer".into() });
        extras.control.push(ControlAction::Invite { scope: MAIN_SCOPE.into(), role_id: "cfo".into() });
        extras.control.push(ControlAction::Invite { scope: "other".into(), role_id: "ceo".into() });
        let (rec, _) = run_with_extras(standard_router(false), |c| c.meeting.max_invites = 0, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        let agents: Vec<&str> = rec.rounds[0].slots.iter().map(|s| s.agent.as_str()).collect();
        assert_eq!(agents, ["scale_reviewer", "pragmatic_reviewer", "cfo"], "người dùng mời không bị giới hạn max_invites");
        assert!(rec.membership.iter().all(|m| m.by == "user"));
        assert_eq!(rec.membership.len(), 2, "lệnh của scope khác không áp vào họp chính");
    }

    #[tokio::test]
    async fn requester_notes_reach_every_prompt_after_they_arrive() {
        let extras = SessionExtras::default();
        extras.control.add_note("Phải hỗ trợ offline").unwrap();
        assert!(extras.control.add_note("   ").is_err());
        assert!(extras.control.add_note(&"x".repeat(MAX_USER_NOTE_CHARS + 1)).is_err());
        let seen = Arc::new(AtomicUsize::new(0));
        let base = standard_router(false);
        let s2 = seen.clone();
        let handler: Handler = Arc::new(move |body: &Value| {
            let t = all_text(body);
            assert!(t.contains("<requester_updates>") && t.contains("1. Phải hỗ trợ offline"), "mọi prompt đều có thông tin bổ sung");
            s2.fetch_add(1, Ordering::SeqCst);
            base(body)
        });
        let (rec, _) = run_with_extras(handler, |_| {}, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert!(seen.load(Ordering::SeqCst) >= 2, "người viết + reviewer");
        assert_eq!(rec.user_notes.len(), 1);
        assert_eq!(rec.user_notes[0].text, "Phải hỗ trợ offline");
    }

    #[tokio::test]
    async fn auto_staffing_picks_a_small_relevant_team() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let sys = system_of(body);
            if sys.contains("[STAFFING]") {
                let t = all_text(body);
                assert!(t.contains("- backend_dev | Backend Developer") && t.contains("- growth | Growth"));
                return completion(
                    &json!({"chair_id":"cto","reviewer_ids":["backend_dev","cto","ghost","dba"],"department_ids":[],
                            "reasons":{"backend_dev":"API là trọng tâm","dba":"Có migration"}})
                    .to_string(),
                );
            }
            base(body)
        });
        let extras = SessionExtras { auto_staff: true, ..SessionExtras::default() };
        let (rec, sink) = run_with_extras(handler, |c| c.meeting.required_role_ids = vec!["ciso".into()], None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.chair_id, "cto");
        assert_eq!(rec.reviewer_ids, ["ciso", "backend_dev", "dba"], "role bắt buộc + bỏ chủ trì và id lạ");
        let st = rec.staffing.as_ref().unwrap();
        assert_eq!(st.reasons["dba"], "Có migration");
        assert_eq!(rec.usage.by_kind["staffing"].calls, 1);
        let types = sink.types();
        assert_eq!(types[0], "session_started");
        assert!(types.contains(&"staffing_decided".to_owned()));
    }

    #[tokio::test]
    async fn custom_reviewers_and_chair_are_used() {
        let (rec, _) = run_with(
            standard_router(false),
            |c| {
                c.meeting.chair_id = "cto".into();
                c.meeting.reviewer_ids = vec!["ceo".into(), "cfo".into(), "tester".into(), "ua_manager".into(), "qa_engineer".into()];
            },
            None,
        )
        .await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.rounds[1].approvals, 5);
        let agents: Vec<&str> = rec.rounds[0].slots.iter().map(|s| s.agent.as_str()).collect();
        assert_eq!(agents, ["ceo", "cfo", "tester", "ua_manager", "qa_engineer"]);
        assert_eq!(rec.rounds[0].slots[1].review.as_ref().unwrap().role, "CFO");
    }

    #[tokio::test]
    async fn departments_meet_first_then_feed_the_main_meeting() {
        let saw_reports = Arc::new(AtomicUsize::new(0));
        let s2 = saw_reports.clone();
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let t = all_text(body);
            if is_author(body)
                && system_of(body).contains("You chair the main meeting")
                && !t.contains("Write plan v")
                && t.contains("<department_reports>")
                && t.contains("department=\"QA & QC\"")
                && t.contains("department=\"Growth (UA & Monetization)\"")
            {
                s2.fetch_add(1, Ordering::SeqCst);
            }
            if is_author(body) && system_of(body).contains("You lead the \"QA & QC\"") {
                assert!(t.contains("Viết test plan cho leaderboard"), "task từ chủ trì phải tới phòng ban");
            }
            base(body)
        });
        let (rec, sink) = run_with(
            handler,
            |c| {
                c.meeting.department_ids = vec!["qa".into(), "growth".into()];
                c.max_parallel_calls = 2;
            },
            None,
        )
        .await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(saw_reports.load(Ordering::SeqCst), 1, "chủ trì phải nhận báo cáo của 2 phòng ban");
        assert_eq!(rec.departments.len(), 2);
        for d in &rec.departments {
            assert_eq!(d.status, SessionStatus::Converged, "{}: {:?}", d.name, d.error);
            assert_eq!(d.plans.len(), 2);
            assert!(d.final_markdown.is_some());
        }
        // Phòng ban growth không được chủ trì giao việc → nhận task mặc định
        assert!(rec.departments[1].task.contains("Growth"));
        let types = sink.types();
        assert_eq!(types.iter().filter(|t| *t == "department_finished").count(), 2);
        assert!(types.contains(&"departments_assigned".to_owned()));
        let cast: Vec<&str> = rec.cast.iter().map(|c| c.id.as_str()).collect();
        assert!(cast.contains(&"qa_lead") && cast.contains(&"monetization_manager"));
    }

    #[tokio::test]
    async fn failing_department_does_not_kill_the_session() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if system_of(body).contains("You lead the \"QA & QC\"") {
                return MockResp::http(401, "application/json", r#"{"error":"bad key"}"#);
            }
            base(body)
        });
        let (rec, _) = run_with(handler, |c| c.meeting.department_ids = vec!["qa".into(), "growth".into()], None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.departments[0].status, SessionStatus::Failed);
        assert!(rec.departments[0].error.as_deref().unwrap().contains("401"));
        assert_eq!(rec.departments[1].status, SessionStatus::Converged);
    }

    #[tokio::test]
    async fn streaming_architect_emits_deltas() {
        let (rec, sink) = run_council(standard_router(true), true, None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        let types = sink.types();
        assert!(types.contains(&"plan_stream_reset".to_owned()));
        assert!(types.contains(&"plan_delta".to_owned()));
        let joined: String = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::PlanDelta { version: 1, delta, scope } if scope == MAIN_SCOPE => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(joined.trim(), plan_md(1).trim());
    }

    #[tokio::test]
    async fn sycophantic_reviewer_is_forced_to_reject_in_round_one() {
        let corrections = Arc::new(AtomicUsize::new(0));
        let c2 = corrections.clone();
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_reviewer(body, "Security Engineer") {
                if all_text(body).contains("ROUND 1 POLICY VIOLATION") {
                    c2.fetch_add(1, Ordering::SeqCst);
                }
                return completion(&approve());
            }
            base(body)
        });
        let (rec, _) = run_with(handler, |c| c.meeting.strict_first_round = true, None).await;
        assert_eq!(corrections.load(Ordering::SeqCst), 1);
        let chaos_r1 = rec.rounds[0].slots.iter().find(|s| s.agent == "chaos_reviewer").unwrap();
        let r = chaos_r1.review.as_ref().unwrap();
        assert_eq!(r.verdict, Verdict::Reject);
        assert!(r.policy_override.is_some());
        assert_eq!(rec.status, SessionStatus::Converged);
    }

    #[tokio::test]
    async fn lenient_first_round_allows_early_approval() {
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_author(body) {
                return completion(&plan_md(1));
            }
            assert!(!all_text(body).contains("MUST be \"REJECT\""));
            completion(&approve())
        });
        let (rec, _) = run_with(handler, |c| c.meeting.strict_first_round = false, None).await;
        assert_eq!(rec.status, SessionStatus::Converged);
        assert_eq!(rec.rounds.len(), 1);
    }

    #[tokio::test]
    async fn stubborn_reviewer_hits_max_rounds_with_open_issues() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_reviewer(body, "Tech Lead") {
                return completion(&format!("```json\n{}\n```", reject()));
            }
            base(body)
        });
        let (rec, _) = run_council(handler, false, None).await;
        assert_eq!(rec.status, SessionStatus::MaxRoundsReached);
        assert_eq!(rec.plans.len(), 3);
        assert_eq!(rec.rounds.len(), 3);
        let fin = rec.final_markdown.unwrap();
        assert!(fin.contains("Plan v3") && fin.contains("Open Issues") && fin.contains("Queue không giới hạn"));
    }

    #[tokio::test]
    async fn one_reviewer_down_does_not_kill_the_round() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_reviewer(body, "Performance Engineer") {
                return MockResp::http(401, "application/json", r#"{"error":"bad key"}"#);
            }
            base(body)
        });
        let (rec, sink) = run_council(handler, false, None).await;
        assert_eq!(rec.status, SessionStatus::PartiallyApproved);
        assert_eq!(sink.types().iter().filter(|t| *t == "review_failed").count(), 2);
        let fin = rec.final_markdown.unwrap();
        assert!(fin.contains("Performance Engineer (không review được)"), "{fin}");
    }

    #[tokio::test]
    async fn retries_on_503_then_succeeds() {
        let hits = Arc::new(AtomicUsize::new(0));
        let h2 = hits.clone();
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_author(body) && h2.fetch_add(1, Ordering::SeqCst) == 0 {
                return MockResp::http(503, "text/plain", "overloaded");
            }
            base(body)
        });
        let (rec, _) = run_council(handler, false, None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert!(hits.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn json_mode_falls_back_when_provider_rejects_response_format() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if !is_author(body) && body.get("response_format").is_some() {
                return MockResp::http(400, "application/json", r#"{"error":"response_format unsupported"}"#);
            }
            if !is_author(body) {
                let r = if plan_version_of(body) == 1 { reject() } else { approve() };
                return completion(&format!("Đây là review của tôi:\n{r}\nHết."));
            }
            base(body)
        });
        let (rec, _) = run_council(handler, false, None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
    }

    #[tokio::test]
    async fn cancel_stops_promptly() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let mut r = base(body);
            r.delay = Duration::from_secs(10);
            r
        });
        let t0 = Instant::now();
        let (rec, sink) = run_council(handler, false, Some(Duration::from_millis(150))).await;
        assert_eq!(rec.status, SessionStatus::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(2), "cancel phải gần như tức thì");
        assert_eq!(sink.types().last().unwrap(), "cancelled");
    }

    #[tokio::test]
    async fn cancel_during_departments_stops_everything() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            let mut r = base(body);
            if system_of(body).contains("You lead the") {
                r.delay = Duration::from_secs(10);
            }
            r
        });
        let t0 = Instant::now();
        let (rec, sink) = run_with(
            handler,
            |c| c.meeting.department_ids = vec!["qa".into(), "engineering".into()],
            Some(Duration::from_millis(300)),
        )
        .await;
        assert_eq!(rec.status, SessionStatus::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert!(rec.plans.is_empty(), "không được vào cuộc họp chính sau khi huỷ");
        assert_eq!(sink.types().last().unwrap(), "cancelled");
    }

    // ── Output bị cắt + ngân sách token ──

    fn is_continuation(body: &Value) -> bool {
        is_author(body) && all_text(body).contains(prompts::CONTINUE_MARKER)
    }

    /// Phiên bản plan mà lần gọi người viết đang viết ("Write plan vN" → N, bản nháp → 1).
    fn author_version(body: &Value) -> u32 {
        let t = all_text(body);
        t.split("Write plan v")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.trim_end_matches('.').parse().ok())
            .unwrap_or(1)
    }

    fn truncations(sink: &CollectSink) -> Vec<(String, String, bool, String)> {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::Truncated { agent, call_kind, recovered, reason, .. } => {
                    Some((agent.clone(), call_kind.clone(), *recovered, reason.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn budgets(sink: &CollectSink) -> Vec<(u64, u64, u64, bool, String)> {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::Budget { used, reserved, limit, estimated, soft, state } => {
                    assert!(*soft);
                    Some((*used, *reserved, *limit, *estimated, state.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// Plan bị cắt giữa section 2 (heading cuối ngoài fence = "## 2. Architecture").
    const CUT_PLAN: &str = "## 1. Overview\nPlan v1 bị cắt\n## 2. Architecture\nKiến trúc dở da";
    const TAIL: &str = "## 2. Architecture\nKiến trúc đầy đủ\n```mermaid\nflowchart LR\n  A-->B\n```\n## 3. Data Flow\nLuồng dữ liệu\n";

    #[test]
    fn continuation_is_validated_strictly() {
        let ok = Completion { text: format!("\n\n{TAIL}"), usage: None, finish: Finish::Stop };
        let joined = accept_continuation("## 1. A\nx\n", "## 2. Architecture", &ok).unwrap();
        assert!(joined.starts_with("## 1. A\nx\n## 2. Architecture\n") && joined.ends_with("Luồng dữ liệu"));
        let wrong_head = Completion { text: "## 3. Data Flow\n".into(), usage: None, finish: Finish::Stop };
        assert!(accept_continuation("", "## 2. Architecture", &wrong_head).is_none());
        let cut = Completion { text: TAIL.into(), usage: None, finish: Finish::Length };
        assert!(accept_continuation("", "## 2. Architecture", &cut).is_none());
        let open_fence = Completion { text: "## 2. Architecture\n```js\nlet a\n".into(), usage: None, finish: Finish::Stop };
        assert!(accept_continuation("", "## 2. Architecture", &open_fence).is_none());
    }

    #[tokio::test]
    async fn truncated_plan_is_continued_and_joined() {
        let conts = Arc::new(AtomicUsize::new(0));
        let c2 = conts.clone();
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_continuation(body) {
                c2.fetch_add(1, Ordering::SeqCst);
                let t = all_text(body);
                assert!(t.contains("## 2. Architecture") && t.contains("Plan v1 bị cắt"), "phải gửi phần đã giữ + heading");
                assert!(!t.contains("Kiến trúc dở da"), "phần dở dang không được gửi lại");
                return sse(TAIL);
            }
            if is_author(body) {
                return sse_with(CUT_PLAN, Some("length"), Some((100, 20)));
            }
            completion(&approve())
        });
        let (rec, sink) = run_with(
            handler,
            |c| {
                c.stream_plans = true;
                c.meeting.strict_first_round = false;
            },
            None,
        )
        .await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(conts.load(Ordering::SeqCst), 1);
        assert_eq!(rec.plans.len(), 1);
        let md = &rec.plans[0].markdown;
        assert!(md.starts_with("## 1. Overview\nPlan v1 bị cắt\n## 2. Architecture\nKiến trúc đầy đủ"), "{md}");
        assert!(!md.contains("dở da") && md.contains("## 3. Data Flow"));
        assert_eq!(truncations(&sink), vec![("architect".into(), "plan".into(), true, "length".into())]);

        // UI: sau reset cuối cùng, delta của v1 ghép lại đúng bằng bản ghép (không còn đuôi bị cắt).
        let evs = sink.0.lock().unwrap();
        let last_reset = evs
            .iter()
            .rposition(|e| matches!(&e.event, CouncilEvent::PlanStreamReset { version: 1, .. }))
            .unwrap();
        let shown: String = evs[last_reset..]
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::PlanDelta { version: 1, delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(&shown, md);
    }

    #[tokio::test]
    async fn failed_continuations_stop_with_truncated_and_keep_last_version() {
        let conts = Arc::new(AtomicUsize::new(0));
        let c2 = conts.clone();
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_continuation(body) {
                // Lần 1: sai heading; lần 2: lại bị cắt → cả 2 đều không hợp lệ.
                return if c2.fetch_add(1, Ordering::SeqCst) == 0 {
                    completion("## 3. Data Flow\nbỏ qua section 2")
                } else {
                    completion_with(TAIL, Some("length"), Some((100, 20)))
                };
            }
            if is_author(body) {
                return if author_version(body) == 1 {
                    completion(&plan_md(1))
                } else {
                    completion_with(CUT_PLAN, Some("length"), Some((100, 20)))
                };
            }
            completion(&if plan_version_of(body) == 1 { reject() } else { approve() })
        });
        let (rec, sink) = run_with(handler, |_| {}, None).await;
        assert_eq!(rec.status, SessionStatus::Truncated, "{:?}", rec.error);
        assert_eq!(conts.load(Ordering::SeqCst), MAX_CONTINUATIONS as usize);
        assert_eq!(rec.plans.len(), 1, "không tạo phiên bản từ plan bị cắt");
        assert_eq!(rec.rounds.len(), 1);
        let fin = rec.final_markdown.unwrap();
        assert!(fin.contains("Plan v1") && fin.contains("Phiên dừng sớm") && fin.contains("Open Issues"), "{fin}");
        assert_eq!(truncations(&sink), vec![("architect".into(), "plan".into(), false, "length".into())]);
        assert!(!sink.0.lock().unwrap().iter().any(|e| matches!(&e.event, CouncilEvent::PlanReady { version: 2, .. })));
        assert_eq!(sink.types().last().unwrap(), "finished");
    }

    #[tokio::test]
    async fn broken_stream_stops_with_truncated() {
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_author(body) {
                return if author_version(body) == 1 { sse(&plan_md(1)) } else { sse(&plan_md(2)).abort_after(3) };
            }
            completion(&if plan_version_of(body) == 1 { reject() } else { approve() })
        });
        let (rec, sink) = run_with(handler, |c| c.stream_plans = true, None).await;
        assert_eq!(rec.status, SessionStatus::Truncated, "{:?}", rec.error);
        assert_eq!(rec.plans.len(), 1);
        assert_eq!(truncations(&sink)[0].3, "stream_error");
    }

    #[tokio::test]
    async fn truncated_reviewer_fails_slot_and_session_never_converges() {
        let handler: Handler = Arc::new(move |body: &Value| {
            if is_author(body) {
                return completion(&plan_md(author_version(body)));
            }
            let v = plan_version_of(body);
            if is_reviewer(body, "Performance Engineer") && v == 1 {
                return completion_with(r#"{"verdict":"REJECT","critical_issues":["dở da"#, Some("length"), None);
            }
            completion(&if v == 1 { reject() } else { approve() })
        });
        // max_tokens đã ở trần → chat_json không nới thêm, trả Truncated { max_tokens_cap } ngay.
        let (rec, sink) = run_with(handler, |c| c.max_tokens = Some(OUTPUT_TOKENS_CAP), None).await;
        let slot = rec.rounds[0].slots.iter().find(|s| s.agent == "scale_reviewer").unwrap();
        assert!(slot.review.is_none() && slot.error.as_deref().unwrap().contains("bị cắt"));
        assert!(rec.rounds[1].converged, "round 2 mọi reviewer đều APPROVE");
        assert_eq!(rec.status, SessionStatus::PartiallyApproved, "có reviewer bị cắt → không được Converged");
        assert_eq!(truncations(&sink), vec![("scale_reviewer".into(), "review".into(), false, "max_tokens_cap".into())]);
        assert!(sink.types().contains(&"review_failed".to_owned()));
    }

    /// Router có usage tuỳ chỉnh: reviewer REJECT v1, APPROVE từ v2.
    fn usage_router(usage: Option<(u32, u32)>, reviewer_calls: Arc<AtomicUsize>) -> Handler {
        Arc::new(move |body: &Value| {
            if is_author(body) {
                return completion_with(&plan_md(author_version(body)), Some("stop"), usage);
            }
            reviewer_calls.fetch_add(1, Ordering::SeqCst);
            let r = if plan_version_of(body) == 1 { reject() } else { approve() };
            completion_with(&r, Some("stop"), usage)
        })
    }

    #[tokio::test]
    async fn budget_too_small_for_round_two_keeps_v1() {
        // Mỗi lần gọi tốn 20k token: v1 (20k) + 3 review (60k) = 80k > 50k → không giữ được chỗ cho v2.
        let calls = Arc::new(AtomicUsize::new(0));
        let (rec, sink) =
            run_with(usage_router(Some((20_000, 0)), calls.clone()), |c| c.meeting.max_total_tokens = Some(50_000), None).await;
        assert_eq!(rec.status, SessionStatus::BudgetExhausted, "{:?}", rec.error);
        assert_eq!(rec.plans.len(), 1);
        assert_eq!(rec.rounds.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let fin = rec.final_markdown.unwrap();
        assert!(fin.contains("Plan v1") && fin.contains("ngân sách") && fin.contains("Open Issues"), "{fin}");
        let b = budgets(&sink);
        let last = b.last().unwrap();
        assert_eq!((last.0, last.1, last.2), (80_000, 0, 50_000), "used = usage thật, không còn chỗ giữ");
        assert_eq!(last.4, "exhausted");
        assert!(b.iter().any(|x| x.4 == "warn" || x.4 == "ok"));
        assert!(!last.3);
    }

    #[tokio::test]
    async fn reviewer_group_reservation_is_all_or_nothing() {
        // Ước lượng mỗi lần gọi ≈ 30k (max_tokens) + prompt. Plan v1 vừa đủ; 2 reviewer (~62k) cũng vừa
        // 70k, nhưng cả nhóm 3 reviewer (~93k) thì không → không reviewer nào được gọi.
        let calls = Arc::new(AtomicUsize::new(0));
        let (rec, sink) = run_with(
            usage_router(Some((100, 20)), calls.clone()),
            |c| {
                c.max_tokens = Some(30_000);
                c.meeting.max_total_tokens = Some(70_000);
            },
            None,
        )
        .await;
        assert_eq!(rec.status, SessionStatus::BudgetExhausted, "{:?}", rec.error);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "nhóm bị từ chối → không gọi reviewer nào");
        assert!(rec.rounds.is_empty());
        assert_eq!(rec.plans.len(), 1);
        let last = budgets(&sink).pop().unwrap();
        assert_eq!((last.0, last.1, last.4.as_str()), (120, 0, "exhausted"));
    }

    #[tokio::test]
    async fn missing_usage_marks_budget_estimated() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (rec, sink) =
            run_with(usage_router(None, calls), |c| c.meeting.max_total_tokens = Some(10_000_000), None).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        let b = budgets(&sink);
        assert_eq!(b.len(), 8, "1 event sau mỗi lần settle (2 plan + 6 review)");
        let last = b.last().unwrap();
        assert!(last.3, "thiếu usage → estimated");
        assert_eq!(last.0, rec.usage.total.total(), "sổ ngân sách khớp báo cáo token");
        assert_eq!((last.1, last.4.as_str()), (0, "ok"));
    }

    #[tokio::test]
    async fn no_budget_events_without_limit() {
        let (rec, sink) = run_council(standard_router(false), false, None).await;
        assert_eq!(rec.status, SessionStatus::Converged);
        assert!(budgets(&sink).is_empty());
    }

    #[tokio::test]
    async fn truncated_department_feeds_main_meeting_like_max_rounds() {
        let base = standard_router(false);
        let handler: Handler = Arc::new(move |body: &Value| {
            if system_of(body).contains("You lead the \"QA & QC\"") && is_author(body) {
                return if author_version(body) == 1 {
                    completion(&plan_md(1))
                } else {
                    completion_with(CUT_PLAN, Some("length"), Some((100, 20)))
                };
            }
            if is_continuation(body) {
                return completion("không có heading");
            }
            base(body)
        });
        let (rec, _) = run_with(handler, |c| c.meeting.department_ids = vec!["qa".into()], None).await;
        let qa = &rec.departments[0];
        assert_eq!(qa.status, SessionStatus::Truncated, "{:?}", qa.error);
        assert!(qa.final_markdown.as_deref().unwrap().contains("Phiên dừng sớm"));
        assert_eq!(rec.plans.len(), 2, "cuộc họp chính vẫn chạy với báo cáo của phòng ban");
        assert_eq!(rec.status, SessionStatus::Truncated, "trạng thái phiên = kết quả tệ nhất");
    }

    // ── Section revise + kiểm tra nhất quán ──

    /// Plan có Changelog từ v2. Id sau khi kế thừa: s1..s5 = mục 1..5, s6 = Changelog.
    fn sec_plan(v: u32) -> String {
        let fill = "Nội dung chi tiết của mục này để plan có kích thước thực tế. ".repeat(6);
        let mut md = format!(
            "## 1. Overview\nTổng quan v{v}. {fill}\n## 2. Architecture\nKiến trúc. {fill}{fill}\n```mermaid\nflowchart LR\n  A-->B\n```\n\
## 3. Data Flow\nLuồng dữ liệu v{v}.\n## 4. Tech Stack & Schema\nRedis. {fill}\n## 5. Edge Cases & Trade-offs\nRủi ro. {fill}\n"
        );
        if v > 1 {
            md.push_str(&format!(
                "## 6. Changelog (v{} → v{v})\n| # | Reviewer | Issue | Resolution | Status |\n|---|---|---|---|---|\n| 1 | Tech Lead | Backpressure | Thêm queue | Fixed |\n",
                v - 1
            ));
        }
        md
    }

    fn patch_block(id: &str, base: u32, body: &str) -> String {
        format!("<!-- section id=\"{id}\" base_version=\"{base}\" -->\n{body}\n<!-- /section -->\n")
    }
    const NEW_FLOW: &str = "## 3. Data Flow\nLuồng dữ liệu v3: thêm queue có giới hạn.";
    const NEW_LOG: &str = "## 6. Changelog (v2 → v3)\n| # | Reviewer | Issue | Resolution | Status |\n|---|---|---|---|---|\n| 1 | Tech Lead | Backpressure | Queue giới hạn | Fixed |";

    fn good_patch() -> String {
        patch_block("s3", 2, NEW_FLOW) + &patch_block("s6", 2, NEW_LOG)
    }

    fn is_section_call(body: &Value) -> bool {
        system_of(body).contains("SECTION REVISE")
    }
    fn is_gate(body: &Value) -> bool {
        system_of(body).starts_with(prompts::CONSISTENCY_MARKER)
    }
    fn body_size(body: &Value) -> usize {
        body["messages"].to_string().len()
    }

    /// Ghi lại các lần gọi đặc biệt để test kiểm tra.
    #[derive(Default)]
    struct Seen {
        /// Kích thước request section revise.
        section: Vec<usize>,
        /// User message của request section revise.
        section_user: Vec<String>,
        /// (version, kích thước request) của các lần viết lại toàn bộ (v ≥ 2).
        full: Vec<(u32, usize)>,
        conflict_refines: usize,
        gate: Vec<String>,
    }

    type Reply = Arc<dyn Fn(usize) -> MockResp + Send + Sync>;

    struct SecScenario {
        /// Reviewer REJECT các phiên bản ≤ giá trị này.
        reject_until: u32,
        /// Field `sections` của review REJECT.
        sections: Value,
        /// Câu trả lời cho lần section revise thứ n (từ 1).
        patch: Reply,
        /// Câu trả lời cho lần kiểm tra nhất quán thứ n (từ 1).
        gate: Reply,
        /// Usage của reviewer theo phiên bản plan (mặc định 100/20).
        review_usage: Arc<dyn Fn(u32) -> (u32, u32) + Send + Sync>,
    }

    impl Default for SecScenario {
        fn default() -> Self {
            Self {
                reject_until: 2,
                sections: json!(["3"]),
                patch: Arc::new(|_| completion(&good_patch())),
                gate: Arc::new(|_| completion("OK")),
                review_usage: Arc::new(|_| (100, 20)),
            }
        }
    }

    fn section_router(sc: SecScenario, seen: Arc<Mutex<Seen>>) -> Handler {
        let (n_patch, n_gate) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        Arc::new(move |body: &Value| {
            if is_gate(body) {
                seen.lock().unwrap().gate.push(all_text(body));
                return (sc.gate)(n_gate.fetch_add(1, Ordering::SeqCst) + 1);
            }
            if is_author(body) {
                let v = author_version(body);
                if is_section_call(body) {
                    let mut s = seen.lock().unwrap();
                    s.section.push(body_size(body));
                    s.section_user.push(body["messages"][1]["content"].as_str().unwrap_or_default().to_owned());
                    drop(s);
                    return (sc.patch)(n_patch.fetch_add(1, Ordering::SeqCst) + 1);
                }
                if v >= 2 {
                    let mut s = seen.lock().unwrap();
                    s.full.push((v, body_size(body)));
                    if all_text(body).contains("Consistency check — REJECT") {
                        s.conflict_refines += 1;
                    }
                }
                return completion(&sec_plan(v));
            }
            let v = plan_version_of(body);
            let r = if v <= sc.reject_until {
                json!({"verdict":"REJECT","summary_bubble":"Thiếu backpressure","critical_issues":["Thiếu backpressure ở luồng dữ liệu"],
                       "sections": sc.sections.clone(),"suggestions":["Thêm queue"]})
                .to_string()
            } else {
                approve()
            };
            completion_with(&r, Some("stop"), Some((sc.review_usage)(v)))
        })
    }

    async fn run_section(
        sc: SecScenario,
        tweak: impl FnOnce(&mut AppConfig),
    ) -> (SessionRecord, Arc<CollectSink>, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (rec, sink) = run_with(
            section_router(sc, seen.clone()),
            |c| {
                c.meeting.revise_mode = ReviseMode::Auto;
                c.max_rounds = 4;
                tweak(c);
            },
            None,
        )
        .await;
        (rec, sink, seen)
    }

    fn fallbacks(sink: &CollectSink) -> Vec<(String, u32)> {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::ReviseFallback { reason, consecutive, .. } => Some((fallback_name(*reason), *consecutive)),
                _ => None,
            })
            .collect()
    }

    fn gate_results(sink: &CollectSink) -> Vec<(String, Vec<ConsistencyConflict>)> {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::Consistency { result, conflicts, .. } => Some((result.clone(), conflicts.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn meeting_revise_config_defaults_and_validation() {
        let mut c = crate::config::tests::with_default_provider("k", "m");
        assert_eq!(c.meeting.revise_mode, ReviseMode::Full);
        assert_eq!(c.meeting.section_mode_max_ratio, 0.5);
        assert_eq!(c.meeting.consistency_checker, "author");
        let old: crate::config::MeetingConfig = serde_json::from_str("{}").unwrap();
        assert_eq!((old.revise_mode, old.consistency_checker.as_str()), (ReviseMode::Full, "author"));
        let m: crate::config::MeetingConfig = serde_json::from_str(r#"{"revise_mode":"section"}"#).unwrap();
        assert_eq!(m.revise_mode, ReviseMode::Section);
        c.validate().unwrap();
        c.meeting.consistency_checker = "scale_reviewer".into();
        c.validate().unwrap();
        c.meeting.consistency_checker = "ghost".into();
        c.meeting.section_mode_max_ratio = 1.5;
        let e = c.validate().unwrap_err();
        assert!(e.contains("người kiểm tra nhất quán 'ghost'") && e.contains("0.1..=1.0"), "{e}");
        c.meeting.consistency_checker = "author".into();
        c.meeting.section_mode_max_ratio = 0.05;
        assert!(c.validate().is_err());
        c.meeting.section_mode_max_ratio = 0.1;
        c.validate().unwrap();
    }

    #[tokio::test]
    async fn section_mode_round_two_merges_and_sends_smaller_prompt() {
        let (rec, sink, seen) = run_section(SecScenario::default(), |_| {}).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.plans.len(), 3);
        let kinds: Vec<ReviseKind> = rec.plans.iter().map(|p| p.revise_mode).collect();
        assert_eq!(kinds, vec![ReviseKind::Initial, ReviseKind::Full, ReviseKind::Section]);
        assert!(rec.plans.iter().all(|p| p.fallback_reason.is_none()));
        assert!(fallbacks(&sink).is_empty());

        // Section không cần sửa giữ nguyên từng byte; plan mới là plan đầy đủ hợp lệ.
        let (v2, v3) = (sections::split(&rec.plans[1].markdown), sections::split(&rec.plans[2].markdown));
        assert_eq!(v2.len(), v3.len());
        for i in [0, 1, 3, 4] {
            assert_eq!(v2[i].text.as_bytes(), v3[i].text.as_bytes(), "section {}", v2[i].heading);
        }
        assert!(v3[2].text.starts_with(NEW_FLOW) && v3[5].text.starts_with(NEW_LOG));
        assert!(!rec.plans[2].markdown.contains("<!--"), "không còn marker trong plan");
        let ready = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match &e.event {
                CouncilEvent::PlanReady { version: 3, markdown, .. } => Some(markdown.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(ready, rec.plans[2].markdown);

        // Kiểm tra nhất quán chạy đúng 1 lần, kết quả OK.
        assert_eq!(gate_results(&sink), vec![("ok".to_owned(), vec![])]);
        assert_eq!(rec.usage.by_kind["consistency"].calls, 1);
        let gate_prompt = seen.lock().unwrap().gate[0].clone();
        assert!(gate_prompt.contains("<section id=\"s3\">\n## 3. Data Flow\nLuồng dữ liệu v3"));

        // Cùng kịch bản ở full mode → request v3 lớn hơn hẳn.
        let (full_rec, full_sink, full_seen) = run_section(SecScenario::default(), |c| c.meeting.revise_mode = ReviseMode::Full).await;
        assert_eq!(full_rec.status, SessionStatus::Converged);
        assert!(gate_results(&full_sink).is_empty(), "full mode không chạy kiểm tra nhất quán");
        let full_v3 = full_seen.lock().unwrap().full.iter().find(|(v, _)| *v == 3).unwrap().1;
        let section_v3 = seen.lock().unwrap().section[0];
        eprintln!("kích thước request v3: section = {section_v3} byte, full = {full_v3} byte");
        assert!(section_v3 < full_v3, "section {section_v3} ≥ full {full_v3}");
        // Tập cần sửa điển hình: section reviewer nêu + Changelog.
        let user = seen.lock().unwrap().section_user[0].clone();
        let payload_ids: Vec<&str> = ["s1", "s2", "s3", "s4", "s5", "s6"]
            .into_iter()
            .filter(|id| user.contains(&format!("<!-- section id=\"{id}\"")))
            .collect();
        assert_eq!(payload_ids, vec!["s3", "s6"]);
        assert!(rec.usage.total.calls >= full_rec.usage.total.calls);
    }

    #[tokio::test]
    async fn every_fallback_reason_triggers_a_full_revise() {
        let blk = |id: &str, body: &str| patch_block(id, 2, body);
        let flow = || blk("s3", NEW_FLOW);
        let log = || blk("s6", NEW_LOG);
        let cases: Vec<(&str, String, &str)> = vec![
            ("thiếu id", flow(), "missing_ids"),
            ("thừa id", flow() + &log() + &blk("s4", "## 4. Tech Stack & Schema\nx"), "extra_ids"),
            ("trùng id", flow() + &flow() + &log(), "duplicate_ids"),
            ("id lạ", flow() + &log() + &blk("s99", "## 9. Lạ\nx"), "extra_ids"),
            ("sai base", patch_block("s3", 1, NEW_FLOW) + &patch_block("s6", 1, NEW_LOG), "base_mismatch"),
            ("không marker", sec_plan(3), "bad_structure"),
            ("2 heading", blk("s3", "## 3. Data Flow\nx\n### 3.1 Mới\ny") + &log(), "bad_structure"),
            ("structural", format!("{}\n{}", prompts_marker(), flow()), "structural_change"),
        ];
        for (name, patch, reason) in cases {
            let sc = SecScenario { patch: Arc::new(move |_| completion(&patch)), ..SecScenario::default() };
            let (rec, sink, seen) = run_section(sc, |_| {}).await;
            assert_fallback(name, &rec, &sink, &seen, reason);
        }

        // Bị cắt (finish_reason = length) → truncated.
        let sc = SecScenario {
            patch: Arc::new(|_| completion_with(&patch_block("s3", 2, NEW_FLOW), Some("length"), Some((100, 20)))),
            ..SecScenario::default()
        };
        let (rec, sink, seen) = run_section(sc, |_| {}).await;
        assert_fallback("bị cắt", &rec, &sink, &seen, "truncated");

        // Tỉ lệ: section cần sửa quá lớn so với cả plan → không gọi section revise.
        let sc = SecScenario { sections: json!(["2"]), ..SecScenario::default() };
        let (rec, sink, seen) = run_section(sc, |c| c.meeting.section_mode_max_ratio = 0.1).await;
        assert_fallback("tỉ lệ", &rec, &sink, &seen, "ratio");
        assert!(seen.lock().unwrap().section.is_empty(), "ratio được kiểm tra TRƯỚC khi gọi");

        // Reviewer REJECT mà không chỉ ra section nào → chỉ còn Changelog → ratio.
        let sc = SecScenario { sections: json!([]), ..SecScenario::default() };
        let (rec, sink, seen) = run_section(sc, |_| {}).await;
        assert_fallback("không nêu section", &rec, &sink, &seen, "ratio");
        assert!(seen.lock().unwrap().section.is_empty());
    }

    fn prompts_marker() -> &'static str {
        cb::STRUCTURAL_CHANGE_MARKER
    }

    fn assert_fallback(name: &str, rec: &SessionRecord, sink: &CollectSink, seen: &Mutex<Seen>, reason: &str) {
        assert_eq!(rec.status, SessionStatus::Converged, "{name}: {:?}", rec.error);
        assert_eq!(fallbacks(sink), vec![(reason.to_owned(), 1)], "{name}");
        let v3 = &rec.plans[2];
        assert_eq!((v3.revise_mode, v3.fallback_reason.as_deref()), (ReviseKind::Full, Some(reason)), "{name}");
        assert_eq!(v3.markdown, sec_plan(3).trim(), "{name}: v3 là bản viết lại toàn bộ");
        let s = seen.lock().unwrap();
        assert!(s.full.iter().any(|(v, _)| *v == 3), "{name}: phải viết lại toàn bộ v3");
        assert!(s.gate.is_empty(), "{name}: không có vòng section mode → không kiểm tra nhất quán");
        assert!(gate_results(sink).is_empty(), "{name}");
    }

    #[tokio::test]
    async fn two_consecutive_fallbacks_force_full_mode() {
        let sc = SecScenario {
            reject_until: 4,
            patch: Arc::new(|_| completion("viết lại hết, không có marker")),
            ..SecScenario::default()
        };
        let (rec, sink, seen) = run_section(sc, |c| c.max_rounds = 5).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.plans.len(), 5);
        assert_eq!(seen.lock().unwrap().section.len(), 2, "sau 2 lần fallback không thử section revise nữa");
        assert_eq!(fallbacks(&sink), vec![("bad_structure".into(), 1), ("bad_structure".into(), 2)]);
        let v5 = &rec.plans[4];
        assert_eq!((v5.revise_mode, v5.fallback_reason.as_deref()), (ReviseKind::Full, None));
        assert!(gate_results(&sink).is_empty());
    }

    #[tokio::test]
    async fn gate_conflict_with_rounds_left_runs_one_full_revise_then_rechecks() {
        let conflict = "- [s3, s6] Changelog nói dùng queue nhưng Data Flow không có";
        let sc = SecScenario {
            gate: Arc::new(move |n| completion(if n == 1 { conflict } else { "OK" })),
            ..SecScenario::default()
        };
        let (rec, sink, seen) = run_section(sc, |c| c.max_rounds = 5).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.plans.len(), 4, "1 vòng viết lại toàn bộ để sửa mâu thuẫn");
        assert_eq!(rec.rounds.len(), 4, "vòng sửa mâu thuẫn tính vào max_rounds");
        assert_eq!(rec.plans[3].revise_mode, ReviseKind::Full);
        let s = seen.lock().unwrap();
        assert_eq!(s.conflict_refines, 1);
        assert_eq!(s.gate.len(), 2);
        let results = gate_results(&sink);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "conflicts");
        assert_eq!(results[0].1, vec![ConsistencyConflict { sections: vec!["s3".into(), "s6".into()], text: "Changelog nói dùng queue nhưng Data Flow không có".into() }]);
        assert_eq!(results[1].0, "ok");
    }

    #[tokio::test]
    async fn gate_conflict_on_last_round_is_unresolved() {
        let sc = SecScenario { gate: Arc::new(|_| completion("- [s3] Mâu thuẫn TTL")), ..SecScenario::default() };
        let (rec, sink, _) = run_section(sc, |c| c.max_rounds = 3).await;
        assert_eq!(rec.status, SessionStatus::Unresolved, "{:?}", rec.error);
        assert!(rec.rounds.last().unwrap().converged, "reviewer đều APPROVE nhưng không được Converged");
        let fin = rec.final_markdown.unwrap();
        assert!(fin.contains("Phiên dừng sớm") && fin.contains("Mâu thuẫn giữa các section") && fin.contains("- [s3] Mâu thuẫn TTL"), "{fin}");
        assert_eq!(gate_results(&sink)[0].0, "conflicts");
        assert!(matches!(sink.0.lock().unwrap().last().unwrap().event, CouncilEvent::Finished { status: SessionStatus::Unresolved, .. }));
    }

    #[tokio::test]
    async fn malformed_gate_reply_is_retried_once() {
        let sc = SecScenario {
            gate: Arc::new(|n| completion(if n == 1 { "Mình thấy plan ổn, không có mâu thuẫn." } else { "OK" })),
            ..SecScenario::default()
        };
        let (rec, sink, seen) = run_section(sc, |_| {}).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        {
            let s = seen.lock().unwrap();
            assert_eq!(s.gate.len(), 2);
            assert!(s.gate[1].contains("INVALID FORMAT") && s.gate[1].contains("Mình thấy plan ổn"));
        }
        assert_eq!(gate_results(&sink), vec![("ok".to_owned(), vec![])]);
        assert_eq!(rec.usage.by_kind["consistency"].calls, 2);

        // Sai định dạng cả 2 lần ở vòng cuối → unverified → Unresolved.
        let sc = SecScenario { gate: Arc::new(|_| completion("Không có gì")), ..SecScenario::default() };
        let (rec, sink, seen) = run_section(sc, |c| c.max_rounds = 3).await;
        assert_eq!(rec.status, SessionStatus::Unresolved);
        assert_eq!(seen.lock().unwrap().gate.len(), 2, "không còn vòng → không chạy lại gate");
        assert_eq!(gate_results(&sink)[0].0, "unverified");
        assert!(rec.final_markdown.unwrap().contains("Không xác minh được"));

        // Còn vòng → chạy lại gate 1 lần nữa (tối đa 4 lần gọi), vẫn unverified → Unresolved.
        let sc = SecScenario { gate: Arc::new(|_| completion("Không có gì")), ..SecScenario::default() };
        let (rec, sink, seen) = run_section(sc, |c| c.max_rounds = 5).await;
        assert_eq!(rec.status, SessionStatus::Unresolved);
        assert_eq!(seen.lock().unwrap().gate.len(), 4);
        assert_eq!(gate_results(&sink).len(), 2);
    }

    #[tokio::test]
    async fn gate_budget_denial_is_unverified_not_converged() {
        // Review của v3 báo usage cực lớn → vượt ngân sách mềm → không giữ được chỗ cho kiểm tra nhất quán.
        let sc = SecScenario {
            review_usage: Arc::new(|v| if v == 3 { (400_000, 0) } else { (100, 20) }),
            ..SecScenario::default()
        };
        let (rec, sink, seen) = run_section(sc, |c| {
            c.max_rounds = 3;
            c.meeting.max_total_tokens = Some(1_000_000);
        })
        .await;
        assert_eq!(rec.status, SessionStatus::Unresolved, "{:?}", rec.error);
        assert!(seen.lock().unwrap().gate.is_empty(), "không gọi model khi hết ngân sách");
        assert_eq!(gate_results(&sink), vec![("unverified".to_owned(), vec![])]);
        assert!(truncations(&sink).contains(&("architect".into(), "consistency".into(), false, "budget".into())));
    }

    #[tokio::test]
    async fn gate_uses_configured_reviewer_as_checker() {
        let (rec, _, _) = run_section(SecScenario::default(), |c| c.meeting.consistency_checker = "pragmatic_reviewer".into()).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.usage.by_kind["consistency"].calls, 1);
        let by_role = &rec.usage.by_role["pragmatic_reviewer"];
        assert_eq!(by_role.calls, 4, "3 review + 1 kiểm tra nhất quán");
    }

    #[tokio::test]
    async fn gate_is_skipped_without_section_mode_round() {
        // Mặc định (full): không prompt section, không kiểm tra nhất quán, không event mới.
        let (rec, sink, seen) = run_section(SecScenario::default(), |c| c.meeting.revise_mode = ReviseMode::Full).await;
        assert_eq!(rec.status, SessionStatus::Converged);
        let s = seen.lock().unwrap();
        assert!(s.section.is_empty() && s.gate.is_empty());
        let types = sink.types();
        assert!(!types.iter().any(|t| t == "consistency" || t == "revise_fallback"));
        assert!(rec.plans.iter().skip(1).all(|p| p.revise_mode == ReviseKind::Full && p.fallback_reason.is_none()));
        assert_eq!(rec.plans[0].revise_mode, ReviseKind::Initial);
    }

    #[tokio::test]
    async fn section_mode_skips_ratio_check() {
        // revise_mode = section: chỉ fallback khi patch hỏng → vẫn gọi section revise dù tỉ lệ vượt ngưỡng.
        let (rec, sink, seen) = run_section(SecScenario::default(), |c| {
            c.meeting.revise_mode = ReviseMode::Section;
            c.meeting.section_mode_max_ratio = 0.1;
        })
        .await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(seen.lock().unwrap().section.len(), 1);
        assert!(fallbacks(&sink).is_empty());
        assert_eq!(rec.plans[2].revise_mode, ReviseKind::Section);
    }

    // ── checkpoint / rerun / usage từng lần gọi ──

    #[derive(Default)]
    struct CollectCheckpoint(Mutex<Vec<SessionRecord>>);
    impl Checkpoint for CollectCheckpoint {
        fn save(&self, record: SessionRecord) {
            self.0.lock().unwrap().push(record);
        }
    }

    #[tokio::test]
    async fn checkpoints_are_written_after_each_plan_and_round() {
        let cp = Arc::new(CollectCheckpoint::default());
        let extras = SessionExtras { checkpoint: Some(cp.clone()), ..SessionExtras::default() };
        let (rec, _) = run_with_extras(standard_router(false), |_| {}, None, extras).await;
        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        let cps = cp.0.lock().unwrap();
        assert!(cps.iter().all(|c| c.status == SessionStatus::Running && c.id == rec.id && c.finished_at.is_none()));
        let shape: Vec<(usize, usize)> = cps.iter().map(|c| (c.plans.len(), c.rounds.len())).collect();
        // bắt đầu → v1 → round 1 → v2 → round 2
        assert_eq!(shape, vec![(0, 0), (1, 0), (1, 1), (2, 1), (2, 2)]);
        let after_round_one = &cps[2];
        assert_eq!(after_round_one.rounds[0].slots.len(), 3);
        assert_eq!(after_round_one.prompt, rec.prompt);
        // Chỉ phiên bản đã hoàn tất (không có phần stream dở) + usage cập nhật tới thời điểm đó.
        assert_eq!(after_round_one.plans[0].markdown, rec.plans[0].markdown);
        assert_eq!(after_round_one.usage.total.calls, 4);
        assert_eq!(after_round_one.calls.len(), 4);
    }

    #[tokio::test]
    async fn department_rounds_are_checkpointed_too() {
        let cp = Arc::new(CollectCheckpoint::default());
        let extras = SessionExtras { checkpoint: Some(cp.clone()), ..SessionExtras::default() };
        let (rec, _) = run_with_extras(standard_router(false), |c| c.meeting.department_ids = vec!["qa".into()], None, extras).await;
        assert_eq!(rec.departments.len(), 1, "{:?}", rec.error);
        let cps = cp.0.lock().unwrap();
        let first_dept = cps.iter().find(|c| !c.departments.is_empty()).unwrap();
        assert_eq!(first_dept.departments[0].id, "qa");
        assert_eq!(first_dept.departments[0].status, SessionStatus::Running);
        assert!(first_dept.plans.is_empty(), "cuộc họp chính chưa bắt đầu");
        // Sau khi phòng ban xong, checkpoint của cuộc họp chính mang trạng thái thật của phòng ban.
        let last = cps.last().unwrap();
        assert_eq!(last.departments[0].status, rec.departments[0].status);
    }

    #[tokio::test]
    async fn rerun_starts_at_review_of_the_seeded_version() {
        let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
        let b2 = bodies.clone();
        let handler: Handler = Arc::new(move |body: &Value| {
            b2.lock().unwrap().push(body.clone());
            if is_author(body) {
                return completion(&plan_md(3));
            }
            if plan_version_of(body) == 2 { completion(&reject()) } else { completion(&approve()) }
        });
        let base = spawn_mock(handler).await;
        let cfg = cfg_for(&base, false);
        let parent = crate::rerun::tests::parent_record(&cfg);
        let plan = crate::rerun::plan_rerun(&cfg, &parent, MAIN_SCOPE, 2, &crate::rerun::RerunOverrides::default()).unwrap();
        assert!(plan.blocking.is_empty());
        let sink = Arc::new(CollectSink::default());
        let llm = LlmClient::new(crate::llm_client::build_http_client().unwrap(), &plan.cfg);
        let extras = SessionExtras { seed: plan.seed.clone(), parent: Some(plan.parent.clone()), ..SessionExtras::default() };
        let council = Council::with_extras(llm, plan.cfg.clone(), sink.clone(), Uuid::new_v4(), CancellationToken::new(), extras);
        let rec = council.run(plan.prompt.clone()).await;

        assert_eq!(rec.status, SessionStatus::Converged, "{:?}", rec.error);
        assert_eq!(rec.plans.iter().map(|p| p.version).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(rec.plans[0].markdown, crate::rerun::tests::TAGGED);
        assert_eq!(rec.rounds.iter().map(|r| (r.round, r.plan_version)).collect::<Vec<_>>(), vec![(1, 2), (2, 3)]);
        assert_eq!(rec.parent, Some(ParentRef { session_id: parent.id, scope: "main".into(), version_no: 2 }));
        assert_eq!(rec.summary().parent, rec.parent);

        let bodies = bodies.lock().unwrap();
        // Không có bản nháp: lần gọi đầu tiên là review của v2.
        assert!(!is_author(&bodies[0]), "lần gọi đầu phải là review");
        let first_author = bodies.iter().position(is_author).unwrap();
        assert!(first_author >= 3, "3 reviewer review v2 trước khi người viết sửa");
        let scale_v2 = bodies
            .iter()
            .find(|b| system_of(b).contains("[PLAN REVIEWER]") && all_text(b).contains("Issue mở của scale_reviewer"))
            .unwrap();
        let t = all_text(scale_v2);
        assert!(t.contains("<plan version=\"2\""), "{t}");
        assert!(t.contains("Shard theo user"), "plan gieo nằm trong body review");
        assert!(t.contains("Issue mở của scale_reviewer ở v2"), "issue còn mở được gửi lại cho reviewer");
        // Người viết sửa từ đúng plan gieo.
        assert!(all_text(&bodies[first_author]).contains("Shard theo user"));
        // Event: PlanReady v2 trước review đầu tiên.
        let types = sink.types();
        let ready = types.iter().position(|t| t == "plan_ready").unwrap();
        let first_review = types.iter().position(|t| t == "review_ready").unwrap();
        assert!(ready < first_review);
    }

    #[tokio::test]
    async fn calls_are_recorded_with_cached_tokens_and_live_cost() {
        let handler: Handler = Arc::new(|body: &Value| {
            let content = if is_author(body) { plan_md(1) } else if plan_version_of(body) == 1 { reject() } else { approve() };
            let resp = json!({
                "choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":1000,"completion_tokens":100,"prompt_tokens_details":{"cached_tokens":400}}
            });
            MockResp::http(200, "application/json", resp.to_string())
        });
        let (rec, sink) = run_with(
            handler,
            |c| {
                c.pricing.insert("gpt-4.1-mini".into(), crate::config::ModelPrice { input_per_m: 1.0, cached_per_m: 0.5, output_per_m: 4.0 });
            },
            None,
        )
        .await;
        assert_eq!(rec.calls.len() as u32, rec.usage.total.calls);
        assert_eq!(rec.usage.total.cached_tokens, 400 * u64::from(rec.usage.total.calls));
        let c0 = &rec.calls[0];
        assert_eq!((c0.call_id, c0.kind.as_str(), c0.round, c0.model.as_str()), (0, "plan", 0, "gpt-4.1-mini"));
        assert_eq!((c0.prompt, c0.cached, c0.completion, c0.estimated, c0.finish.as_str()), (1000, 400, 100, false, "stop"));
        assert!(rec.calls.iter().any(|c| c.kind == "review" && c.round == 2));
        assert!(rec.calls.windows(2).all(|w| w[0].call_id < w[1].call_id));
        // Mỗi lần gọi: 600 × 1 + 400 × 0.5 + 100 × 4 = 1200 µUSD.
        let per_call = 0.0012;
        let last = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::UsageUpdated { cost_usd, unpriced_models, cached_tokens, .. } => Some((*cost_usd, unpriced_models.clone(), *cached_tokens)),
                _ => None,
            })
            .next_back()
            .unwrap();
        let expect = per_call * f64::from(rec.usage.total.calls);
        assert!((last.0.unwrap() - expect).abs() < 1e-9, "{:?} vs {expect}", last.0);
        assert!(last.1.is_empty() && last.2 == 400);
        // Đọc lại bằng get_usage với bảng giá rỗng → null + model chưa có giá.
        let u = crate::cost::usage_breakdown(&rec, &Default::default());
        assert_eq!(u.totals.cost_usd, None);
        assert_eq!(u.unpriced_models, vec!["gpt-4.1-mini"]);
        assert_eq!(u.by_round.first().map(|r| r.round), Some(0));
    }

    #[tokio::test]
    async fn live_cost_is_null_without_pricing() {
        let (_, sink) = run_council(standard_router(false), false, None).await;
        let events = sink.0.lock().unwrap();
        let (cost, unpriced) = events
            .iter()
            .filter_map(|e| match &e.event {
                CouncilEvent::UsageUpdated { cost_usd, unpriced_models, .. } => Some((*cost_usd, unpriced_models.clone())),
                _ => None,
            })
            .next_back()
            .unwrap();
        assert_eq!(cost, None);
        assert_eq!(unpriced, vec!["gpt-4.1-mini"]);
    }
}
