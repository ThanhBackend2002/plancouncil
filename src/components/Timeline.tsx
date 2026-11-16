import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FileText, FolderTree, Globe, Link, LoaderCircle, Search, TriangleExclamationPoint, UserMinus, UserPlus, Wrench } from "lucide-react";
import { STATUS_META, type RoleResolver } from "../lib/agents";
import type { MembershipChange, RoleId, ToolActivity } from "../lib/contracts";
import { useActiveShallow } from "../store/council";
import type { RoundView, ScopeState, SlotView } from "../store/session";
import { useUi } from "../store/ui";
import { PixelPortrait } from "./PixelCharacter";
import { Badge, Panel, VerdictBadge, cx } from "./ui";

type Msg =
  | { key: string; kind: "task"; text: string }
  | { key: string; kind: "plan"; agentId: RoleId; version: number; complete: boolean }
  | { key: string; kind: "review"; agentId: RoleId; round: number; slot: SlotView }
  | { key: string; kind: "round"; round: RoundView }
  | { key: string; kind: "tool"; activity: ToolActivity }
  | { key: string; kind: "member"; change: MembershipChange }
  | { key: string; kind: "outcome" };

function buildMessages(scope: ScopeState, running: boolean): Msg[] {
  const out: Msg[] = [];
  if (scope.task) out.push({ key: "task", kind: "task", text: scope.task });
  const tools = (phase: "author" | "review", step: number, agent: RoleId) =>
    scope.activities
      .map((a, i) => ({ a, i }))
      .filter(({ a }) => a.phase === phase && a.step === step && a.agent === agent)
      .forEach(({ a, i }) => out.push({ key: `t${i}`, kind: "tool", activity: a }));
  for (const p of scope.plans) {
    if (scope.authorId) tools("author", p.version, scope.authorId);
    if (scope.authorId) out.push({ key: `p${p.version}`, kind: "plan", agentId: scope.authorId, version: p.version, complete: p.complete });
    for (const r of scope.rounds.filter((x) => x.planVersion === p.version)) {
      // Mời vào / mời ra có hiệu lực từ đầu vòng; tự rút xảy ra sau lượt review của họ.
      scope.membership
        .map((m, i) => ({ m, i }))
        .filter(({ m }) => m.round === r.round && m.action !== "abstained")
        .forEach(({ m, i }) => out.push({ key: `m${i}`, kind: "member", change: m }));
      const ids = [...new Set([...scope.reviewerIds, ...Object.keys(r.slots)])];
      for (const id of ids) {
        tools("review", r.round, id);
        const slot = r.slots[id];
        if (slot && (slot.review || slot.error || slot.pending)) {
          out.push({ key: `r${r.round}-${id}`, kind: "review", agentId: id, round: r.round, slot });
        }
      }
      scope.membership
        .map((m, i) => ({ m, i }))
        .filter(({ m }) => m.round === r.round && m.action === "abstained")
        .forEach(({ m, i }) => out.push({ key: `m${i}`, kind: "member", change: m }));
      if (r.completed) out.push({ key: `rc${r.round}`, kind: "round", round: r });
    }
  }
  if (!running && scope.status !== "idle" && scope.status !== "running") out.push({ key: "outcome", kind: "outcome" });
  return out;
}

