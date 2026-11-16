// IPC contract — mirror 1:1 của src-tauri/src/models.rs + config.rs.
// Sửa bên Rust thì phải sửa ở đây.

/** Id của role do người dùng quản lý (vd "cto", "qa_engineer", hoặc uuid). */
export type RoleId = string;
export type Verdict = "APPROVE" | "REJECT" | "ABSTAIN";
export type SessionStatus =
  | "running"
  | "converged"
  | "max_rounds_reached"
  | "partially_approved"
  | "cancelled"
  | "failed";

/** Scope của cuộc họp chính; phòng ban dùng id phòng ban. */
export const MAIN_SCOPE = "main";

export interface Review {
  agent: RoleId;
  role: string;
  verdict: Verdict;
  summary_bubble: string;
  critical_issues: string[];
  suggestions: string[];
  /** Ý tưởng vượt phạm vi yêu cầu — không phải lý do REJECT. */
  out_of_scope?: string[];
  invites?: InviteRequest[];
  /** Số hiệu heading (vd "3.3", "2") mà critical_issues nhắc tới. */
  sections?: string[];
  /** true = không gọi model, giữ nguyên APPROVE vì phần phụ trách không đổi. */
  carried_over?: boolean;
  policy_override?: string;
}

export interface InviteRequest { role_id: RoleId; reason: string }

export interface MembershipChange {
  scope: string;
  agent: RoleId;
  action: "invited" | "dismissed" | "abstained";
  /** "user" | id role đề nghị | "self" */
  by: string;
  reason: string;
  round: number;
}

export interface StaffingInfo {
  chair_id: RoleId;
  reviewer_ids: RoleId[];
  department_ids: string[];
  reasons: Record<string, string>;
  fallback: boolean;
  note: string;
}

export interface PlanVersion { version: number; markdown: string; created_at: string }
export interface ReviewSlot { agent: RoleId; review: Review | null; error: string | null }
export interface RoundRecord { round: number; plan_version: number; slots: ReviewSlot[]; approvals: number; converged: boolean }

/** Ngoại hình nhân vật pixel. Chuỗi rỗng = tự sinh từ id/cấp bậc. */
export interface Appearance {
  gender: string;
  skin: string;
  hair_style: string;
  hair_color: string;
  outfit: string;
  outfit_color: string;
  eyewear: string;
  accessory: string;
}

export interface CastMember {
  id: RoleId;
  name: string;
  title: string;
  color: string;
  icon: string;
  model: string;
  level: RoleLevel;
  appearance?: Appearance;
}
export interface DepartmentBrief { id: string; name: string; lead_id: RoleId; member_ids: RoleId[]; max_rounds: number }
export interface Assignment { department_id: string; task: string }

export interface DepartmentRecord {
  id: string;
  name: string;
  task: string;
  lead_id: RoleId;
  member_ids: RoleId[];
  max_rounds: number;
  status: SessionStatus;
  plans: PlanVersion[];
  rounds: RoundRecord[];
  final_markdown: string | null;
  error: string | null;
}

export interface TokenUsage {
  prompt_tokens: number;
  completion_tokens: number;
  calls: number;
  /** Số lần provider không trả `usage` → token được ước lượng theo độ dài văn bản. */
  estimated_calls: number;
}

export type UsageKind = "assign" | "research" | "plan" | "review" | "staffing";

export interface UsageReport {
  total: TokenUsage;
  by_role: Record<RoleId, TokenUsage>;
  by_scope: Record<string, TokenUsage>;
  by_kind: Record<string, TokenUsage>;
  /** Token tiết kiệm nhờ review theo section + giữ nguyên APPROVE. */
  saved_tokens?: number;
  carried_reviews?: number;
}

export interface ToolActivity {
  scope: string;
  agent: RoleId;
  /** "author" (trước khi viết plan version `step`) | "review" (trước khi review round `step`) */
  phase: "author" | "review";
  step: number;
  tool: string;
  summary: string;
  ok: boolean;
}

export interface SessionInfo {
  workspace: string | null;
  web_search: boolean;
  company_context: boolean;
}

