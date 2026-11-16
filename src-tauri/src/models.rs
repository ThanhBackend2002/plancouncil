//! Domain models + IPC contract (Rust ⇄ React).
//!
//! Mọi struct ở đây là "source of truth" cho `src/lib/contracts.ts` phía frontend.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

pub const SUMMARY_BUBBLE_MAX_WORDS: usize = 15;
/// Giới hạn để feedback không làm phình context của người viết plan qua từng vòng.
pub const MAX_LIST_ITEMS: usize = 10;
pub const MAX_ITEM_CHARS: usize = 600;

/// Scope của cuộc họp chính; các phòng ban dùng id của phòng ban làm scope.
pub const MAIN_SCOPE: &str = "main";

/// Id của role (do người dùng quản lý trong config).
pub type RoleId = String;

// ───────────────────────────── Review ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Approve,
    Reject,
    /// Reviewer tự nhận plan không thuộc chuyên môn → rời cuộc họp, không tính vào hội tụ.
    Abstain,
}

/// Deserialize "khoan dung": LLM hay trả `approve`, `APPROVED`, ` Reject `, `abstain`...
impl<'de> Deserialize<'de> for Verdict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        let norm = raw.trim().to_ascii_uppercase();
        if norm.starts_with("APPROV") {
            Ok(Verdict::Approve)
        } else if norm.starts_with("REJECT") {
            Ok(Verdict::Reject)
        } else if norm.starts_with("ABSTAIN") || norm.starts_with("NOT_RELEVANT") || norm == "SKIP" {
            Ok(Verdict::Abstain)
        } else {
            Err(serde::de::Error::custom(format!(
                "invalid verdict {raw:?}, expected \"APPROVE\" | \"REJECT\" | \"ABSTAIN\""
            )))
        }
    }
}

/// Đề nghị mời thêm 1 chuyên gia vào cuộc họp (từ danh sách `available_experts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InviteRequest {
    pub role_id: RoleId,
    #[serde(default)]
    pub reason: String,
}

/// Chấp nhận `["dba"]`, `[{"role_id":"dba","reason":"…"}]`, `null`.
fn flexible_invites<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<InviteRequest>, D::Error> {
    use serde_json::Value;
    let v = Option::<Value>::deserialize(d)?;
    let items = match v {
        Some(Value::Array(a)) => a,
        Some(Value::Null) | None => vec![],
        Some(other) => vec![other],
    };
    Ok(items
        .into_iter()
        .filter_map(|it| match it {
            Value::String(s) => Some(InviteRequest { role_id: s.trim().to_owned(), reason: String::new() }),
            Value::Object(m) => Some(InviteRequest {
                role_id: m.get("role_id").or_else(|| m.get("id")).and_then(Value::as_str)?.trim().to_owned(),
                reason: m.get("reason").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
            }),
            _ => None,
        })
        .filter(|i| !i.role_id.is_empty())
        .take(3)
        .collect())
}

/// Đúng schema LLM được yêu cầu trả về. Mọi field trừ `verdict` đều optional.
#[derive(Debug, Deserialize)]
pub struct RawReview {
    /// Nhận để khớp schema nhưng không dùng — role do server gán.
    #[serde(default)]
    #[allow(dead_code)]
    pub role: Option<String>,
    pub verdict: Verdict,
    #[serde(default)]
    pub summary_bubble: Option<String>,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub critical_issues: Vec<String>,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub suggestions: Vec<String>,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub out_of_scope: Vec<String>,
    #[serde(default, deserialize_with = "flexible_invites")]
    pub invite: Vec<InviteRequest>,
    /// Số hiệu heading mà critical_issues nhắc tới ("3.3", "2"); đã chuẩn hoá + bỏ trùng.
    #[serde(default, alias = "section", deserialize_with = "flexible_sections")]
    pub sections: Vec<String>,
}

/// Rút số hiệu heading từ 1 chuỗi: "§3.3", "Section 3.3", "3.3.", "mục 3.3", "3.3 Startup" → "3.3".
/// Không có chữ số → `None`.
pub fn normalize_section(s: &str) -> Option<String> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let run: String = s[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    // Bỏ dấu chấm thừa ở cuối ("3." → "3") và gộp ".." lỡ tay.
    let parts: Vec<&str> = run.split('.').filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("."))
}