export const Timeline = memo(function Timeline({ scope, resolve }: { scope: ScopeState; resolve: RoleResolver }) {
  // Chỉ lấy cờ nguyên thuỷ của phiên; `scope` giữ tham chiếu khi plan_delta → không rebuild hội thoại theo delta.
  const session = useActiveShallow((s) => ({
    status: s.status,
    id: s.id,
    seqGap: s.seqGap,
    readOnly: s.readOnly,
    resumed: s.resumed,
  }));
  const running = scope.status === "running" && session.status === "running";
  const msgs = useMemo(() => buildMessages(scope, running), [scope, running]);
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [msgs]);
  useLayoutEffect(() => {
    stick.current = true;
  }, [scope.id, session.id]);

  return (
    <Panel className="flex min-h-[140px] flex-1 flex-col overflow-hidden">
      <LiveAnnouncer scope={scope} resolve={resolve} sessionKey={session.id} />
      <div className="flex items-center gap-2 border-b border-line px-3 py-2 text-xs font-medium text-fg-muted">
        Hội thoại · {scope.name}
        <span className="ml-auto font-normal text-fg-subtle">
          {scope.rounds.length} round · {scope.reviewerIds.length} reviewer
        </span>
      </div>
      <div
        ref={ref}
        onScroll={() => {
          const el = ref.current;
          if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className="min-h-0 flex-1 space-y-2.5 overflow-y-auto p-3"
      >
        {session.seqGap && !session.readOnly && (
          <Notice>Có event bị lỡ. Dữ liệu đầy đủ sẽ được nạp lại từ lịch sử khi phiên kết thúc.</Notice>
        )}
        {session.resumed && session.status === "running" && (
          <Notice>Webview vừa reload: chỉ hiển thị hội thoại từ thời điểm kết nối lại.</Notice>
        )}
        {msgs.length === 0 && (
          <p className="px-2 py-6 text-center text-xs text-fg-subtle">
            {running ? `${resolve(scope.authorId).name || "Người viết"} đang chuẩn bị bản nháp…` : "Chưa có hội thoại nào."}
          </p>
        )}
        {msgs.map((m) => {
          switch (m.kind) {
            case "task":
              return <SystemLine key={m.key} text={`Nhiệm vụ được giao: ${m.text}`} />;
            // Props là giá trị nguyên thuỷ hoặc tham chiếu có sẵn trong state (slot, round, activity…),
            // không truyền object `m` dựng lại mỗi lần buildMessages → memo con mới có tác dụng.
            case "plan":
              return <PlanMsg key={m.key} agentId={m.agentId} version={m.version} complete={m.complete} resolve={resolve} />;
            case "review":
              return <ReviewMsg key={m.key} agentId={m.agentId} slot={m.slot} resolve={resolve} />;
            case "round":
              return <RoundLine key={m.key} round={m.round} />;
            case "tool":
              return <ToolLine key={m.key} activity={m.activity} resolve={resolve} />;
            case "member":
              return <MemberLine key={m.key} change={m.change} resolve={resolve} />;
            case "outcome":
              return (
                <Outcome
                  key={m.key}
                  status={scope.status}
                  dept={scope.kind === "department"}
                  reviewers={scope.reviewerIds.length}
                  rounds={scope.rounds.length}
                  maxRounds={scope.maxRounds}
                  error={scope.error}
                />
              );
          }
        })}
      </div>
    </Panel>
  );
});

const Notice = memo(function Notice({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-2 rounded-control border border-warn-line bg-warn-subtle px-2.5 py-1.5 text-[11px] text-warn">
      <TriangleExclamationPoint className="mt-px size-3.5 shrink-0" />
      {children}
    </div>
  );
});

const SystemLine = memo(function SystemLine({ text }: { text: string }) {
  return (
    <div className="flex items-center gap-2 text-[11px] text-fg-subtle">
      <span className="h-px flex-1 bg-surface-2" />
      <span className="max-w-[85%] text-center break-words">{text}</span>
      <span className="h-px flex-1 bg-surface-2" />
    </div>
  );
});

function Bubble({
  agentId,
  resolve,
  children,
  accent,
}: {
  agentId: RoleId;
  resolve: RoleResolver;
  children: React.ReactNode;
  accent?: string;
}) {
  const m = resolve(agentId);
  return (
    <div className="flex items-start gap-2">
      <PixelPortrait meta={m} size={30} />
      <div className="min-w-0 flex-1">
        <div className="mb-0.5 flex items-baseline gap-1.5">
          <span className="px-font text-xs font-bold" style={{ color: m.color }}>
            {m.name}
          </span>
          {m.title && m.title !== m.name && <span className="truncate text-[10px] text-fg-subtle">{m.title}</span>}
        </div>
        <div
          className="rounded-control rounded-tl-none border bg-surface/70 px-2.5 py-1.5 text-xs text-fg-soft"
          style={{ borderColor: accent ?? "var(--line)" }}
        >
          {children}
        </div>
      </div>
    </div>
  );
}

const PlanMsg = memo(function PlanMsg({
  agentId,
  version,
  complete,
  resolve,
}: {
  agentId: RoleId;
  version: number;
  complete: boolean;
  resolve: RoleResolver;
}) {
  const focus = useUi((s) => s.focusPlan);
  const first = version === 1;
  return (
    <Bubble agentId={agentId} resolve={resolve}>
      {complete ? (
        <span className="flex items-center gap-2">
          <FileText className="size-3.5 shrink-0 text-accent-fg" />
          <span className="flex-1">
            {first ? "Đây là bản nháp Plan v1, mời mọi người góp ý." : `Đã chỉnh sửa theo góp ý → Plan v${version}.`}
          </span>
          <button type="button" onClick={() => focus(version)} className="shrink-0 text-[11px] text-accent-fg hover:text-accent-fg">
            Xem v{version} →
          </button>
        </span>
      ) : (
        <span className="inline-flex items-center gap-1.5 text-fg-muted">
          <LoaderCircle className="size-3 animate-spin" /> Đang viết Plan v{version}…
        </span>
      )}
    </Bubble>
  );
});

const ReviewMsg = memo(function ReviewMsg({ agentId, slot, resolve }: { agentId: RoleId; slot: SlotView; resolve: RoleResolver }) {
  const [open, setOpen] = useState(false);
  const r = slot.review;
  const accent = r ? (r.verdict === "APPROVE" ? "var(--success-line)" : "var(--danger-line)") : slot.error ? "var(--warn-line)" : undefined;
  const expandable =
    !!r &&
    (r.critical_issues.length > 0 ||
      r.suggestions.length > 0 ||
      !!r.policy_override ||
      (r.out_of_scope?.length ?? 0) > 0 ||
      (r.invites?.length ?? 0) > 0);

  return (
    <Bubble agentId={agentId} resolve={resolve} accent={accent}>
      {slot.pending ? (
        <span className="inline-flex items-center gap-1.5 text-fg-subtle">
          <LoaderCircle className="size-3 animate-spin" /> đang đọc plan…
        </span>
      ) : slot.error ? (
        <span className="break-words text-warn">Không review được: {slot.error}</span>
      ) : r ? (
        <>
          <button
            type="button"
            disabled={!expandable}
            onClick={() => setOpen((o) => !o)}
            aria-expanded={expandable ? open : undefined}
            className="flex w-full items-start gap-1.5 text-left"
          >
            <VerdictBadge verdict={r.verdict} carried={r.carried_over} />
            {r.policy_override && (
              <span title={r.policy_override}>
                <TriangleExclamationPoint className="mt-0.5 size-3.5 text-warn" />
              </span>
            )}
            <span className="min-w-0 flex-1 break-words">{r.summary_bubble}</span>
            {expandable &&
              (open ? (
                <ChevronDown className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" />
              ) : (
                <ChevronRight className="mt-0.5 size-3.5 shrink-0 text-fg-subtle" />
              ))}
          </button>
          {!open && expandable && (
            <div className="mt-1 text-[10px] text-fg-subtle">
              {r.critical_issues.length} vấn đề · {r.suggestions.length} gợi ý
            </div>
          )}
          {open && (
            <div className="mt-2 space-y-2 border-t border-line pt-2">
              {r.policy_override && <p className="text-warn">{r.policy_override}</p>}
              {r.critical_issues.length > 0 && (
                <div>
                  <div className="mb-1 font-medium text-danger">Critical issues</div>
                  <ol className="list-decimal space-y-1 pl-4 marker:text-fg-faint">
                    {r.critical_issues.map((it, i) => (
                      <li key={i} className="break-words">
                        {it}
                      </li>
                    ))}
                  </ol>
                </div>
              )}
              {(r.out_of_scope?.length ?? 0) > 0 && (
                <div>
                  <div className="mb-1 font-medium text-fg-muted">Ngoài phạm vi (không bắt buộc sửa)</div>
                  <ul className="list-disc space-y-1 pl-4 text-fg-subtle marker:text-fg-faint">
                    {r.out_of_scope!.map((it, i) => (
                      <li key={i} className="break-words">
                        {it}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {(r.invites?.length ?? 0) > 0 && (
                <div className="text-success/90">
                  Đề nghị mời: {r.invites!.map((i) => `${resolve(i.role_id).name}${i.reason ? ` (${i.reason})` : ""}`).join(", ")}
                </div>
              )}
              {r.suggestions.length > 0 && (
                <div>
                  <div className="mb-1 font-medium text-info">Gợi ý</div>
                  <ul className="list-disc space-y-1 pl-4 text-fg-muted marker:text-fg-faint">
                    {r.suggestions.map((it, i) => (
                      <li key={i} className="break-words">
                        {it}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          )}
        </>
      ) : null}
    </Bubble>
  );
});

const TOOL_ICON: Record<string, typeof FileText> = {
  read_file: FileText,
  list_files: FolderTree,
  search_code: Search,
  web_search: Globe,
  fetch_url: Link,
};

const ToolLine = memo(function ToolLine({ activity: a, resolve }: { activity: ToolActivity; resolve: RoleResolver }) {
  const m = resolve(a.agent);
  const Icon = TOOL_ICON[a.tool] ?? Wrench;
  return (
    <div className="flex items-center gap-2 pl-9 text-[11px]" title={a.summary}>
      <Icon className={cx("size-3.5 shrink-0", a.ok ? "text-info" : "text-warn")} />
      <span className="truncate text-fg-subtle">
        <span style={{ color: m.color }}>{m.name}</span> {a.summary}
      </span>
    </div>
  );
});

const MemberLine = memo(function MemberLine({ change: c, resolve }: { change: MembershipChange; resolve: RoleResolver }) {
  const m = resolve(c.agent);
  const by = c.by === "user" ? "bạn" : c.by === "self" ? "" : resolve(c.by).name;
  const text =
    c.action === "invited"
      ? `được mời vào họp${by ? ` (đề nghị bởi ${by})` : ""}`
      : c.action === "dismissed"
        ? "được mời ra khỏi cuộc họp"
        : "tự rút khỏi cuộc họp";
  const Icon = c.action === "invited" ? UserPlus : UserMinus;
  return (
    <div className="flex items-start gap-2 rounded-control bg-surface/40 px-2 py-1.5 text-[11px] text-fg-muted">
      <Icon className={cx("mt-px size-3.5 shrink-0", c.action === "invited" ? "text-success" : "text-fg-subtle")} />
      <PixelPortrait meta={m} size={16} />
      <span className="min-w-0 break-words">
        <b style={{ color: m.color }}>{m.name}</b> {text}
        {c.reason && <span className="text-fg-subtle"> — {c.reason}</span>}
      </span>
    </div>
  );
});

const RoundLine = memo(function RoundLine({ round }: { round: RoundView }) {
  const failed = Object.values(round.slots).filter((s) => s.error).length;
  return (
    <div className="flex items-center justify-center gap-1.5 text-[11px] text-fg-subtle">
      <span className="h-px flex-1 bg-surface-2" />
      Round {round.round}:
      <Badge tone={round.converged ? "success" : round.approvals > 0 ? "warning" : "danger"}>
        {round.approvals}/{round.total} APPROVE
      </Badge>
      {failed > 0 && <Badge tone="warning">{failed} lỗi</Badge>}
      <span className="h-px flex-1 bg-surface-2" />
    </div>
  );
});

const Outcome = memo(function Outcome({
  status,
  dept,
  reviewers,
  rounds,
  maxRounds,
  error,
}: {
  status: ScopeState["status"];
  dept: boolean;
  reviewers: number;
  rounds: number;
  maxRounds: number;
  error: string | null;
}) {
  const meta = STATUS_META[status];
  const text: Record<string, string> = {
    converged: dept
      ? reviewers
        ? `Phòng ban đã thống nhất ở round ${rounds}. Plan được gửi lên cuộc họp chính.`
        : "Plan của trưởng phòng được gửi lên cuộc họp chính."
      : `Tất cả reviewer đã APPROVE ở round ${rounds}.`,
    partially_approved: "Các reviewer còn hoạt động đã APPROVE, nhưng có reviewer không review được.",
    max_rounds_reached: `Hết ${maxRounds} vòng mà vẫn còn REJECT. Final có kèm mục Open Issues.`,
    cancelled: "Phiên đã bị dừng.",
    failed: error ?? "Gặp lỗi.",
  };
  return (
    <div className={cx("flex items-start gap-2 rounded-control border border-line bg-canvas/60 px-2.5 py-2 text-xs text-fg-soft")}>
      <Badge tone={meta.tone}>{meta.label}</Badge>
      <span className="break-words">{text[status]}</span>
    </div>
  );
});

/** Các sự kiện "đã xong" (plan version viết xong, lượt review xong) — không gồm delta đang stream. */
function completedEvents(scope: ScopeState, resolve: RoleResolver): { key: string; text: string }[] {
  const out: { key: string; text: string }[] = [];
  const where = scope.kind === "main" ? "" : ` (${scope.name})`;
  for (const p of scope.plans) {
    if (p.complete) out.push({ key: `p${p.version}`, text: `Plan v${p.version} đã sẵn sàng${where}` });
  }
  for (const r of scope.rounds) {
    for (const [id, slot] of Object.entries(r.slots)) {
      const name = resolve(id).name;
      if (slot.review) out.push({ key: `r${r.round}-${id}`, text: `${name} đã review xong round ${r.round}: ${slot.review.verdict}${where}` });
      else if (slot.error) out.push({ key: `r${r.round}-${id}`, text: `${name} không review được round ${r.round}${where}` });
    }
  }
  return out;
}

/** Vùng ẩn cho trình đọc màn hình: chỉ đọc khi có tin nhắn hoàn chỉnh mới, không đọc từng delta. */
const LiveAnnouncer = memo(function LiveAnnouncer({
  scope,
  resolve,
  sessionKey,
}: {
  scope: ScopeState;
  resolve: RoleResolver;
  sessionKey: string | null;
}) {
  const [text, setText] = useState("");
  const seen = useRef<{ key: string; ids: Set<string> } | null>(null);

  useEffect(() => {
    const key = `${sessionKey ?? ""}:${scope.id}`;
    const events = completedEvents(scope, resolve);
    // Đổi phiên / đổi nơi họp / mở phiên cũ: ghi nhận hiện trạng, không đọc lại cả lịch sử.
    if (!seen.current || seen.current.key !== key) {
      seen.current = { key, ids: new Set(events.map((e) => e.key)) };
      setText("");
      return;
    }
    const ids = seen.current.ids;
    const fresh = events.filter((e) => !ids.has(e.key));
    if (fresh.length === 0) return;
    for (const e of fresh) ids.add(e.key);
    setText(fresh.map((e) => e.text).join(". "));
  }, [scope, resolve, sessionKey]);

  return (
    <div className="sr-only" aria-live="polite" aria-atomic="true">
      {text}
    </div>
  );
});
