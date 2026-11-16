// Reducer thuần cho EventEnvelope → SessionState. Không phụ thuộc React/zustand để dễ test.
// Mỗi phiên có nhiều "scope": cuộc họp chính ("main") + từng phòng ban (id phòng ban).

import {
  MAIN_SCOPE,
  type BudgetSnapshot,
  type CastMember,
  type DepartmentBrief,
  type EventEnvelope,
  type Review,
  type RoleId,
  type RoundRecord,
  type MembershipChange,
  type SessionInfo,
  type SessionRecord,
  type StaffingInfo,
  type SessionStatus,
  type ToolActivity,
  type TokenUsage,
  type TruncatedReason,
  type FallbackReason,
  type ConsistencyConflict,
  type ConsistencyResult,
  type UsageReport,
} from "../lib/contracts";

const TRUNCATE_REASON: Record<TruncatedReason, string> = {
  length: "chạm giới hạn token",
  max_tokens_cap: "chạm trần max_tokens",
  budget: "hết ngân sách để nối tiếp",
  stream_error: "stream bị đứt",
};

const FALLBACK_REASON: Record<FallbackReason, string> = {
  missing_ids: "thiếu section bắt buộc",
  extra_ids: "trả thừa / sai id section",
  duplicate_ids: "trùng id section",
  base_mismatch: "sai phiên bản gốc",
  bad_structure: "sai cấu trúc",
  structural_change: "cần đổi cấu trúc plan",
  ratio: "phần cần sửa quá lớn / không chỉ rõ section",
  truncated: "output bị cắt",
};

const TRUNCATE_KIND: Record<string, string> = { plan: "Plan", review: "Review", consistency: "Kiểm tra nhất quán" };

export type AgentStatus = "idle" | "thinking" | "done" | "error";
export type Phase = "idle" | SessionStatus;

export interface AgentView {
  status: AgentStatus;
  round: number;
  planVersion: number;
  review: Review | null;
  error: string | null;
}

export interface PlanView {
  version: number;
  markdown: string;
  complete: boolean;
}

export interface SlotView {
  review: Review | null;
  error: string | null;
  pending: boolean;
}

export interface RoundView {
  round: number;
  planVersion: number;
  slots: Record<RoleId, SlotView>;
  approvals: number;
  total: number;
  converged: boolean;
  completed: boolean;
}

export interface FinalView {
  version: number;
  markdown: string;
  status: SessionStatus;
}

/** Câu thoại gần nhất trong scope — phòng họp pixel hiện bong bóng của người đang nói. */
export interface Speech {
  agentId: RoleId;
  text: string;
  tone: "info" | "approve" | "reject" | "error";
  seq: number;
}

export interface ScopeState {
  id: string;
  name: string;
  kind: "main" | "department";
  /** Người viết plan: chủ trì (main) hoặc trưởng phòng. */
  authorId: RoleId;
  reviewerIds: RoleId[];
  maxRounds: number;
  task: string | null;
  status: Phase;
  currentRound: number;
  plans: PlanView[];
  rounds: RoundView[];
  agents: Record<RoleId, AgentView>;
  final: FinalView | null;
  error: string | null;
  speech: Speech | null;
  /** Lần role dùng công cụ (đọc file / tìm web) trong scope này. */
  activities: ToolActivity[];
  /** Thành viên vào / ra giữa chừng. */
  membership: MembershipChange[];
  /** Dòng thông báo hệ thống trong hội thoại (vd. output bị cắt), gắn với phiên bản plan lúc xảy ra. */
  notices?: ScopeNotice[];
}

export interface ScopeNotice {
  key: string;
  agentId: RoleId;
  planVersion: number;
  text: string;
}