export interface SessionRecord {
  id: string;
  prompt: string;
  created_at: string;
  finished_at: string | null;
  status: SessionStatus;
  max_rounds: number;
  chair_id: RoleId;
  reviewer_ids: RoleId[];
  cast: CastMember[];
  departments: DepartmentRecord[];
  usage?: UsageReport;
  activities?: ToolActivity[];
  options?: SessionInfo;
  membership?: MembershipChange[];
  staffing?: StaffingInfo | null;
  plans: PlanVersion[];
  rounds: RoundRecord[];
  final_markdown: string | null;
  error: string | null;
}

export interface SessionSummary {
  id: string;
  prompt_preview: string;
  created_at: string;
  status: SessionStatus;
  versions: number;
  rounds: number;
  departments: number;
  total_tokens: number;
}

/**
 * 1 kết quả `search_history` (struct `SearchHit` của search_index.rs). Đặt tên khác để không trùng
 * `SearchHit` của tìm kiếm web.
 */
export interface HistorySearchHit {
  session_id: string;
  session_title: string;
  /** "main" hoặc id phòng ban. */
  scope: string;
  /** Version plan của section; 0 với kind = "request". */
  version_no: number;
  kind: "request" | "section";
  section_key: string | null;
  heading: string | null;
  /** Văn bản gốc, chỗ khớp bọc `[[` … `]]`. */
  snippet: string;
  /** bm25: càng nhỏ càng liên quan. */
  rank: number;
  created_at: string;
}

export type CouncilEvent =
  | {
      type: "session_started";
      prompt: string;
      max_rounds: number;
      chair_id: RoleId;
      reviewer_ids: RoleId[];
      cast: CastMember[];
      departments: DepartmentBrief[];
      options: SessionInfo;
    }
  | {
      type: "usage_updated";
      scope: string;
      agent: RoleId;
      kind: UsageKind;
      prompt_tokens: number;
      completion_tokens: number;
      estimated: boolean;
      total: TokenUsage;
    }
  | { type: "tool_used"; activity: ToolActivity }
  | { type: "staffing_decided"; staffing: StaffingInfo }
  | { type: "membership_changed"; change: MembershipChange; member: CastMember | null }
  | { type: "tokens_saved"; scope: string; agent: RoleId; tokens: number; carried: boolean; total_saved: number }
  | { type: "departments_assigned"; assignments: Assignment[] }
  | { type: "agent_thinking"; scope: string; agent: RoleId; round: number; plan_version: number }
  | { type: "plan_stream_reset"; scope: string; version: number }
  | { type: "plan_delta"; scope: string; version: number; delta: string }
  | { type: "plan_ready"; scope: string; version: number; markdown: string }
  | { type: "review_ready"; scope: string; round: number; review: Review }
  | { type: "review_failed"; scope: string; round: number; agent: RoleId; error: string }
  | {
      type: "round_completed";
      scope: string;
      round: number;
      plan_version: number;
      approvals: number;
      total: number;
      converged: boolean;
    }
  | {
      type: "department_finished";
      department_id: string;
      status: SessionStatus;
      final_markdown: string | null;
      error: string | null;
    }
  | { type: "finished"; status: SessionStatus; final_version: number; final_markdown: string }
  | { type: "cancelled"; round: number }
  | { type: "failed"; error: string };

export type EventEnvelope = CouncilEvent & { session_id: string; seq: number; ts: string };

export const COUNCIL_EVENT = "council-event";
export const SESSION_SAVED_EVENT = "council-session-saved";

// ── Config (schema v3) ──
/** Giá trị key trả về từ get_config đã bị mask; gửi lại nguyên giá trị mask = giữ key cũ, "" = xoá. */
export const MASK_PREFIX = "••••";
export type JsonMode = "off" | "json_object";
export type RoleLevel = "c_level" | "head" | "manager" | "lead" | "engineer" | "specialist";

/** Nơi duy nhất quản lý nguồn API: base URL + key + cache danh sách model. */
export interface Provider {
  id: string;
  name: string;
  base_url: string;
  api_key: string | null;
  models: string[];
  models_fetched_at: string | null;
}

export interface RoleDef {
  id: RoleId;
  name: string;
  title: string;
  level: RoleLevel;
  icon: string;
  color: string;
  description: string;
  /** Persona: role này quan tâm / soi điều gì (chèn vào system prompt). */
  instructions: string;
  provider_id: string;
  model: string;
  temperature: number;
  appearance: Appearance;
}