/// Chấp nhận `null`, `"3.3"`, `3`, `["§3.3", 2, "mục 4.1"]`, `"3.3, 4.2"`, `[{"section":"3.3"}]`.
fn flexible_sections<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    use serde_json::Value;
    fn collect(v: Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                // "3.3, 4.2" / "3.3; 4.2" → nhiều mục; mỗi mục lấy số hiệu đầu tiên.
                out.extend(s.split([',', ';']).filter_map(normalize_section));
            }
            Value::Number(n) => out.extend(normalize_section(&n.to_string())),
            Value::Array(a) => a.into_iter().for_each(|it| collect(it, out)),
            Value::Object(m) => m.into_iter().for_each(|(_, it)| collect(it, out)),
            Value::Null | Value::Bool(_) => {}
        }
    }
    let mut raw = Vec::new();
    if let Some(v) = Option::<Value>::deserialize(d)? {
        collect(v, &mut raw);
    }
    let mut out: Vec<String> = Vec::new();
    for s in raw {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out.truncate(MAX_LIST_ITEMS);
    Ok(out)
}

/// Chấp nhận `null`, `"a string"`, `["a", {"issue": "b"}]`.
pub fn flexible_string_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    use serde_json::Value;
    let v = Option::<Value>::deserialize(d)?;
    let items = match v {
        None | Some(Value::Null) => vec![],
        Some(Value::String(s)) => vec![Value::String(s)],
        Some(Value::Array(a)) => a,
        Some(other) => vec![other],
    };
    Ok(items
        .into_iter()
        .filter_map(|it| match it {
            Value::String(s) => Some(s),
            Value::Null => None,
            Value::Object(map) => {
                let parts: Vec<String> = map
                    .values()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                (!parts.is_empty()).then(|| parts.join(" — "))
            }
            other => Some(other.to_string()),
        })
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect())
}

/// Review đã được validate/sanitize — thứ duy nhất đi ra Frontend và vào lịch sử.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    /// Id của role đã review.
    pub agent: RoleId,
    /// Tên hiển thị của role tại thời điểm review.
    pub role: String,
    pub verdict: Verdict,
    pub summary_bubble: String,
    pub critical_issues: Vec<String>,
    pub suggestions: Vec<String>,
    /// Ý tưởng vượt phạm vi yêu cầu — ghi nhận nhưng không phải lý do REJECT.
    #[serde(default)]
    pub out_of_scope: Vec<String>,
    /// Chuyên gia reviewer đề nghị mời thêm.
    #[serde(default)]
    pub invites: Vec<InviteRequest>,
    /// Số hiệu heading (vd "3.3", "2") mà critical_issues nhắc tới. Phiên cũ không có field → rỗng.
    #[serde(default)]
    pub sections: Vec<String>,
    /// true = không gọi model: phần reviewer phụ trách không đổi nên giữ nguyên APPROVE vòng trước.
    #[serde(default)]
    pub carried_over: bool,
    /// Khác `None` khi server-side policy đã can thiệp (vd: ép REJECT ở Round 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_override: Option<String>,
}

impl Review {
    pub fn from_raw(agent: &str, display_name: &str, raw: RawReview) -> Self {
        let clamp = |v: Vec<String>| -> Vec<String> {
            v.into_iter()
                .take(MAX_LIST_ITEMS)
                .map(|s| truncate_chars(&s, MAX_ITEM_CHARS))
                .collect()
        };
        let critical_issues = clamp(raw.critical_issues);
        let suggestions = clamp(raw.suggestions);
        let out_of_scope = clamp(raw.out_of_scope);

        let bubble_src = raw
            .summary_bubble
            .filter(|s| !s.trim().is_empty())
            .or_else(|| critical_issues.first().cloned())
            .unwrap_or_else(|| match raw.verdict {
                Verdict::Approve => "Plan ổn, không còn vấn đề nghiêm trọng.".into(),
                Verdict::Reject => "Còn vấn đề cần xử lý.".into(),
                Verdict::Abstain => "Nội dung không thuộc chuyên môn của tôi, xin phép rút.".into(),
            });

        Review {
            agent: agent.to_owned(),
            // Không tin `role` do LLM tự khai — dùng tên server biết chắc.
            role: display_name.to_owned(),
            verdict: raw.verdict,
            summary_bubble: truncate_words(&bubble_src, SUMMARY_BUBBLE_MAX_WORDS),
            critical_issues,
            suggestions,
            out_of_scope,
            invites: raw.invite,
            sections: raw.sections,
            carried_over: false,
            policy_override: None,
        }
    }

