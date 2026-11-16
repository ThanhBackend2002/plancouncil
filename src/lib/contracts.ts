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
  /** Output người viết bị cắt, không nối tiếp được → giữ phiên bản hợp lệ gần nhất. */
  | "truncated"
  /** Hết ngân sách token (giới hạn mềm) → giữ phiên bản gần nhất. */
  | "budget_exhausted"
  /** Mọi reviewer APPROVE nhưng kiểm tra nhất quán còn mâu thuẫn / không xác minh được khi hết vòng. */
  | "unresolved"
  /** Dành cho wave sau. */
  | "interrupted"
  | "cancelled"
  | "failed";

export type TruncatedCallKind = "plan" | "review" | "consistency";
export type TruncatedReason = "length" | "max_tokens_cap" | "budget" | "stream_error";
export type BudgetState = "ok" | "warn" | "exhausted";

/** Ảnh chụp ngân sách token mới nhất (event `budget`). */
export interface BudgetSnapshot {
  used: number;
  reserved: number;
  limit: number;
  estimated: boolean;
  soft: boolean;
  state: BudgetState;
}

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

/** Cách phiên bản plan được tạo: bản nháp đầu, viết lại toàn bộ, hay chỉ sửa section liên quan. */
export type ReviseKind = "initial" | "full" | "section";
/** Lý do section revise không dùng được → viết lại toàn bộ. */
export type FallbackReason =
  | "missing_ids"
  | "extra_ids"
  | "duplicate_ids"
  | "base_mismatch"
  | "bad_structure"
  | "structural_change"
  | "ratio"
  | "truncated";

export interface PlanVersion {
  version: number;
  markdown: string;
  created_at: string;
  /** Bản ghi cũ không có field → "full". */
  revise_mode?: ReviseKind;
  fallback_reason?: FallbackReason | null;
}

/** 1 mâu thuẫn giữa các section (id "s2", "s5"…). */
export interface ConsistencyConflict { sections: string[]; text: string }
export type ConsistencyResult = "ok" | "conflicts" | "unverified";
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
  /** Phần prompt đọc từ cache (bản ghi cũ / FE cũ: thiếu). */
  cached_tokens?: number;
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
  /** Theo tên model. Bản ghi rất cũ (không có `calls`): rỗng / thiếu. */
  by_model?: Record<string, TokenUsage>;
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
  /** Nơi khởi tạo: thiếu = app desktop, "mcp" = Claude Code qua MCP. */
  origin?: string | null;
}

/** Thông tin người yêu cầu bổ sung giữa cuộc họp (send_council_message qua MCP). */
export interface UserNote {
  ts: string;
  text: string;
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
      /** Model đã gọi (backend cũ: thiếu). */
      model?: string;
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
      type: "truncated";
      scope: string;
      agent: RoleId;
      call_kind: TruncatedCallKind;
      recovered: boolean;
      reason: TruncatedReason;
    }
  | { type: "revise_fallback"; scope: string; round: number; reason: FallbackReason; consecutive: number }
  | { type: "consistency"; scope: string; result: ConsistencyResult; conflicts: ConsistencyConflict[] }
  | ({ type: "budget" } & BudgetSnapshot)
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
/** full = luôn viết lại toàn bộ; auto = section revise + mọi luật fallback; section = chỉ fallback khi patch hỏng. */
export type ReviseMode = "full" | "auto" | "section";

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
  /** Ngân sách token mềm cho cả phiên; null = không giới hạn (nếu đặt thì ≥ 1000). */
  max_total_tokens?: number | null;
  /** Cách chỉnh plan từ vòng 2 (mặc định "full"). */
  revise_mode: ReviseMode;
  /** 0.1–1.0; chỉ áp cho "auto". */
  section_mode_max_ratio: number;
  /** "author" hoặc id role reviewer. */
  consistency_checker: string;
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
  /** Giá do người dùng tự nhập, theo tên model (USD / 1M token). Không có giá → chi phí hiển thị "—". */
  pricing?: Record<string, ModelPrice>;
}

export interface ModelPrice {
  input_per_m: number;
  cached_per_m: number;
  output_per_m: number;
}