export interface SessionState {
  id: string | null;
  prompt: string;
  status: Phase;
  cast: CastMember[];
  usage: UsageReport;
  options: SessionInfo | null;
  /** Thành phần do AI chọn (chế độ Tự động). */
  staffing: StaffingInfo | null;
  /** Ảnh chụp ngân sách token mới nhất (chỉ có khi phiên đặt giới hạn). */
  budget?: BudgetSnapshot;
  /** Lần cắt output gần nhất KHÔNG nối tiếp được (cho banner trạng thái). */
  lastTruncated?: { scope: string; reason: TruncatedReason };
  /** Kết quả kiểm tra nhất quán gần nhất (cho banner `unresolved`). */
  consistency?: { scope: string; result: ConsistencyResult; conflicts: ConsistencyConflict[] };
  /** Số lần revise-theo-section bị loại LIÊN TIẾP gần nhất (cho banner gợi ý revise_mode=full). */
  fallbackStreak?: number;
  /** Thứ tự hiển thị: "main" trước, rồi các phòng ban. */
  scopeOrder: string[];
  scopes: Record<string, ScopeState>;
  error: string | null;
  /** -1 = chưa nhận event nào (vừa start hoặc vừa khôi phục sau reload). */
  lastSeq: number;
  /** true nếu phát hiện seq bị nhảy → nạp lại từ đĩa khi phiên kết thúc. */
  seqGap: boolean;
  /** true nếu state được khôi phục qua `active_session()` sau khi webview reload. */
  resumed: boolean;
  readOnly: boolean;
  createdAt: string | null;
  /**
   * Text plan đang stream, tách khỏi `scopes` để mỗi `plan_delta` chỉ thay map nhỏ này
   * (không copy plans/scope/scopes) → component nào không hiển thị text stream sẽ không re-render.
   * Key = `draftKey(scope, version)`. Bất biến: chỉ có key cho plan đã tồn tại và CHƯA complete;
   * khi có key thì đây là markdown hiện hành của plan đó (thay cho `PlanView.markdown`).
   * Đọc qua `planText()`, đừng đọc `PlanView.markdown` trực tiếp cho plan chưa complete.
   */
  drafts: Record<string, string>;
}

const idleAgent = (): AgentView => ({ status: "idle", round: 0, planVersion: 0, review: null, error: null });

function makeScope(p: {
  id: string;
  name: string;
  kind: ScopeState["kind"];
  authorId: RoleId;
  reviewerIds: RoleId[];
  maxRounds: number;
  status?: Phase;
}): ScopeState {
  const agents: Record<RoleId, AgentView> = {};
  for (const id of [p.authorId, ...p.reviewerIds]) if (id) agents[id] = idleAgent();
  return {
    id: p.id,
    name: p.name,
    kind: p.kind,
    authorId: p.authorId,
    reviewerIds: p.reviewerIds,
    maxRounds: p.maxRounds,
    task: null,
    status: p.status ?? "running",
    currentRound: 0,
    plans: [],
    rounds: [],
    agents,
    final: null,
    error: null,
    speech: null,
    activities: [],
    membership: [],
  };
}

export const emptyUsage = (): TokenUsage => ({ prompt_tokens: 0, completion_tokens: 0, calls: 0, estimated_calls: 0 });
export const emptyReport = (): UsageReport => ({ total: emptyUsage(), by_role: {}, by_scope: {}, by_kind: {} });

function addUsage(map: Record<string, TokenUsage>, key: string, p: number, c: number, est: boolean): Record<string, TokenUsage> {
  const cur = map[key] ?? emptyUsage();
  return {
    ...map,
    [key]: {
      prompt_tokens: cur.prompt_tokens + p,
      completion_tokens: cur.completion_tokens + c,
      calls: cur.calls + 1,
      estimated_calls: cur.estimated_calls + (est ? 1 : 0),
    },
  };
}

const mainScope = (authorId: RoleId, reviewerIds: RoleId[], maxRounds: number, status?: Phase) =>
  makeScope({ id: MAIN_SCOPE, name: "Họp chính", kind: "main", authorId, reviewerIds, maxRounds, status });

