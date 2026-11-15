import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FileText, FolderTree, Globe, Link, LoaderCircle, Search, TriangleExclamationPoint, UserMinus, UserPlus, Wrench } from "lucide-react";
import { STATUS_META, type RoleResolver } from "../lib/agents";
import type { MembershipChange, RoleId, ToolActivity } from "../lib/contracts";
import type { RoundView, ScopeState, SessionState, SlotView } from "../store/session";
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

export function Timeline({ session, scope, resolve }: { session: SessionState; scope: ScopeState; resolve: RoleResolver }) {
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
      <div className="flex items-center gap-2 border-b border-neutral-800 px-3 py-2 text-xs font-medium text-neutral-400">
        Hội thoại · {scope.name}
        <span className="ml-auto font-normal text-neutral-600">
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
          <p className="px-2 py-6 text-center text-xs text-neutral-600">
            {running ? `${resolve(scope.authorId).name || "Người viết"} đang chuẩn bị bản nháp…` : "Chưa có hội thoại nào."}
          </p>
        )}
        {msgs.map((m) => {
          switch (m.kind) {
            case "task":
              return <SystemLine key={m.key} text={`Nhiệm vụ được giao: ${m.text}`} />;
            case "plan":
              return <PlanMsg key={m.key} msg={m} resolve={resolve} first={m.version === 1} />;
            case "review":
              return <ReviewMsg key={m.key} msg={m} resolve={resolve} />;
            case "round":
              return <RoundLine key={m.key} round={m.round} />;
            case "tool":
              return <ToolLine key={m.key} activity={m.activity} resolve={resolve} />;
            case "member":
              return <MemberLine key={m.key} change={m.change} resolve={resolve} />;
            case "outcome":
              return <Outcome key={m.key} scope={scope} />;
          }
        })}
      </div>
    </Panel>
  );
}

function Notice({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-2 rounded-md border border-amber-500/25 bg-amber-500/10 px-2.5 py-1.5 text-[11px] text-amber-200">
      <TriangleExclamationPoint className="mt-px size-3.5 shrink-0" />
      {children}
    </div>
  );
}

function SystemLine({ text }: { text: string }) {
  return (
    <div className="flex items-center gap-2 text-[11px] text-neutral-500">
      <span className="h-px flex-1 bg-neutral-800" />
      <span className="max-w-[85%] text-center break-words">{text}</span>
      <span className="h-px flex-1 bg-neutral-800" />
    </div>
  );
}

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
          {m.title && m.title !== m.name && <span className="truncate text-[10px] text-neutral-600">{m.title}</span>}
        </div>
        <div
          className="rounded-md rounded-tl-none border bg-neutral-900/70 px-2.5 py-1.5 text-xs text-neutral-300"
          style={{ borderColor: accent ?? "#27272a" }}
        >
          {children}
        </div>
      </div>
    </div>
  );
}

function PlanMsg({
  msg,
  resolve,
  first,
}: {
  msg: Extract<Msg, { kind: "plan" }>;
  resolve: RoleResolver;
  first: boolean;
}) {
  const focus = useUi((s) => s.focusPlan);
  return (
    <Bubble agentId={msg.agentId} resolve={resolve}>
      {msg.complete ? (
        <span className="flex items-center gap-2">
          <FileText className="size-3.5 shrink-0 text-violet-400" />
          <span className="flex-1">
            {first ? "Đây là bản nháp Plan v1, mời mọi người góp ý." : `Đã chỉnh sửa theo góp ý → Plan v${msg.version}.`}
          </span>
          <button type="button" onClick={() => focus(msg.version)} className="shrink-0 text-[11px] text-violet-300 hover:text-violet-200">
            Xem v{msg.version} →
          </button>
        </span>
      ) : (
        <span className="inline-flex items-center gap-1.5 text-neutral-400">
          <LoaderCircle className="size-3 animate-spin" /> Đang viết Plan v{msg.version}…
        </span>
      )}
    </Bubble>
  );
}