    /// Round 1 (chế độ nghiêm): cấm APPROVE và bắt buộc ≥ 1 critical issue cụ thể. ABSTAIN được phép.
    pub fn violates_round_one_policy(&self) -> bool {
        match self.verdict {
            Verdict::Approve => true,
            Verdict::Reject => self.critical_issues.is_empty(),
            Verdict::Abstain => false,
        }
    }

    /// Fallback cuối cùng sau khi đã yêu cầu LLM tự sửa mà vẫn vi phạm. Không bịa issue.
    pub fn enforce_round_one_policy(&mut self) {
        if self.violates_round_one_policy() {
            self.verdict = Verdict::Reject;
            self.policy_override = Some(format!(
                "Round-1 policy: verdict bị ép thành REJECT (reviewer trả về {} critical issue, yêu cầu ≥ 1).",
                self.critical_issues.len()
            ));
        }
    }
}

// ───────────────────────────── Session ─────────────────────────────

/// Cách phiên bản plan được tạo ra.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviseKind {
    /// Bản nháp đầu (v1).
    Initial,
    /// Viết lại toàn bộ plan. Mặc định cho bản ghi cũ không có field.
    #[default]
    Full,
    /// Chỉ sửa các section liên quan rồi ghép lại (section revise).
    Section,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanVersion {
    pub version: u32,
    pub markdown: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub revise_mode: ReviseKind,
    /// Lý do section revise không dùng được → viết lại toàn bộ (vd "missing_ids", "ratio").
    #[serde(default)]
    pub fallback_reason: Option<String>,
}

impl PlanVersion {
    pub fn new(version: u32, markdown: String, revise_mode: ReviseKind) -> Self {
        Self { version, markdown, created_at: Utc::now(), revise_mode, fallback_reason: None }
    }
}

/// 1 mâu thuẫn giữa các section do bước kiểm tra nhất quán phát hiện.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsistencyConflict {
    /// Id section ("s2", "s5").
    pub sections: Vec<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSlot {
    pub agent: RoleId,
    pub review: Option<Review>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundRecord {
    pub round: u32,
    pub plan_version: u32,
    pub slots: Vec<ReviewSlot>,
    pub approvals: u32,
    pub converged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    /// Mọi reviewer APPROVE.
    Converged,
    /// Hết vòng mà vẫn còn REJECT → Final kèm "Open Issues".
    MaxRoundsReached,
    /// Reviewer còn lại đều APPROVE nhưng ≥1 reviewer lỗi API.
    PartiallyApproved,
    /// Output của người viết bị cắt (max_tokens / stream đứt) và không nối tiếp được →
    /// dừng, giữ phiên bản hợp lệ gần nhất.
    Truncated,
    /// Hết ngân sách token (giới hạn mềm) trước lần gọi kế tiếp → dừng, giữ phiên bản gần nhất.
    BudgetExhausted,
    /// Dành cho wave sau (consistency gate): còn vấn đề chưa giải quyết.
    Unresolved,
    /// Dành cho wave sau: phiên bị gián đoạn (vd. app tắt giữa chừng).
    Interrupted,
    Cancelled,
    Failed,
}

impl SessionStatus {
    /// Mức "tệ" của kết quả để gộp trạng thái phiên (lớn hơn = tệ hơn).
    pub fn severity(self) -> u8 {
        match self {
            SessionStatus::Running => 0,
            SessionStatus::Converged => 1,
            SessionStatus::PartiallyApproved => 2,
            SessionStatus::MaxRoundsReached => 3,
            SessionStatus::Unresolved => 4,
            SessionStatus::Truncated => 5,
            SessionStatus::BudgetExhausted => 6,
            SessionStatus::Interrupted => 7,
            SessionStatus::Cancelled => 8,
            SessionStatus::Failed => 9,
        }
    }

    pub fn worst(self, other: SessionStatus) -> SessionStatus {
        if other.severity() > self.severity() {
            other
        } else {
            self
        }
    }
}

/// Ảnh chụp role tại thời điểm họp — lịch sử vẫn hiển thị đúng khi role bị sửa/xoá sau này.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastMember {
    pub id: RoleId,
    pub name: String,
    pub title: String,
    pub color: String,
    pub icon: String,
    pub model: String,
    #[serde(default)]
    pub level: crate::config::RoleLevel,
    #[serde(default)]
    pub appearance: crate::config::Appearance,
}