const deptScope = (d: DepartmentBrief, status?: Phase) =>
  makeScope({
    id: d.id,
    name: d.name,
    kind: "department",
    authorId: d.lead_id,
    reviewerIds: d.member_ids,
    maxRounds: d.max_rounds,
    status,
  });

export function initialSession(): SessionState {
  return {
    id: null,
    prompt: "",
    status: "idle",
    cast: [],
    usage: emptyReport(),
    options: null,
    staffing: null,
    scopeOrder: [MAIN_SCOPE],
    scopes: { [MAIN_SCOPE]: mainScope("", [], 0, "idle") },
    error: null,
    lastSeq: -1,
    seqGap: false,
    resumed: false,
    readOnly: false,
    createdAt: null,
    drafts: {},
  };
}

export const isTerminal = (p: Phase): boolean => p !== "idle" && p !== "running";

export function mainOf(s: SessionState): ScopeState {
  return s.scopes[MAIN_SCOPE] ?? mainScope("", [], 0, s.status);
}

// ───────────────────────── text plan đang stream ─────────────────────────

export const draftKey = (scope: string, version: number): string => `${scope}#${version}`;

/** Markdown hiện hành của 1 plan: bản nháp đang stream (nếu có) hoặc text đã lưu trong PlanView. */
export function planText(s: SessionState, scope: string, p: PlanView): string {
  return p.complete ? p.markdown : (s.drafts[draftKey(scope, p.version)] ?? p.markdown);
}

/** Bỏ bản nháp của 1 plan (plan bị reset / đã xong). Không có key → trả nguyên state, không copy. */
function dropDraft(s: SessionState, scope: string, version: number): SessionState {
  const key = draftKey(scope, version);
  if (!(key in s.drafts)) return s;
  const { [key]: _gone, ...rest } = s.drafts;
  return { ...s, drafts: rest };
}

// ───────────────────────── helpers bất biến ─────────────────────────

function upsertPlan(plans: PlanView[], version: number, f: (p: PlanView) => PlanView): PlanView[] {
  const i = plans.findIndex((p) => p.version === version);
  if (i === -1) {
    return [...plans, f({ version, markdown: "", complete: false })].sort((a, b) => a.version - b.version);
  }
  const next = plans.slice();
  next[i] = f(plans[i]!);
  return next;
}

function upsertRound(sc: ScopeState, round: number, planVersion: number, f: (r: RoundView) => RoundView): RoundView[] {
  const i = sc.rounds.findIndex((r) => r.round === round);
  if (i === -1) {
    const slots: Record<RoleId, SlotView> = {};
    for (const id of sc.reviewerIds) slots[id] = { review: null, error: null, pending: false };
    const fresh: RoundView = {
      round,
      planVersion,
      slots,
      approvals: 0,
      total: sc.reviewerIds.length,
      converged: false,
      completed: false,
    };
    return [...sc.rounds, f(fresh)].sort((a, b) => a.round - b.round);
  }
  const next = sc.rounds.slice();
  next[i] = f(sc.rounds[i]!);
  return next;
}

function setAgent(sc: ScopeState, id: RoleId, patch: Partial<AgentView>): Record<RoleId, AgentView> {
  return { ...sc.agents, [id]: { ...(sc.agents[id] ?? idleAgent()), ...patch } };
}

function settle(sc: ScopeState, to: AgentStatus, error: string | null): Record<RoleId, AgentView> {
  const agents = { ...sc.agents };
  for (const [id, a] of Object.entries(agents)) {
    if (a.status === "thinking") agents[id] = { ...a, status: to, error };
  }
  return agents;
}

/** Scope chưa biết (vd. khôi phục sau reload, mất session_started) → tạo tạm. */
function ensureScope(s: SessionState, id: string): SessionState {
  if (s.scopes[id]) return s;
  const sc =
    id === MAIN_SCOPE
      ? mainScope("", [], 0)
      : makeScope({ id, name: id, kind: "department", authorId: "", reviewerIds: [], maxRounds: 0 });
  const order = id === MAIN_SCOPE ? [MAIN_SCOPE, ...s.scopeOrder.filter((x) => x !== MAIN_SCOPE)] : [...s.scopeOrder, id];
  return { ...s, scopes: { ...s.scopes, [id]: sc }, scopeOrder: order };
}