function ReviewMsg({ msg, resolve }: { msg: Extract<Msg, { kind: "review" }>; resolve: RoleResolver }) {
  const [open, setOpen] = useState(false);
  const { slot } = msg;
  const r = slot.review;
  const accent = r ? (r.verdict === "APPROVE" ? "#16a34a66" : "#dc262666") : slot.error ? "#d9770666" : undefined;
  const expandable =
    !!r &&
    (r.critical_issues.length > 0 ||
      r.suggestions.length > 0 ||
      !!r.policy_override ||
      (r.out_of_scope?.length ?? 0) > 0 ||
      (r.invites?.length ?? 0) > 0);

  return (
    <Bubble agentId={msg.agentId} resolve={resolve} accent={accent}>
      {slot.pending ? (
        <span className="inline-flex items-center gap-1.5 text-neutral-500">
          <LoaderCircle className="size-3 animate-spin" /> đang đọc plan…
        </span>
      ) : slot.error ? (
        <span className="break-words text-amber-300">Không review được: {slot.error}</span>
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
                <TriangleExclamationPoint className="mt-0.5 size-3.5 text-amber-400" />
              </span>
            )}
            <span className="min-w-0 flex-1 break-words">{r.summary_bubble}</span>
            {expandable &&
              (open ? (
                <ChevronDown className="mt-0.5 size-3.5 shrink-0 text-neutral-500" />
              ) : (
                <ChevronRight className="mt-0.5 size-3.5 shrink-0 text-neutral-500" />
              ))}
          </button>
          {!open && expandable && (
            <div className="mt-1 text-[10px] text-neutral-500">
              {r.critical_issues.length} vấn đề · {r.suggestions.length} gợi ý
            </div>
          )}
          {open && (
            <div className="mt-2 space-y-2 border-t border-neutral-800 pt-2">
              {r.policy_override && <p className="text-amber-300">{r.policy_override}</p>}
              {r.critical_issues.length > 0 && (
                <div>
                  <div className="mb-1 font-medium text-red-300">Critical issues</div>
                  <ol className="list-decimal space-y-1 pl-4 marker:text-neutral-600">
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
                  <div className="mb-1 font-medium text-neutral-400">Ngoài phạm vi (không bắt buộc sửa)</div>
                  <ul className="list-disc space-y-1 pl-4 text-neutral-500 marker:text-neutral-700">
                    {r.out_of_scope!.map((it, i) => (
                      <li key={i} className="break-words">
                        {it}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {(r.invites?.length ?? 0) > 0 && (
                <div className="text-emerald-300/90">
                  Đề nghị mời: {r.invites!.map((i) => `${resolve(i.role_id).name}${i.reason ? ` (${i.reason})` : ""}`).join(", ")}
                </div>
              )}
              {r.suggestions.length > 0 && (
                <div>
                  <div className="mb-1 font-medium text-sky-300">Gợi ý</div>
                  <ul className="list-disc space-y-1 pl-4 text-neutral-400 marker:text-neutral-600">
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
}

const TOOL_ICON: Record<string, typeof FileText> = {
  read_file: FileText,
  list_files: FolderTree,
  search_code: Search,
  web_search: Globe,
  fetch_url: Link,
};

function ToolLine({ activity: a, resolve }: { activity: ToolActivity; resolve: RoleResolver }) {
  const m = resolve(a.agent);
  const Icon = TOOL_ICON[a.tool] ?? Wrench;
  return (
    <div className="flex items-center gap-2 pl-9 text-[11px]" title={a.summary}>
      <Icon className={cx("size-3.5 shrink-0", a.ok ? "text-sky-400" : "text-amber-400")} />
      <span className="truncate text-neutral-500">
        <span style={{ color: m.color }}>{m.name}</span> {a.summary}
      </span>
    </div>
  );
}

function MemberLine({ change: c, resolve }: { change: MembershipChange; resolve: RoleResolver }) {
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
    <div className="flex items-start gap-2 rounded-md bg-neutral-900/40 px-2 py-1.5 text-[11px] text-neutral-400">
      <Icon className={cx("mt-px size-3.5 shrink-0", c.action === "invited" ? "text-emerald-400" : "text-neutral-500")} />
      <PixelPortrait meta={m} size={16} />
      <span className="min-w-0 break-words">
        <b style={{ color: m.color }}>{m.name}</b> {text}
        {c.reason && <span className="text-neutral-500"> — {c.reason}</span>}
      </span>
    </div>
  );
}

function RoundLine({ round }: { round: RoundView }) {
  const failed = Object.values(round.slots).filter((s) => s.error).length;
  return (
    <div className="flex items-center justify-center gap-1.5 text-[11px] text-neutral-500">
      <span className="h-px flex-1 bg-neutral-800" />
      Round {round.round}:
      <Badge tone={round.converged ? "success" : round.approvals > 0 ? "warning" : "danger"}>
        {round.approvals}/{round.total} APPROVE
      </Badge>
      {failed > 0 && <Badge tone="warning">{failed} lỗi</Badge>}
      <span className="h-px flex-1 bg-neutral-800" />
    </div>
  );
}

function Outcome({ scope }: { scope: ScopeState }) {
  const meta = STATUS_META[scope.status];
  const dept = scope.kind === "department";
  const text: Record<string, string> = {
    converged: dept
      ? scope.reviewerIds.length
        ? `Phòng ban đã thống nhất ở round ${scope.rounds.length}. Plan được gửi lên cuộc họp chính.`
        : "Plan của trưởng phòng được gửi lên cuộc họp chính."
      : `Tất cả reviewer đã APPROVE ở round ${scope.rounds.length}.`,
    partially_approved: "Các reviewer còn hoạt động đã APPROVE, nhưng có reviewer không review được.",
    max_rounds_reached: `Hết ${scope.maxRounds} vòng mà vẫn còn REJECT. Final có kèm mục Open Issues.`,
    cancelled: "Phiên đã bị dừng.",
    failed: scope.error ?? "Gặp lỗi.",
  };
  return (
    <div className={cx("flex items-start gap-2 rounded-md border border-neutral-800 bg-neutral-950/60 px-2.5 py-2 text-xs text-neutral-300")}>
      <Badge tone={meta.tone}>{meta.label}</Badge>
      <span className="break-words">{text[scope.status]}</span>
    </div>
  );
}