/// Thông tin phòng ban gửi lên UI khi bắt đầu phiên.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepartmentBrief {
    pub id: String,
    pub name: String,
    pub lead_id: RoleId,
    pub member_ids: Vec<RoleId>,
    pub max_rounds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub department_id: String,
    pub task: String,
}

/// Kết quả họp nội bộ của 1 phòng ban.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepartmentRecord {
    pub id: String,
    pub name: String,
    pub task: String,
    pub lead_id: RoleId,
    pub member_ids: Vec<RoleId>,
    pub max_rounds: u32,
    pub status: SessionStatus,
    pub plans: Vec<PlanVersion>,
    pub rounds: Vec<RoundRecord>,
    pub final_markdown: Option<String>,
    pub error: Option<String>,
}

/// Token tiêu thụ cộng dồn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub calls: u32,
    /// Số lần gọi mà provider không trả `usage` → token được ước lượng theo độ dài văn bản.
    pub estimated_calls: u32,
}

impl TokenUsage {
    pub fn add(&mut self, prompt: u64, completion: u64, estimated: bool) {
        self.prompt_tokens += prompt;
        self.completion_tokens += completion;
        self.calls += 1;
        if estimated {
            self.estimated_calls += 1;
        }
    }
    pub fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// Báo cáo token của 1 phiên: tổng + theo role + theo nơi họp + theo loại việc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageReport {
    pub total: TokenUsage,
    pub by_role: std::collections::BTreeMap<RoleId, TokenUsage>,
    pub by_scope: std::collections::BTreeMap<String, TokenUsage>,
    /// "assign" | "research" | "plan" | "review" | "staffing"
    pub by_kind: std::collections::BTreeMap<String, TokenUsage>,
    /// Token ước lượng tiết kiệm được nhờ review theo section + giữ nguyên APPROVE.
    pub saved_tokens: u64,
    /// Số lượt review không cần gọi model (phần phụ trách không đổi).
    pub carried_reviews: u32,
}

/// Thay đổi thành viên trong lúc họp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipChange {
    pub scope: String,
    pub agent: RoleId,
    /// "invited" | "dismissed" | "abstained"
    pub action: String,
    /// "user" | id role đề nghị | "self"
    pub by: String,
    pub reason: String,
    pub round: u32,
}

/// Thành phần họp do AI đề xuất / tự chọn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StaffingInfo {
    pub chair_id: RoleId,
    pub reviewer_ids: Vec<RoleId>,
    pub department_ids: Vec<String>,
    /// id role / phòng ban → lý do được chọn
    pub reasons: std::collections::BTreeMap<String, String>,
    /// true = model lỗi, dùng thành phần mặc định
    pub fallback: bool,
    pub note: String,
}

/// 1 lần role dùng công cụ (đọc file / tìm web) — hiển thị trong hội thoại.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolActivity {
    pub scope: String,
    pub agent: RoleId,
    /// "author" (trước khi viết plan version `step`) | "review" (trước khi review round `step`)
    pub phase: String,
    pub step: u32,
    pub tool: String,
    pub summary: String,
    pub ok: bool,
}