export interface Department {
  id: string;
  name: string;
  mission: string;
  lead_id: RoleId;
  member_ids: RoleId[];
  max_rounds: number;
}

export type PlanDepth = "mvp" | "standard" | "enterprise";
export type StaffingMode = "manual" | "suggest" | "auto";

export interface MeetingConfig {
  chair_id: RoleId;
  reviewer_ids: RoleId[];
  department_ids: string[];
  strict_first_round: boolean;
  plan_depth: PlanDepth;
  staffing: StaffingMode;
  required_role_ids: RoleId[];
  max_auto_reviewers: number;
  max_auto_departments: number;
  dynamic_membership: boolean;
  max_invites: number;
  section_routing: boolean;
}

/** Thư mục dự án được cấp quyền ĐỌC. */
export interface Workspace {
  id: string;
  name: string;
  path: string;
}

export type WebSearchProvider = "duckduckgo" | "tavily" | "brave" | "searxng";

export interface WebSearchConfig {
  provider: WebSearchProvider;
  api_key: string | null;
  base_url: string;
  max_results: number;
  enabled_by_default: boolean;
}

export interface CompanyContext {
  text: string;
  enabled_by_default: boolean;
}

export type ToolPolicy = "authors" | "all" | "off";

/** Tuỳ chọn riêng cho 1 phiên họp. */
export interface SessionOptions {
  workspace_id: string | null;
  web_search: boolean;
  company_context: boolean;
  /** Ghi đè chủ trì / reviewer cho phiên này (từ đề xuất đã duyệt). */
  chair_id?: RoleId | null;
  reviewer_ids?: RoleId[] | null;
  auto_staff?: boolean;
  plan_depth?: PlanDepth | null;
}

export interface StaffingProposal { staffing: StaffingInfo; tokens: number }

export interface SearchHit {
  title: string;
  url: string;
  snippet: string;
}

export interface AppConfig {
  schema_version: number;
  providers: Provider[];
  roles: RoleDef[];
  departments: Department[];
  meeting: MeetingConfig;
  max_tokens: number | null;
  max_rounds: number;
  request_timeout_secs: number;
  stream_idle_timeout_secs: number;
  max_retries: number;
  json_mode: JsonMode;
  stream_plans: boolean;
  max_parallel_calls: number;
  company_context: CompanyContext;
  workspaces: Workspace[];
  web_search: WebSearchConfig;
  tool_policy: ToolPolicy;
  max_tool_steps: number;
}

export interface CommandError {
  kind: "invalid" | "already_running" | "not_found" | "storage" | "llm" | "internal" | "index_unavailable" | "index";
  message: string;
}
export interface ConnectionTestResult { agent: RoleId; model: string; latency_ms: number; reply: string }
export interface ModelList { provider_id: string; models: string[]; fetched_at: string; cached: boolean }

export type HealthStatus =
  | "ok"
  | "quota"
  | "auth"
  | "not_found"
  | "rate_limit"
  | "bad_request"
  | "server"
  | "timeout"
  | "network"
  | "unassigned"
  | "other";

/** Kết quả kiểm tra 1 cặp provider + model (dùng chung cho mọi role gán cặp đó). */
export interface ModelHealth {
  provider_id: string;
  provider_name: string;
  model: string;
  role_ids: RoleId[];
  status: HealthStatus;
  message: string;
  latency_ms: number | null;
  /** Số dư / hạn mức nếu provider có API (OpenRouter). */
  balance: string | null;
  checked_at: string;
}

// Command names + args (invoke dùng camelCase cho tên tham số theo convention Tauri v2)
// get_config() / save_config({ config }) / start_council({ prompt, departments?, options? }) → session id
// pick_folder() → path | null / test_web_search({ query }) → SearchHit[] / default_roles() → RoleDef[]
// check_models({ roleIds? }) → ModelHealth[]
// stop_council({ sessionId }) / active_session() / list_sessions({ limit? })
// load_session({ sessionId }) / delete_session({ sessionId })
// search_history({ query, limit?, offset? }) → HistorySearchHit[] (lỗi "index_unavailable" nếu không có index)
// test_connection({ agent }) / export_markdown({ content, suggestedName }) → path | null
// list_models({ providerId, baseUrl?, apiKey? }) → ModelList