function patchScope(s: SessionState, id: string, f: (sc: ScopeState) => ScopeState): SessionState {
  const base = ensureScope(s, id);
  return { ...base, scopes: { ...base.scopes, [id]: f(base.scopes[id]!) } };
}

function mapScopes(s: SessionState, f: (sc: ScopeState) => ScopeState): SessionState {
  return { ...s, scopes: Object.fromEntries(Object.entries(s.scopes).map(([k, v]) => [k, f(v)])) };
}

/** Role chưa có trong danh sách reviewer (scope tạm) → thêm vào để hiển thị. */
function withReviewer(sc: ScopeState, id: RoleId): ScopeState {
  if (id === sc.authorId || sc.reviewerIds.includes(id)) return sc;
  return { ...sc, reviewerIds: [...sc.reviewerIds, id] };
}

// ───────────────────────── reducer ─────────────────────────

const say = (agentId: RoleId, text: string, tone: Speech["tone"], seq: number): Speech => ({ agentId, text, tone, seq });

function reduce(s: SessionState, env: EventEnvelope): SessionState {
  switch (env.type) {
    case "session_started":
      return s; // xử lý ở applyEvent

    case "usage_updated": {
      const { prompt_tokens: p, completion_tokens: c, estimated: e } = env;
      return {
        ...s,
        usage: {
          ...s.usage,
          total: env.total,
          by_role: addUsage(s.usage.by_role, env.agent, p, c, e),
          by_scope: addUsage(s.usage.by_scope, env.scope, p, c, e),
          by_kind: addUsage(s.usage.by_kind, env.kind, p, c, e),
        },
      };
    }

    case "tokens_saved":
      return {
        ...s,
        usage: {
          ...s.usage,
          saved_tokens: env.total_saved,
          carried_reviews: (s.usage.carried_reviews ?? 0) + (env.carried ? 1 : 0),
        },
      };

    case "staffing_decided":
      return { ...s, staffing: env.staffing };

    case "membership_changed": {
      const ch = env.change;
      const cast = env.member && !s.cast.some((c) => c.id === env.member!.id) ? [...s.cast, env.member] : s.cast;
      return patchScope({ ...s, cast }, ch.scope, (sc0) => {
        let sc: ScopeState = { ...sc0, membership: [...sc0.membership, ch] };
        if (ch.action === "invited") {
          sc = withReviewer(sc, ch.agent);
          return { ...sc, agents: setAgent(sc, ch.agent, idleAgent()), speech: say(ch.agent, `Đã được mời vào họp: ${ch.reason}`, "info", env.seq) };
        }
        sc = { ...sc, reviewerIds: sc.reviewerIds.filter((id) => id !== ch.agent) };
        return ch.action === "abstained" ? { ...sc, speech: say(ch.agent, `Xin phép rút: ${ch.reason}`, "info", env.seq) } : sc;
      });
    }

    case "tool_used": {
      const a = env.activity;
      return patchScope(s, a.scope, (sc) => ({
        ...sc,
        activities: [...sc.activities, a],
        speech: say(a.agent, `${a.ok ? "🔎" : "⚠️"} ${a.summary}`, a.ok ? "info" : "error", env.seq),
      }));
    }

    case "departments_assigned": {
      // Chủ trì giao việc xong → thôi "thinking" trong lúc chờ các phòng ban.
      let next = patchScope(s, MAIN_SCOPE, (sc) => ({
        ...sc,
        agents: settle(sc, "idle", null),
        speech: sc.authorId
          ? say(sc.authorId, `Đã giao việc cho ${env.assignments.length} phòng ban. Mời các phòng họp trước!`, "info", env.seq)
          : sc.speech,
      }));
      for (const a of env.assignments) next = patchScope(next, a.department_id, (sc) => ({ ...sc, task: a.task }));
      return next;
    }

    case "agent_thinking": {
      const next = patchScope(s, env.scope, (sc0) => {
        let sc = sc0;
        if (!sc.authorId && env.plan_version > 0 && !sc.reviewerIds.includes(env.agent) && sc.rounds.length === 0) {
          sc = { ...sc, authorId: env.agent }; // scope tạm: call đầu tiên là của người viết
        }
        // round 0 = chủ trì đang giao việc cho phòng ban
        if (env.agent === sc.authorId || env.plan_version === 0) {
          return {
            ...sc,
            agents: setAgent(sc, env.agent, { status: "thinking", planVersion: env.plan_version, error: null }),
            plans:
              env.plan_version > 0
                ? upsertPlan(sc.plans, env.plan_version, (p) => ({ ...p, markdown: "", complete: false }))
                : sc.plans,
          };
        }
        sc = withReviewer(sc, env.agent);
        return {
          ...sc,
          currentRound: Math.max(sc.currentRound, env.round),
          agents: setAgent(sc, env.agent, {
            status: "thinking",
            round: env.round,
            planVersion: env.plan_version,
            review: null,
            error: null,
          }),
          rounds: upsertRound(sc, env.round, env.plan_version, (r) => ({
            ...r,
            slots: { ...r.slots, [env.agent]: { review: null, error: null, pending: true } },
          })),
        };
      });
      // Nhánh người viết ở trên đã reset markdown của plan → bỏ luôn bản nháp cũ (nếu có).
      // authorId sau patch đúng bằng authorId mà nhánh trên dùng để chọn nhánh.
      return env.plan_version > 0 && env.agent === next.scopes[env.scope]?.authorId
        ? dropDraft(next, env.scope, env.plan_version)
        : next;
    }

    case "plan_stream_reset":
      return dropDraft(
        patchScope(s, env.scope, (sc) => ({
          ...sc,
          plans: upsertPlan(sc.plans, env.version, (p) => ({ ...p, markdown: "", complete: false })),
        })),
        env.scope,
        env.version,
      );

    case "plan_delta": {
      // Đường nhanh: plan đã có và đang viết dở → chỉ nối vào `drafts`, giữ nguyên tham chiếu
      // `scopes` / `plans` (Timeline, phòng họp… không re-render). `planText()` cho ra đúng
      // chuỗi như cách cũ `p.markdown + delta`.
      const plan = s.scopes[env.scope]?.plans.find((p) => p.version === env.version);
      if (plan && !plan.complete) {
        const key = draftKey(env.scope, env.version);
        return { ...s, drafts: { ...s.drafts, [key]: (s.drafts[key] ?? plan.markdown) + env.delta } };
      }
      // Plan chưa có (vd. khôi phục sau reload) hoặc đã complete → giữ nguyên logic cũ.
      return patchScope(s, env.scope, (sc) => ({
        ...sc,
        plans: upsertPlan(sc.plans, env.version, (p) => ({ ...p, markdown: p.markdown + env.delta })),
      }));
    }

    case "plan_ready": {
      const next = patchScope(s, env.scope, (sc) => ({
        ...sc,
        plans: upsertPlan(sc.plans, env.version, () => ({ version: env.version, markdown: env.markdown, complete: true })),
        agents: sc.authorId ? setAgent(sc, sc.authorId, { status: "done", planVersion: env.version, error: null }) : sc.agents,
        speech: sc.authorId
          ? say(
              sc.authorId,
              env.version === 1
                ? sc.reviewerIds.length
                  ? "Plan v1 xong rồi, mời mọi người phản biện!"
                  : "Plan v1 xong rồi."
                : `Đã sửa theo góp ý, mời xem plan v${env.version}.`,
              "info",
              env.seq,
            )
          : sc.speech,
      }));
      // plan_ready thay toàn bộ text → bản nháp không còn dùng nữa.
      return dropDraft(next, env.scope, env.version);
    }

    case "review_ready":
      return patchScope(s, env.scope, (sc0) => {
        const id = env.review.agent;
        const sc = withReviewer(sc0, id);
        const planVersion = sc.agents[id]?.planVersion ?? 0;
        return {
          ...sc,
          agents: setAgent(sc, id, { status: "done", round: env.round, review: env.review, error: null }),
          speech: say(id, env.review.summary_bubble, env.review.verdict === "APPROVE" ? "approve" : "reject", env.seq),
          rounds: upsertRound(sc, env.round, planVersion, (r) => ({
            ...r,
            slots: { ...r.slots, [id]: { review: env.review, error: null, pending: false } },
          })),
        };
      });

    case "review_failed":
      return patchScope(s, env.scope, (sc0) => {
        const sc = withReviewer(sc0, env.agent);
        const planVersion = sc.agents[env.agent]?.planVersion ?? 0;
        return {
          ...sc,
          agents: setAgent(sc, env.agent, { status: "error", round: env.round, review: null, error: env.error }),
          speech: say(env.agent, `Lỗi: ${env.error}`, "error", env.seq),
          rounds: upsertRound(sc, env.round, planVersion, (r) => ({
            ...r,
            slots: { ...r.slots, [env.agent]: { review: null, error: env.error, pending: false } },
          })),
        };
      });

    case "truncated": {
      const what = TRUNCATE_KIND[env.call_kind] ?? env.call_kind;
      const why = TRUNCATE_REASON[env.reason] ?? env.reason;
      const text = env.recovered
        ? `${what} bị cắt (${why}) — đã viết tiếp và ghép lại.`
        : `${what} bị cắt (${why}) — không dùng được.`;
      const base = env.recovered ? s : { ...s, lastTruncated: { scope: env.scope, reason: env.reason } };
      return patchScope(base, env.scope, (sc) => ({
        ...sc,
        notices: [
          ...(sc.notices ?? []),
          { key: `n${env.seq}`, agentId: env.agent, planVersion: sc.plans.at(-1)?.version ?? 0, text },
        ],
      }));
    }

    case "revise_fallback": {
      const why = FALLBACK_REASON[env.reason] ?? env.reason;
      s = { ...s, fallbackStreak: env.consecutive };
      const text =
        `Không sửa theo section được (${why}) — viết lại toàn bộ plan.` +
        (env.consecutive >= 2 ? ` ${env.consecutive} lần liên tiếp → các vòng sau luôn viết lại toàn bộ.` : "");
      return patchScope(s, env.scope, (sc) => ({
        ...sc,
        notices: [
          ...(sc.notices ?? []),
          { key: `n${env.seq}`, agentId: sc.authorId, planVersion: sc.plans.at(-1)?.version ?? 0, text },
        ],
      }));
    }

    case "consistency": {
      s = { ...s, consistency: { scope: env.scope, result: env.result, conflicts: env.conflicts } };
      const list = env.conflicts.map((c) => `[${c.sections.join(", ")}] ${c.text}`).join("; ");
      const text =
        env.result === "ok"
          ? "Kiểm tra nhất quán: các section không mâu thuẫn."
          : env.result === "conflicts"
            ? `Kiểm tra nhất quán phát hiện mâu thuẫn: ${list}`
            : "Kiểm tra nhất quán: không xác minh được (sai định dạng, bị cắt hoặc hết ngân sách).";
      return patchScope(s, env.scope, (sc) => ({
        ...sc,
        notices: [
          ...(sc.notices ?? []),
          { key: `n${env.seq}`, agentId: sc.authorId, planVersion: sc.plans.at(-1)?.version ?? 0, text },
        ],
      }));
    }

    case "budget":
      return {
        ...s,
        budget: {
          used: env.used,
          reserved: env.reserved,
          limit: env.limit,
          estimated: env.estimated,
          soft: env.soft,
          state: env.state,
        },
      };

    case "round_completed":
      return patchScope(s, env.scope, (sc) => ({
        ...sc,
        rounds: upsertRound(sc, env.round, env.plan_version, (r) => ({
          ...r,
          planVersion: env.plan_version,
          approvals: env.approvals,
          total: env.total,
          converged: env.converged,
          completed: true,
        })),
      }));

    case "department_finished":
      return patchScope(s, env.department_id, (sc) => {
        const last = sc.plans.at(-1);
        return {
          ...sc,
          status: env.status,
          error: env.error,
          final:
            env.final_markdown != null
              ? { version: last?.version ?? 1, markdown: env.final_markdown, status: env.status }
              : null,
          agents: settle(sc, env.error ? "error" : "idle", env.error),
          speech: sc.authorId
            ? say(
                sc.authorId,
                env.error ? `Phòng ban gặp lỗi: ${env.error}` : "Phòng ban đã chốt plan, gửi lên cuộc họp chính!",
                env.error ? "error" : "approve",
                env.seq,
              )
            : sc.speech,
        };
      });

    case "finished": {
      const next = patchScope(s, MAIN_SCOPE, (sc) => ({
        ...sc,
        status: env.status,
        final: { version: env.final_version, markdown: env.final_markdown, status: env.status },
      }));
      return { ...mapScopes(next, (sc) => ({ ...sc, agents: settle(sc, "idle", null) })), status: env.status };
    }

    case "cancelled":
      return {
        ...mapScopes(s, (sc) => ({
          ...sc,
          status: sc.status === "running" ? "cancelled" : sc.status,
          agents: settle(sc, "idle", null),
        })),
        status: "cancelled",
      };

    case "failed":
      return {
        ...mapScopes(s, (sc) => ({
          ...sc,
          status: sc.status === "running" ? "failed" : sc.status,
          agents: settle(sc, "error", env.error),
        })),
        status: "failed",
        error: env.error,
      };
  }
}