/// Tuỳ chọn đã dùng cho phiên (lưu vào lịch sử).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionInfo {
    pub workspace: Option<String>,
    pub web_search: bool,
    pub company_context: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: Uuid,
    pub prompt: String,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: SessionStatus,
    pub max_rounds: u32,
    /// Chủ trì cuộc họp chính (phiên cũ: "architect").
    #[serde(default = "default_chair")]
    pub chair_id: RoleId,
    #[serde(default)]
    pub reviewer_ids: Vec<RoleId>,
    #[serde(default)]
    pub cast: Vec<CastMember>,
    #[serde(default)]
    pub departments: Vec<DepartmentRecord>,
    #[serde(default)]
    pub usage: UsageReport,
    #[serde(default)]
    pub activities: Vec<ToolActivity>,
    #[serde(default)]
    pub options: SessionInfo,
    #[serde(default)]
    pub membership: Vec<MembershipChange>,
    #[serde(default)]
    pub staffing: Option<StaffingInfo>,
    /// Plan + round của cuộc họp chính.
    pub plans: Vec<PlanVersion>,
    pub rounds: Vec<RoundRecord>,
    pub final_markdown: Option<String>,
    pub error: Option<String>,
}

fn default_chair() -> RoleId {
    "architect".into()
}

impl SessionRecord {
    pub fn new(id: Uuid, prompt: String, max_rounds: u32) -> Self {
        Self {
            id,
            prompt,
            created_at: Utc::now(),
            finished_at: None,
            status: SessionStatus::Running,
            max_rounds,
            chair_id: default_chair(),
            reviewer_ids: vec![],
            cast: vec![],
            departments: vec![],
            usage: UsageReport::default(),
            activities: vec![],
            options: SessionInfo::default(),
            membership: vec![],
            staffing: None,
            plans: vec![],
            rounds: vec![],
            final_markdown: None,
            error: None,
        }
    }

    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id,
            prompt_preview: truncate_chars(&self.prompt, 140),
            created_at: self.created_at,
            status: self.status,
            versions: self.plans.len() as u32,
            rounds: self.rounds.len() as u32,
            departments: self.departments.len() as u32,
            total_tokens: self.usage.total.total(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: Uuid,
    pub prompt_preview: String,
    pub created_at: DateTime<Utc>,
    pub status: SessionStatus,
    pub versions: u32,
    pub rounds: u32,
    #[serde(default)]
    pub departments: u32,
    #[serde(default)]
    pub total_tokens: u64,
}

// ───────────────────────────── Events ─────────────────────────────

pub const COUNCIL_EVENT: &str = "council-event";

/// Payload của `app.emit("council-event", ...)`. Tagged union → TS discriminated union.
/// `scope` = "main" (cuộc họp chính) hoặc id phòng ban.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// Event ngắn hạn, emit xong là bỏ → chênh kích thước biến thể không đáng kể; box sẽ phải sửa mọi chỗ tạo event.
#[allow(clippy::large_enum_variant)]
pub enum CouncilEvent {
    SessionStarted {
        prompt: String,
        max_rounds: u32,
        chair_id: RoleId,
        reviewer_ids: Vec<RoleId>,
        cast: Vec<CastMember>,
        departments: Vec<DepartmentBrief>,
        options: SessionInfo,
    },
    /// Sau mỗi lần gọi LLM: token của lần gọi + tổng cộng dồn của phiên.
    UsageUpdated {
        scope: String,
        agent: RoleId,
        kind: String,
        prompt_tokens: u64,
        completion_tokens: u64,
        estimated: bool,
        total: TokenUsage,
    },
    /// Role vừa dùng công cụ (đọc file / tìm web).
    ToolUsed {
        activity: ToolActivity,
    },
    /// AI đã chọn thành phần họp (chế độ tự động).
    StaffingDecided {
        staffing: StaffingInfo,
    },
    /// Thành viên vào / ra cuộc họp giữa chừng. `member` có khi được mời (để UI hiển thị).
    MembershipChanged {
        change: MembershipChange,
        member: Option<CastMember>,
    },
    /// Token tiết kiệm được (review theo section / giữ nguyên APPROVE).
    TokensSaved {
        scope: String,
        agent: RoleId,
        tokens: u64,
        carried: bool,
        total_saved: u64,
    },
    /// Chủ trì đã giao việc cho các phòng ban.
    DepartmentsAssigned {
        assignments: Vec<Assignment>,
    },
    AgentThinking {
        scope: String,
        agent: RoleId,
        round: u32,
        plan_version: u32,
    },
    PlanStreamReset {
        scope: String,
        version: u32,
    },
    PlanDelta {
        scope: String,
        version: u32,
        delta: String,
    },
    PlanReady {
        scope: String,
        version: u32,
        markdown: String,
    },
    ReviewReady {
        scope: String,
        round: u32,
        review: Review,
    },
    ReviewFailed {
        scope: String,
        round: u32,
        agent: RoleId,
        error: String,
    },
    /// Output bị cắt. `call_kind`: "plan" | "review" | "consistency";
    /// `reason`: "length" | "max_tokens_cap" | "budget" | "stream_error";
    /// `recovered = true` khi đã nối tiếp thành công (plan dùng bản ghép).
    Truncated {
        scope: String,
        agent: RoleId,
        call_kind: String,
        recovered: bool,
        reason: String,
    },
    /// Section revise không dùng được → viết lại toàn bộ plan. `consecutive` = số lần fallback liên tiếp
    /// (≥ 2 → các vòng còn lại của nơi họp này luôn viết lại toàn bộ).
    ReviseFallback {
        scope: String,
        round: u32,
        reason: crate::context_builder::FallbackReason,
        consecutive: u32,
    },
    /// Kết quả kiểm tra nhất quán sau khi mọi reviewer APPROVE (chỉ khi có vòng dùng section revise).
    /// `result`: "ok" | "conflicts" | "unverified".
    Consistency {
        scope: String,
        result: String,
        conflicts: Vec<ConsistencyConflict>,
    },
    /// Ảnh chụp ngân sách token sau mỗi lần settle (chỉ khi có giới hạn). `soft` luôn true.
    /// `state`: "ok" | "warn" (≥ 80%) | "exhausted".
    Budget {
        used: u64,
        reserved: u64,
        limit: u64,
        estimated: bool,
        soft: bool,
        state: String,
    },
    RoundCompleted {
        scope: String,
        round: u32,
        plan_version: u32,
        approvals: u32,
        total: u32,
        converged: bool,
    },
    DepartmentFinished {
        department_id: String,
        status: SessionStatus,
        final_markdown: Option<String>,
        error: Option<String>,
    },
    Finished {
        status: SessionStatus,
        final_version: u32,
        final_markdown: String,
    },
    Cancelled {
        round: u32,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct EventEnvelope {
    pub session_id: Uuid,
    /// Tăng đơn điệu trong 1 session → Frontend có thể detect mất/đảo event.
    pub seq: u64,
    pub ts: DateTime<Utc>,
    #[serde(flatten)]
    pub event: CouncilEvent,
}

// ───────────────────────────── Helpers ─────────────────────────────

pub fn truncate_words(s: &str, max_words: usize) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() <= max_words {
        words.join(" ")
    } else {
        format!("{}…", words[..max_words].join(" "))
    }
}

/// Cắt theo char (không theo byte) để an toàn với tiếng Việt có dấu / emoji.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        None => s.to_owned(),
        Some((idx, _)) => format!("{}…", &s[..idx]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_statuses_and_events_serialize_snake_case() {
        for (s, j) in [
            (SessionStatus::Truncated, "\"truncated\""),
            (SessionStatus::BudgetExhausted, "\"budget_exhausted\""),
            (SessionStatus::Unresolved, "\"unresolved\""),
            (SessionStatus::Interrupted, "\"interrupted\""),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), j);
            assert_eq!(serde_json::from_str::<SessionStatus>(j).unwrap(), s);
        }
        assert_eq!(SessionStatus::Converged.worst(SessionStatus::Truncated), SessionStatus::Truncated);
        assert_eq!(SessionStatus::BudgetExhausted.worst(SessionStatus::MaxRoundsReached), SessionStatus::BudgetExhausted);
        let t = serde_json::to_value(CouncilEvent::Truncated {
            scope: "main".into(),
            agent: "cto".into(),
            call_kind: "plan".into(),
            recovered: true,
            reason: "length".into(),
        })
        .unwrap();
        assert_eq!(t, serde_json::json!({"type":"truncated","scope":"main","agent":"cto","call_kind":"plan","recovered":true,"reason":"length"}));
        let f = serde_json::to_value(CouncilEvent::ReviseFallback {
            scope: "main".into(),
            round: 2,
            reason: crate::context_builder::FallbackReason::MissingIds,
            consecutive: 1,
        })
        .unwrap();
        assert_eq!(f, serde_json::json!({"type":"revise_fallback","scope":"main","round":2,"reason":"missing_ids","consecutive":1}));
        let c = serde_json::to_value(CouncilEvent::Consistency {
            scope: "main".into(),
            result: "conflicts".into(),
            conflicts: vec![ConsistencyConflict { sections: vec!["s2".into(), "s5".into()], text: "lệch".into() }],
        })
        .unwrap();
        assert_eq!(
            c,
            serde_json::json!({"type":"consistency","scope":"main","result":"conflicts","conflicts":[{"sections":["s2","s5"],"text":"lệch"}]})
        );
        // Bản ghi cũ không có revise_mode / fallback_reason vẫn đọc được.
        let old: PlanVersion = serde_json::from_str(r#"{"version":1,"markdown":"x","created_at":"2024-01-01T00:00:00Z"}"#).unwrap();
        assert_eq!((old.revise_mode, old.fallback_reason), (ReviseKind::Full, None));
        let b = serde_json::to_value(CouncilEvent::Budget { used: 1, reserved: 2, limit: 3, estimated: false, soft: true, state: "ok".into() }).unwrap();
        assert_eq!(b, serde_json::json!({"type":"budget","used":1,"reserved":2,"limit":3,"estimated":false,"soft":true,"state":"ok"}));
    }

