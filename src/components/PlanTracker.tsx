import { memo, useMemo } from "react";
import { CircleCheck, CircleX, Clock, FileText, LoaderCircle, PenLine, Scissors, TriangleAlert } from "lucide-react";
import { STATUS_META, type RoleResolver } from "../lib/agents";
import { trackVersions, type VersionState, type VersionTrack, type Vote } from "../lib/planTrack";
import { useActive } from "../store/council";
import type { ScopeState } from "../store/session";
import { useUi } from "../store/ui";
import { Badge, Panel, cx } from "./ui";

const STATE: Record<VersionState, { label: string; icon: typeof FileText; cls: string }> = {
  writing: { label: "Đang viết", icon: PenLine, cls: "border-info-line bg-info-subtle text-info" },
  incomplete: { label: "Viết dở", icon: Scissors, cls: "border-warn-line bg-warn-subtle text-warn" },
  waiting: { label: "Chờ review", icon: Clock, cls: "border-line text-fg-muted" },
  reviewing: { label: "Đang review", icon: LoaderCircle, cls: "border-info-line bg-info-subtle text-info" },
  rejected: { label: "Bị REJECT", icon: CircleX, cls: "border-danger-line bg-danger-subtle text-danger" },
  approved: { label: "APPROVE", icon: CircleCheck, cls: "border-success-line bg-success-subtle text-success" },
  partial: { label: "Duyệt một phần", icon: TriangleAlert, cls: "border-warn-line bg-warn-subtle text-warn" },
};

const VOTE: Record<Vote, string> = {
  approve: "bg-success-solid",
  reject: "bg-danger-solid",
  abstain: "bg-fg-faint",
  error: "bg-warn-solid",
  pending: "bg-info-solid animate-pulse",
  none: "bg-surface-3",
};

/** Thanh theo dõi plan: mỗi phiên bản 1 thẻ (trạng thái, phiếu, số issue). Bấm thẻ → mở plan (popup). */
export const PlanTracker = memo(function PlanTracker({ scope, resolve }: { scope: ScopeState; resolve: RoleResolver }) {
  const focusPlan = useUi((s) => s.focusPlan);
  const sessionRunning = useActive((s) => s.status === "running");
  const running = sessionRunning && scope.status === "running";
  const tracks = useMemo(
    () => trackVersions(scope.plans, scope.rounds, scope.final, running),
    [scope.plans, scope.rounds, scope.final, running],
  );
  const meta = STATUS_META[scope.status];
  const latest = tracks.at(-1);

  return (
    <Panel className="shrink-0 p-3">
      <div className="mb-2 flex items-center gap-2 text-xs">
        <FileText className="size-3.5 text-fg-muted" aria-hidden />
        <span className="font-medium text-fg-soft">Tiến độ plan · {scope.name}</span>
        <Badge tone={meta.tone}>{meta.label}</Badge>
        {scope.currentRound > 0 && (
          <span className="text-fg-subtle">
            Round {scope.currentRound}/{scope.maxRounds}
          </span>
        )}
        {latest && (
          <button
            type="button"
            onClick={() => focusPlan(latest.version)}
            className="ml-auto rounded-md px-2 py-0.5 text-[11px] text-accent-fg hover:bg-surface-2"
          >
            Mở v{latest.version} mới nhất →
          </button>
        )}
      </div>
      {tracks.length === 0 ? (
        <p className="py-3 text-center text-xs text-fg-subtle">
          {running ? `${resolve(scope.authorId).name || "Người viết"} đang chuẩn bị bản nháp v1…` : "Chưa có plan nào — bắt đầu họp để chủ trì viết v1."}
        </p>
      ) : (
        <ol className="flex gap-2 overflow-x-auto pb-1" aria-label="Các phiên bản plan">
          {tracks.map((t) => (
            <li key={t.version} className="shrink-0">
              <VersionCard track={t} resolve={resolve} onOpen={() => focusPlan(t.version)} />
            </li>
          ))}
        </ol>
      )}
    </Panel>
  );
});

function VersionCard({ track: t, resolve, onOpen }: { track: VersionTrack; resolve: RoleResolver; onOpen: () => void }) {
  const st = STATE[t.state];
  const Icon = st.icon;
  const counted = t.votes.filter((v) => v.vote !== "none");
  const detail =
    t.state === "rejected"
      ? `${t.rejects} REJECT · ${t.issues} vấn đề`
      : t.round !== null
        ? `${t.approvals}/${counted.length} APPROVE`
        : t.state === "writing"
          ? "Chủ trì đang viết…"
          : "Chưa có vòng review";
  const tip = [
    `Plan v${t.version}: ${st.label}${t.round !== null ? ` (round ${t.round})` : ""}`,
    ...t.votes.map((v) => `• ${resolve(v.agent).name}: ${v.vote.toUpperCase()}`),
    ...t.topIssues.map((i) => `✗ ${i}`),
    "Bấm để xem plan",
  ].join("\n");
  return (
    <button
      type="button"
      onClick={onOpen}
      title={tip}
      aria-label={`Xem plan v${t.version} — ${st.label}`}
      className={cx(
        "group flex w-44 flex-col gap-1.5 rounded-lg border p-2.5 text-left transition-colors hover:border-accent/60 hover:bg-surface-2/60",
        t.isFinal ? "border-accent/60 ring-1 ring-accent/30" : "border-line",
      )}
    >
      <div className="flex items-center gap-1.5">
        <span className="px-font text-base font-bold text-fg">v{t.version}</span>
        {t.isFinal && <Badge tone="info">FINAL</Badge>}
        <span className={cx("ml-auto inline-flex items-center gap-1 rounded-full border px-1.5 py-0.5 text-[10px] font-medium", st.cls)}>
          <Icon className={cx("size-3", (t.state === "writing" || t.state === "reviewing") && "animate-spin [animation-duration:2s]")} aria-hidden />
          {st.label}
        </span>
      </div>
      {t.votes.length > 0 && (
        <div className="flex flex-wrap gap-1" aria-hidden>
          {t.votes.map((v) => (
            <span key={v.agent} className={cx("size-2 rounded-[2px]", VOTE[v.vote])} />
          ))}
        </div>
      )}
      <div className="flex items-center justify-between text-[11px] text-fg-subtle">
        <span className="truncate">{detail}</span>
        <span className="shrink-0 text-accent-fg opacity-0 transition-opacity group-hover:opacity-100">Xem →</span>
      </div>
    </button>
  );
}