/**
 * Áp 1 event vào state:
 * - bỏ qua event của session khác;
 * - bỏ qua event trùng (seq ≤ lastSeq);
 * - đánh dấu `seqGap` nếu seq nhảy.
 */
export function applyEvent(s: SessionState, env: EventEnvelope): SessionState {
  if (env.type === "session_started") {
    if (s.id !== null && s.id !== env.session_id && s.status === "running") return s;
    if (s.id === env.session_id && s.lastSeq >= env.seq) return s;
    const scopes: Record<string, ScopeState> = {
      [MAIN_SCOPE]: mainScope(env.chair_id, env.reviewer_ids, env.max_rounds),
    };
    for (const d of env.departments) scopes[d.id] = deptScope(d);
    return {
      ...initialSession(),
      id: env.session_id,
      prompt: env.prompt,
      status: "running",
      cast: env.cast,
      options: env.options ?? null,
      scopeOrder: [MAIN_SCOPE, ...env.departments.map((d) => d.id)],
      scopes,
      lastSeq: env.seq,
      seqGap: env.seq !== 0,
      createdAt: env.ts,
    };
  }
  if (s.id !== env.session_id || s.readOnly) return s;
  if (s.lastSeq >= 0 && env.seq <= s.lastSeq) return s;
  const gap = s.lastSeq >= 0 && env.seq !== s.lastSeq + 1;
  const next = reduce(s, env);
  return { ...next, lastSeq: env.seq, seqGap: s.seqGap || gap };
}