    #[test]
    fn verdict_is_lenient() {
        for (s, v) in [
            ("\"APPROVE\"", Verdict::Approve),
            ("\"approved\"", Verdict::Approve),
            ("\" Reject \"", Verdict::Reject),
            ("\"REJECTED\"", Verdict::Reject),
        ] {
            assert_eq!(serde_json::from_str::<Verdict>(s).unwrap(), v);
        }
        assert!(serde_json::from_str::<Verdict>("\"MAYBE\"").is_err());
    }

    #[test]
    fn raw_review_tolerates_schema_drift() {
        let raw: RawReview = serde_json::from_str(
            r#"{"verdict":"reject","critical_issues":["a",{"issue":"b"},null,""],"suggestions":"only one"}"#,
        )
        .unwrap();
        assert_eq!(raw.critical_issues, vec!["a", "b"]);
        assert_eq!(raw.suggestions, vec!["only one"]);
        let r = Review::from_raw("chaos_reviewer", "Security Engineer", raw);
        assert_eq!(r.role, "Security Engineer");
        assert_eq!(r.agent, "chaos_reviewer");
        assert_eq!(r.summary_bubble, "a"); // fallback từ issue đầu tiên
    }

    #[test]
    fn section_normalization() {
        for (s, want) in [
            ("3.3", Some("3.3")),
            ("§3.3", Some("3.3")),
            ("Section 3.3", Some("3.3")),
            ("3.3.", Some("3.3")),
            ("mục 3.3", Some("3.3")),
            ("## 3. Data Flow", Some("3")),
            ("3.3 Startup", Some("3.3")),
            ("Overview", None),
            ("", None),
        ] {
            assert_eq!(normalize_section(s).as_deref(), want, "input {s:?}");
        }
    }