export interface CommandError {
  kind:
    | "invalid"
    | "already_running"
    | "not_found"
    | "storage"
    | "llm"
    | "internal"
    | "index_unavailable"
    | "index"
    | "no_valid_version"
    | "missing_roles"
    | "unsupported_scope"
    | "secret_missing";
  message: string;
  /** Chỉ có với `secret_missing`: key nào đang thiếu. */
  target?: SecretTarget;
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

// ── API key: nơi lưu (keyring / file / RAM) — mirror src-tauri/src/secrets.rs ──
export type KeyStorage = "keyring" | "file" | "memory";

// Config trả về có thêm `key_storage` (backend tự quyết; chỉ đổi được qua set_secret).
export interface Provider { key_storage?: KeyStorage }
export interface WebSearchConfig { key_storage?: KeyStorage }

export type SecretTargetKind = "provider" | "websearch";
/** `websearch` dùng id = dịch vụ tìm web đang chọn (vd "tavily"). */
export interface SecretTarget { kind: SecretTargetKind; id: string }

export type SecretErrorCode = "keyring_unavailable" | "keyring_locked" | "storage" | "invalid";

export interface SecretTargetStatus {
  target: SecretTarget;
  /** Nơi key đang thực sự nằm; key plaintext chưa migrate khỏi config.json = "file". */
  storage: KeyStorage;
  /** false = key vẫn nằm plaintext trong config.json. */
  migrated: boolean;
  has_key: boolean;
  /** Key đáng lẽ phải có nhưng không có (RAM sau khi mở lại app, keyring khoá…). */
  missing: boolean;
  error?: string;
  error_code?: SecretErrorCode;
}

export interface SecretStatusReport {
  targets: SecretTargetStatus[];
  /** File backup/hỏng còn chứa api_key plaintext. */
  leftover_files: string[];
  /** Đường dẫn secrets.json (dùng trong hộp thoại đồng ý lưu dạng file). */
  secrets_file: string;
}

export interface SetSecretResult { storage: KeyStorage; masked: string }

/** Lỗi command liên quan tới key (giữ nguyên `kind` từ backend). */
export interface SecretCommandError {
  kind: string;
  message: string;
  target?: SecretTarget;
}

// set_secret({ target, api_key, storage }) → SetSecretResult (lỗi keyring_unavailable / keyring_locked)
// delete_secret({ target }) / secret_status() → SecretStatusReport / delete_leftover_files({ files }) → string[]
// start_council có thể trả lỗi { kind: "secret_missing", target }

// ── Crash safety / chạy lại / chi phí (bổ sung — interface trùng tên được TS gộp khai báo) ──

/** Nguồn gốc phiên chạy lại. `version_no = 0` = chạy lại từ yêu cầu gốc (`restart_from_request`). */
export interface ParentRef {
  session_id: string;
  /** "main" (phòng ban hiện báo `unsupported_scope`). */
  scope: string;
  version_no: number;
}

/** Token của 1 lần gọi LLM (`SessionRecord.calls`, tối đa 2000; bản ghi cũ: []). */
export interface CallUsage {
  call_id: number;
  scope: string;
  agent: RoleId;
  /** 0 = trước vòng review đầu (chọn thành phần / giao việc / bản nháp). */
  round: number;
  kind: UsageKind | "consistency";
  model: string;
  prompt: number;
  cached: number;
  completion: number;
  estimated: boolean;
  finish: "stop" | "length" | "error" | "tool_calls";
  latency_ms: number;
}

export interface SessionRecord {
  /** null = phiên mới / bản ghi cũ. */
  parent?: ParentRef | null;
  calls?: CallUsage[];
  user_notes?: UserNote[];
}

export interface SessionSummary {
  parent?: ParentRef | null;
}

export interface TokenUsage {
  /** Phần prompt_tokens đọc từ cache của provider (bản ghi cũ: 0 / thiếu). */
  cached_tokens?: number;
}

/** USD / 1 triệu token. */
export interface ModelPrice {
  input_per_m: number;
  cached_per_m: number;
  output_per_m: number;
}
/** Tên model (đúng chuỗi `model` của role) → giá. Mặc định rỗng; số hữu hạn ≥ 0. */
export type Pricing = Record<string, ModelPrice>;

export interface AppConfig {
  pricing?: Pricing;
}

/**
 * Field thêm vào event `usage_updated` (luôn có từ backend mới). Chi phí cộng dồn của phiên theo bảng
 * giá lúc BẮT ĐẦU phiên; `cost_usd = null` khi có model chưa có giá (`unpriced_models`).
 */
export interface UsageUpdatedCost {
  cached_tokens: number;
  cost_usd: number | null;
  unpriced_models: string[];
}
export type UsageUpdatedEvent = Extract<CouncilEvent, { type: "usage_updated" }> & UsageUpdatedCost;

export interface RerunOverrides {
  /** Tập con reviewer của phiên gốc. */
  reviewers?: RoleId[];
  /** 1..=10. */
  max_rounds?: number;
  /** Thiếu = giữ cấu hình hiện tại; null = không giới hạn; số ≥ 1000. */
  max_total_tokens?: number | null;
}

export interface RerunResult {
  /** null khi dry_run. */
  new_session_id: string | null;
  /** Section chỉ gắn tag cho reviewer không còn trong nhóm mới. */
  uncovered_sections: string[];
  /** Role của phiên gốc không còn / không có provider-model dùng được. */
  missing_roles: RoleId[];
  available_versions: number[];
}

export interface RestartResult { new_session_id: string }

/** Lỗi của rerun_from_version / restart_from_request / get_usage (giữ nguyên `kind` + payload). */
export interface RerunCommandError {
  kind:
    | "no_valid_version"
    | "missing_roles"
    | "unsupported_scope"
    | "already_running"
    | "not_found"
    | "invalid"
    | "secret_missing"
    | (string & {});
  message: string;
  /** Chỉ với `no_valid_version`. */
  available_versions?: number[];
  /** Chỉ với `missing_roles`. */
  missing_roles?: RoleId[];
  target?: SecretTarget;
}

export interface UsageRow {
  prompt: number;
  cached: number;
  completion: number;
  calls: number;
  estimated: boolean;
  cost_usd: number | null;
}
export interface RoleUsageRow extends UsageRow { agent: RoleId }
export interface RoundUsageRow extends UsageRow { scope: string; round: number }

/** get_usage({ session_id }) — chi phí tính bằng bảng giá HIỆN TẠI. Bản ghi cũ: by_round = [], cost null. */
export interface UsageBreakdown {
  by_role: RoleUsageRow[];
  by_round: RoundUsageRow[];
  totals: { prompt: number; cached: number; completion: number; estimated: boolean; cost_usd: number | null };
  unpriced_models: string[];
}

// rerun_from_version({ session_id, scope, version_no, dry_run?, overrides? }) → RerunResult
// restart_from_request({ session_id }) → RestartResult
// get_usage({ session_id }) → UsageBreakdown

/** Tham số rerun_from_version phía FE (snake_case — command dùng rename_all = "snake_case"). */
export interface RerunArgs {
  session_id: string;
  scope: string;
  version_no: number;
  dry_run?: boolean;
  overrides?: RerunOverrides;
}
export type UsageSummary = UsageBreakdown;
export type UsageTotals = UsageBreakdown["totals"];