// ───────────────────────── từ bản ghi đã lưu ─────────────────────────

function scopeFromRecord(
  base: ScopeState,
  plansRec: SessionRecord["plans"],
  roundsRec: RoundRecord[],
  final: string | null,
  status: SessionStatus,
  error: string | null,
): ScopeState {
  // Phiên cũ không lưu reviewer_ids → suy ra từ slot.
  const reviewerIds = base.reviewerIds.length
    ? base.reviewerIds
    : [...new Set(roundsRec.flatMap((r) => r.slots.map((s) => s.agent)))];
  const sc: ScopeState = { ...base, reviewerIds };
  const plans: PlanView[] = plansRec.map((p) => ({ version: p.version, markdown: p.markdown, complete: true }));
  const rounds: RoundView[] = roundsRec.map((r) => {
    const slots: Record<RoleId, SlotView> = {};
    for (const id of reviewerIds) slots[id] = { review: null, error: null, pending: false };
    for (const slot of r.slots) slots[slot.agent] = { review: slot.review, error: slot.error, pending: false };
    return {
      round: r.round,
      planVersion: r.plan_version,
      slots,
      approvals: r.approvals,
      total: r.slots.length,
      converged: r.converged,
      completed: true,
    };
  });

  const agents: Record<RoleId, AgentView> = {};
  for (const id of [sc.authorId, ...reviewerIds]) if (id) agents[id] = idleAgent();
  const lastPlan = plans.at(-1);
  if (sc.authorId) {
    agents[sc.authorId] =
      status === "failed" && plans.length === 0
        ? { ...idleAgent(), status: "error", error }
        : { ...idleAgent(), status: lastPlan ? "done" : "idle", planVersion: lastPlan?.version ?? 0 };
  }
  const lastRound = rounds.at(-1);
  if (lastRound) {
    for (const id of reviewerIds) {
      const slot = lastRound.slots[id];
      if (!slot) continue;
      agents[id] = {
        status: slot.review ? "done" : slot.error ? "error" : "idle",
        round: lastRound.round,
        planVersion: lastRound.planVersion,
        review: slot.review,
        error: slot.error,
      };
    }
  }
  return {
    ...sc,
    status,
    currentRound: rounds.length,
    plans,
    rounds,
    agents,
    final: final != null && lastPlan ? { version: lastPlan.version, markdown: final, status } : null,
    error,
  };
}