    #[test]
    fn raw_review_sections_shapes() {
        let raw: RawReview = serde_json::from_str(
            r#"{"verdict":"REJECT","critical_issues":["x"],
                "sections":["§3.3", 2, "Section 3.3", "mục 4.1", "3.3.", "3.3, 4.2", null, "n/a", {"section":"5"}]}"#,
        )
        .unwrap();
        assert_eq!(raw.sections, vec!["3.3", "2", "4.1", "4.2", "5"]);

        // 1 chuỗi / 1 số thay vì mảng; alias "section".
        let raw: RawReview = serde_json::from_str(r#"{"verdict":"REJECT","sections":"mục 2"}"#).unwrap();
        assert_eq!(raw.sections, vec!["2"]);
        let raw: RawReview = serde_json::from_str(r#"{"verdict":"REJECT","section":3.3}"#).unwrap();
        assert_eq!(raw.sections, vec!["3.3"]);

        // Thiếu field / null → rỗng; from_raw giữ nguyên.
        let raw: RawReview = serde_json::from_str(r#"{"verdict":"APPROVE"}"#).unwrap();
        assert!(raw.sections.is_empty());
        let raw: RawReview = serde_json::from_str(r#"{"verdict":"APPROVE","sections":null}"#).unwrap();
        assert!(raw.sections.is_empty());
        let raw: RawReview =
            serde_json::from_str(r#"{"verdict":"REJECT","critical_issues":["x"],"sections":["3.3"]}"#).unwrap();
        assert_eq!(Review::from_raw("a", "A", raw).sections, vec!["3.3"]);
    }

    #[test]
    fn old_review_json_has_no_sections() {
        let old = r#"{"agent":"dba","role":"DBA","verdict":"REJECT","summary_bubble":"s",
            "critical_issues":["x"],"suggestions":[]}"#;
        let r: Review = serde_json::from_str(old).unwrap();
        assert!(r.sections.is_empty());
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["sections"], serde_json::json!([]));
    }

    #[test]
    fn round_one_policy() {
        let raw: RawReview =
            serde_json::from_str(r#"{"verdict":"APPROVE","critical_issues":["x"]}"#).unwrap();
        let mut r = Review::from_raw("scale_reviewer", "Perf", raw);
        assert!(r.violates_round_one_policy());
        r.enforce_round_one_policy();
        assert_eq!(r.verdict, Verdict::Reject);
        assert!(r.policy_override.is_some());
    }

    #[test]
    fn truncation_is_unicode_safe() {
        assert_eq!(truncate_chars("Kiến trúc", 4), "Kiến…");
        let long = "một hai ba bốn năm sáu bảy tám chín mười mười-một mười-hai mười-ba mười-bốn mười-lăm mười-sáu";
        let t = truncate_words(long, SUMMARY_BUBBLE_MAX_WORDS);
        assert!(t.ends_with('…'));
        assert_eq!(t.trim_end_matches('…').split_whitespace().count(), 15);
    }

    #[test]
    fn event_envelope_shape() {
        let env = EventEnvelope {
            session_id: Uuid::nil(),
            seq: 7,
            ts: Utc::now(),
            event: CouncilEvent::PlanDelta {
                scope: MAIN_SCOPE.into(),
                version: 2,
                delta: "x".into(),
            },
        };
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["type"], "plan_delta");
        assert_eq!(v["seq"], 7);
        assert_eq!(v["version"], 2);
        assert_eq!(v["scope"], "main");
    }

    #[test]
    fn old_session_files_still_load() {
        // Phiên lưu trước khi có role động / phòng ban.
        let old = r#"{"id":"00000000-0000-0000-0000-000000000000","prompt":"p","created_at":"2026-01-01T00:00:00Z",
            "finished_at":null,"status":"converged","max_rounds":3,"plans":[],"rounds":[{"round":1,"plan_version":1,
            "slots":[{"agent":"scale_reviewer","review":null,"error":"x"}],"approvals":0,"converged":false}],
            "final_markdown":null,"error":null}"#;
        let r: SessionRecord = serde_json::from_str(old).unwrap();
        assert_eq!(r.chair_id, "architect");
        assert!(r.departments.is_empty() && r.cast.is_empty());
        assert_eq!(r.rounds[0].slots[0].agent, "scale_reviewer");
    }
}
