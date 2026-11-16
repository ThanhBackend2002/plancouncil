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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use serde_json::{json, Value};

use crate::config::{AppConfig, Department, RoleDef, ToolPolicy};
use crate::llm_client::{unwrap_markdown_fence, ChatMessage, Expect, LlmClient, LlmError, StreamSignal, Usage};
use crate::models::*;
use crate::prompts::{self, CompanyMode, PlanContext, PromptOpts};
use crate::{routing, staffing};
use crate::tools::Toolbox;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAction {
    Invite { scope: String, role_id: String },
    Dismiss { scope: String, role_id: String },
}

/// Hàng đợi lệnh điều khiển thành viên từ UI → orchestrator (đọc ở đầu mỗi vòng review).
#[derive(Debug, Default)]
pub struct MeetingControl {
    pending: Mutex<Vec<ControlAction>>,
}

impl MeetingControl {
    pub fn push(&self, a: ControlAction) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).push(a);
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
        let permits = cfg.max_parallel_calls.clamp(1, 16) as usize;
        Self {
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
        }
    }

    fn prompt_opts(&self) -> PromptOpts {
        let m = &self.cfg.meeting;
        PromptOpts { depth: m.plan_depth, routing: m.section_routing, strict_first_round: m.strict_first_round }
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

    /// Ghi token của 1 lần gọi; thiếu `usage` → ước lượng theo độ dài prompt/output.
    fn record_usage(&self, scope: &str, agent: &str, kind: &str, usage: Option<Usage>, prompt_chars: usize, output_chars: usize) {
        let (p, c, estimated) = match usage {
            Some(u) if u.prompt_tokens + u.completion_tokens > 0 => (u.prompt_tokens, u.completion_tokens, false),
            _ => (estimate_tokens(prompt_chars), estimate_tokens(output_chars), true),
        };
        let total = {
            let mut r = self.usage.lock().unwrap_or_else(|e| e.into_inner());
            r.total.add(p, c, estimated);
            r.by_role.entry(agent.to_owned()).or_default().add(p, c, estimated);
            r.by_scope.entry(scope.to_owned()).or_default().add(p, c, estimated);
            r.by_kind.entry(kind.to_owned()).or_default().add(p, c, estimated);
            r.total
        };
        self.emitter.send(CouncilEvent::UsageUpdated {
            scope: scope.to_owned(),
            agent: agent.to_owned(),
            kind: kind.to_owned(),
            prompt_tokens: p,
            completion_tokens: c,
            estimated,
            total,
        });
    }

    // ── bối cảnh ──

    /// Gắn bối cảnh công ty vào system prompt và dữ liệu bổ sung vào đầu user message.
    fn decorate(&self, mut msgs: Vec<ChatMessage>, notes: Option<&str>, overview: bool) -> Vec<ChatMessage> {
        if let Some(sys) = msgs.first_mut().filter(|m| m.role == "system") {
            sys.content.push_str(&prompts::company_block(&self.extras.company));
        }
        let prefix = prompts::context_prefix(notes, if overview { self.extras.overview.as_deref() } else { None });
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
        let max_steps = self.cfg.max_tool_steps.clamp(1, 12);
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
            let allow = used < max_steps;
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
            self.record_usage(scope, &role.id, "research", turn.usage, chars(&msgs), out_chars);

            if turn.tool_calls.is_empty() || !allow {
                let notes = turn.content.unwrap_or_default();
                let notes = notes.trim();
                return Ok((!notes.is_empty() && !notes.starts_with(prompts::NO_RESEARCH)).then(|| truncate_chars(notes, 6_000)));
            }
            msgs.push(turn.assistant_message.clone());
            for call in &turn.tool_calls {
                used += 1;
                let (result, summary, ok) = if used <= max_steps {
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
            if let Ok((info, usage, pc, oc)) =
                staffing::propose(&self.llm, &self.cfg, &prompt, &self.extras.company, &self.cancel).await
            {
                let m = &mut self.cfg.meeting;
                m.chair_id = info.chair_id.clone();
                m.reviewer_ids = info.reviewer_ids.clone();
                m.department_ids = info.department_ids.clone();
                staffing = Some((info, usage, pc, oc));
            }
        }

        let m = &self.cfg.meeting;
        let mut record = SessionRecord::new(self.emitter.session_id, prompt.clone(), self.cfg.max_rounds);
        record.chair_id = m.chair_id.clone();
        record.reviewer_ids = m.reviewer_ids.clone();
        record.cast = self.cast();
        record.options = self.extras.info.clone();

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
        if let Some((info, usage, pc, oc)) = staffing {
            self.record_usage(MAIN_SCOPE, &info.chair_id, "staffing", usage, pc, oc);
            self.emitter.send(CouncilEvent::StaffingDecided { staffing: info.clone() });
            record.staffing = Some(info);
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
        record.activities = std::mem::take(&mut *self.activities.lock().unwrap_or_else(|e| e.into_inner()));
        record.membership = std::mem::take(&mut *self.membership.lock().unwrap_or_else(|e| e.into_inner()));
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

        let status = res.status.unwrap_or(SessionStatus::MaxRoundsReached);
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

        if let Some(ep) = self.cfg.resolve(&chair.id) {
            self.emitter.send(CouncilEvent::AgentThinking {
                scope: MAIN_SCOPE.into(),
                agent: chair.id.clone(),
                round: 0,
                plan_version: 0,
            });
            let msgs = self.decorate(prompts::assignment_messages(chair, prompt, depts, self.cfg.meeting.plan_depth), None, false);
            let result = {
                let _permit = self.permit().await?;
                self.llm.chat_json::<RawAssignments>(&ep, &msgs, &self.cancel).await
            };
            match result {
                Ok((raw, text, usage)) => {
                    self.record_usage(MAIN_SCOPE, &chair.id, "assign", usage, msg_chars(&msgs), text.len());
                    for a in raw.assignments {
                        let task = a.task.trim();
                        if !task.is_empty() && depts.iter().any(|d| d.id == a.department_id) {
                            tasks.insert(a.department_id, truncate_chars(task, 2_000));
                        }
                    }
                }
                Err(LlmError::Cancelled) => return Err(Halt::Cancelled),
                Err(e) => log::warn!("chủ trì giao việc lỗi ({e}); phòng ban nhận nguyên yêu cầu"),
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

        // Người viết plan nghiên cứu (đọc workspace / tìm web) 1 lần trước bản nháp; ghi chú dùng lại khi chỉnh sửa.
        let notes = if self.may_research(true) {
            let purpose = match spec.ctx {
                PlanContext::Main => "write the plan for the request",
                PlanContext::Department { .. } => "write your department's plan for the assigned task",
            };
            self.research(spec.scope, spec.author, "author", 1, purpose, &spec.ctx, request, None).await?
        } else {
            None
        };
        let draft = self.decorate(
            prompts::author_draft_messages(spec.author, &spec.ctx, request, &spec.reports, &reviewers, &opts),
            notes.as_deref(),
            true,
        );
        let v1 = self.call_author(spec.scope, spec.author, 1, &draft).await?;
        if opts.routing && !reviewers.is_empty() && !routing::is_tagged(&v1.markdown) {
            log::info!("[{}] {} không gắn tag reviewer cho section → reviewer đọc toàn bộ plan", spec.scope, spec.author.name);
        }
        out.plans.push(v1);

        if reviewers.is_empty() {
            // Phòng ban không có thành viên phản biện → plan của trưởng phòng là kết quả.
            out.status = Some(SessionStatus::Converged);
            out.final_markdown = out.plans.last().map(|p| p.markdown.clone());
            return Ok(());
        }

        let mut previous: HashMap<String, Review> = HashMap::new();
        let mut last_fp: HashMap<String, u64> = HashMap::new();
        // (role_id, lý do, người đề nghị) — áp dụng ở đầu vòng kế tiếp
        let mut proposed: Vec<(String, String, String)> = Vec::new();

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
                self.finish_loop(out, SessionStatus::Converged);
                return Ok(());
            }

            // ── 2. Review (song song; giữ nguyên APPROVE khi phần phụ trách không đổi) ──
            let plan = out.plans.last().cloned().expect("có ít nhất 1 plan");
            let available = if invites_left > 0 { self.available_experts(spec.author, &reviewers) } else { vec![] };
            let (available, reviewed_once_ref, invited_ref, previous_ref, last_fp_ref, plan_ref) =
                (&available, &reviewed_once, &invited, &previous, &last_fp, &plan);
            let results = join_all(reviewers.iter().map(|r| async move {
                let fp = routing::fingerprint(&plan_ref.markdown, &r.id);
                let prev = previous_ref.get(&r.id);
                let unchanged = last_fp_ref.get(&r.id) == Some(&fp);
                if opts.routing && unchanged && prev.is_some_and(|p| p.verdict == Verdict::Approve) {
                    return Ok((carried(prev.expect("vừa kiểm tra")), fp, true));
                }
                let rc = prompts::ReviewCtx {
                    round,
                    first_review: !reviewed_once_ref.contains(&r.id),
                    invited: invited_ref.contains(&r.id),
                    routed: false,
                    available,
                };
                self.call_reviewer(spec, r, rc, request, plan_ref, prev).await.map(|rv| (rv, fp, false))
            }))
            .await;
            if self.cancel.is_cancelled() {
                return Err(Halt::Cancelled);
            }

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

            let status = if converged {
                Some(SessionStatus::Converged)
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

            let last_round = out.rounds.last().expect("vừa push");
            let msgs = self.decorate(
                prompts::author_refine_messages(spec.author, &spec.ctx, request, &plan, last_round, &reviewers, &opts),
                notes.as_deref(),
                false,
            );
            let next = self.call_author(spec.scope, spec.author, plan.version + 1, &msgs).await?;
            out.plans.push(next);
        }
        self.finish_loop(out, SessionStatus::MaxRoundsReached);
        Ok(())
    }

    fn finish_loop(&self, out: &mut LoopResult, status: SessionStatus) {
        let mut final_md = out.plans.last().map(|p| p.markdown.clone()).unwrap_or_default();
        if matches!(status, SessionStatus::MaxRoundsReached | SessionStatus::PartiallyApproved) {
            if let Some(r) = out.rounds.last() {
                final_md.push_str(&prompts::open_issues_appendix(r, |id| self.name_of(id)));
            }
        }
        out.status = Some(status);
        out.final_markdown = Some(final_md);
    }

    async fn call_author(
        &self,
        scope: &str,
        author: &RoleDef,
        version: u32,
        messages: &[ChatMessage],
    ) -> Result<PlanVersion, Halt> {
        let ep = self.cfg.resolve(&author.id).ok_or_else(|| {
            Halt::Failed(format!("role '{}' không tồn tại", author.id))
        })?;
        let _permit = self.permit().await?;
        self.emitter.send(CouncilEvent::AgentThinking {
            scope: scope.to_owned(),
            agent: author.id.clone(),
            round: version,
            plan_version: version,
        });

        let raw = if self.cfg.stream_plans {
            let mut pending = String::new();
            let mut last_flush = Instant::now();
            let emitter = &self.emitter;
            let completion = self
                .llm
                .chat_stream(&ep, messages, &self.cancel, |sig| match sig {
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
                })
                .await?;
            if !pending.is_empty() {
                self.emitter.send(CouncilEvent::PlanDelta {
                    scope: scope.to_owned(),
                    version,
                    delta: pending,
                });
            }
            completion
        } else {
            self.llm.chat(&ep, messages, Expect::Text, &self.cancel).await?
        };
        self.record_usage(scope, &author.id, "plan", raw.usage, msg_chars(messages), raw.text.len());

        let markdown = unwrap_markdown_fence(&raw.text);
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
        Ok(PlanVersion {
            version,
            markdown,
            created_at: Utc::now(),
        })
    }

    async fn call_reviewer(
        &self,
        spec: &LoopSpec<'_>,
        reviewer: &RoleDef,
        mut rc: prompts::ReviewCtx<'_>,
        request: &str,
        plan: &PlanVersion,
        previous: Option<&Review>,
    ) -> Result<Review, LlmError> {
        let round = rc.round;
        let opts = self.prompt_opts();
        let ep = self
            .cfg
            .resolve(&reviewer.id)
            .ok_or_else(|| LlmError::InvalidResponse(format!("role '{}' không tồn tại", reviewer.id)))?;

        // Chính sách "Tất cả": reviewer tự kiểm chứng (1 lần ở round 1), ghi chú dùng lại các round sau.
        let note_key = format!("{}/{}", spec.scope, reviewer.id);
        if round == 1 && self.may_research(false) {
            match self
                .research(spec.scope, reviewer, "review", round, "review the plan below from your area of expertise", &spec.ctx, request, Some(plan))
                .await
            {
                Ok(Some(n)) => {
                    self.reviewer_notes.lock().unwrap_or_else(|e| e.into_inner()).insert(note_key.clone(), n);
                }
                Ok(None) | Err(Halt::Failed(_)) => {}
                Err(Halt::Cancelled) => return Err(LlmError::Cancelled),
            }
        }
        let notes = self.reviewer_notes.lock().unwrap_or_else(|e| e.into_inner()).get(&note_key).cloned();

        let _permit = self.permit().await?;
        self.emitter.send(CouncilEvent::AgentThinking {
            scope: spec.scope.to_owned(),
            agent: reviewer.id.clone(),
            round,
            plan_version: plan.version,
        });
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
        if let Some(v) = &routed_view {
            rc.routed = true;
            self.record_saving(spec.scope, &reviewer.id, estimate_tokens(plan.markdown.len().saturating_sub(v.len())), false);
        }
        let view = routed_view.as_deref().unwrap_or(&plan.markdown);
        let strict = opts.strict_first_round && round == 1 && !rc.invited;
        let mut messages = self.decorate(
            prompts::reviewer_messages(reviewer, &spec.author.name, &spec.ctx, &rc, &opts, request, plan.version, view, previous),
            notes.as_deref(),
            false,
        );

        let (raw, raw_text, usage) = self.llm.chat_json::<RawReview>(&ep, &messages, &self.cancel).await?;
        self.record_usage(spec.scope, &reviewer.id, "review", usage, msg_chars(&messages), raw_text.len());
        let mut review = Review::from_raw(&reviewer.id, &reviewer.name, raw);

        if strict && review.violates_round_one_policy() {
            // Cho model 1 cơ hội tự sửa (rẻ hơn & trung thực hơn việc server tự bịa issue).
            log::info!("{} vi phạm round-1 policy; yêu cầu review lại", reviewer.name);
            messages.push(ChatMessage::assistant(raw_text));
            messages.push(ChatMessage::user(prompts::ROUND_ONE_CORRECTION));
            match self.llm.chat_json::<RawReview>(&ep, &messages, &self.cancel).await {
                Ok((raw2, text2, usage2)) => {
                    self.record_usage(spec.scope, &reviewer.id, "review", usage2, msg_chars(&messages), text2.len());
                    review = Review::from_raw(&reviewer.id, &reviewer.name, raw2);
                }
                Err(LlmError::Cancelled) => return Err(LlmError::Cancelled),
                Err(e) => log::warn!("{}: correction call lỗi ({e}); giữ review gốc", reviewer.name),
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
    use crate::test_support::{completion, spawn_mock, sse, tool_call, Handler, MockResp};

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
            info: SessionInfo { workspace: Some("game".into()), web_search: false, company_context: true },
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
}