export function fromRecord(rec: SessionRecord, readOnly = true): SessionState {
  const s = initialSession();
  const scopes: Record<string, ScopeState> = {
    [MAIN_SCOPE]: scopeFromRecord(
      mainScope(rec.chair_id ?? "architect", rec.reviewer_ids ?? [], rec.max_rounds, rec.status),
      rec.plans,
      rec.rounds,
      rec.final_markdown,
      rec.status,
      rec.error,
    ),
  };
  for (const d of rec.departments ?? []) {
    const base = deptScope(
      { id: d.id, name: d.name, lead_id: d.lead_id, member_ids: d.member_ids, max_rounds: d.max_rounds },
      d.status,
    );
    scopes[d.id] = { ...scopeFromRecord(base, d.plans, d.rounds, d.final_markdown, d.status, d.error), task: d.task };
  }
  for (const a of rec.activities ?? []) {
    const sc = scopes[a.scope];
    if (sc) sc.activities = [...sc.activities, a];
  }
  for (const m of rec.membership ?? []) {
    const sc = scopes[m.scope];
    if (!sc) continue;
    sc.membership = [...sc.membership, m];
    if (m.action === "invited") sc.reviewerIds = sc.reviewerIds.includes(m.agent) ? sc.reviewerIds : [...sc.reviewerIds, m.agent];
    else sc.reviewerIds = sc.reviewerIds.filter((id) => id !== m.agent);
  }
  return {
    ...s,
    id: rec.id,
    prompt: rec.prompt,
    status: rec.status,
    cast: rec.cast ?? [],
    usage: rec.usage ?? emptyReport(),
    options: rec.options ?? null,
    staffing: rec.staffing ?? null,
    scopeOrder: [MAIN_SCOPE, ...(rec.departments ?? []).map((d) => d.id)],
    scopes,
    error: rec.error,
    readOnly,
    createdAt: rec.created_at,
  };
}
