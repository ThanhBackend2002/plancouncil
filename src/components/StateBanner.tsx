import { memo, useEffect, useMemo, useState, type ReactNode } from "react";
import { CircleAlert, KeyRound, OctagonAlert, Play, RotateCcw, Scissors, TriangleAlert, Wallet } from "lucide-react";
import { MAIN_SCOPE } from "../lib/contracts";
import { ipc, toCommandError } from "../lib/ipc";
import { activeOf, useActiveShallow, useCouncil } from "../store/council";
import { useConfig, useResolver } from "../store/config";
import { mainOf } from "../store/session";
import { toastError } from "../store/toast";
import { useUi } from "../store/ui";
import { RerunDialog } from "./RerunDialog";
import { Button, cx } from "./ui";

type Tone = "warn" | "danger";

function Banner({
  tone,
  icon,
  title,
  alert,
  children,
  action,
}: {
  tone: Tone;
  icon: ReactNode;
  title: string;
  alert?: boolean;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div
      role={alert ? "alert" : "status"}
      className={cx(
        "flex shrink-0 items-start gap-2.5 rounded-panel border px-3 py-2.5 text-xs",
        tone === "warn" ? "border-warn-line bg-warn-subtle text-warn" : "border-danger-line bg-danger-subtle text-danger",
      )}
    >
      <span className="mt-0.5 shrink-0" aria-hidden>
        {icon}
      </span>
      <div className="min-w-0 flex-1 space-y-1">
        <p className="font-semibold">{title}</p>
        {children && <div className="text-fg-soft">{children}</div>}
      </div>
      {action && <div className="shrink-0 self-center">{action}</div>}
    </div>
  );
}

/** Dò xem có version hợp lệ để chạy tiếp không (dry-run). Lỗi → null, banner dùng số liệu cục bộ. */
function useAvailableVersions(sessionId: string | null, localVersion: number, enabled: boolean): number[] | null {
  const [versions, setVersions] = useState<number[] | null>(null);
  useEffect(() => {
    setVersions(null);
    if (!enabled || !sessionId) return;
    let alive = true;
    ipc
      .rerunFromVersion({ session_id: sessionId, scope: MAIN_SCOPE, version_no: localVersion, dry_run: true })
      .then((r) => alive && setVersions(r.available_versions))
      .catch((e) => {
        // no_valid_version = chắc chắn không có version; lỗi khác (vd. backend chưa có lệnh) → dùng số liệu cục bộ.
        if (alive && toCommandError(e).kind === "no_valid_version") setVersions([]);
      });
    return () => {
      alive = false;
    };
  }, [sessionId, localVersion, enabled]);
  return versions;
}

const RERUN_STATUSES = new Set(["truncated", "budget_exhausted", "interrupted", "unresolved"]);

/** Banner trạng thái ở đầu màn họp: 1 thông điệp + 1 hành động chính cho mỗi trạng thái. */
export const StateBanner = memo(function StateBanner() {
  const s = useActiveShallow((st) => {
    const main = mainOf(st);
    return {
      id: st.id,
      status: st.status,
      lastTruncated: st.lastTruncated,
      consistency: st.consistency,
      fallbackStreak: st.fallbackStreak ?? 0,
      plans: main.plans,
      reviewerIds: main.reviewerIds,
      maxRounds: main.maxRounds,
      cast: st.cast,
    };
  });
  const isLive = useCouncil((c) => c.viewing === null);
  const startError = useCouncil((c) => (c.viewing === null ? c.startError : null));
  const openSettings = useUi((u) => u.openSettings);
  const config = useConfig((c) => c.config);
  const resolve = useResolver(s.cast);
  const [dialog, setDialog] = useState<null | { focusBudget: boolean }>(null);
  const [restarting, setRestarting] = useState(false);

  const localVersion = useMemo(() => s.plans.filter((p) => p.complete).at(-1)?.version ?? 0, [s.plans]);
  const rerunnable = RERUN_STATUSES.has(s.status) && !!s.id;
  const available = useAvailableVersions(s.id, localVersion, rerunnable);
  const noVersion = available ? available.length === 0 : localVersion === 0;
  const version = available && available.length > 0 ? Math.max(...available) : localVersion;
  // Chỉ báo cáo khi đã chạy xong (lastTruncated của phiên đang chạy không đổi trạng thái).
  const truncReason = s.lastTruncated?.reason;

  const restart = async () => {
    if (!s.id) return;
    setRestarting(true);
    try {
      const r = await ipc.restartFromRequest(s.id);
      useCouncil.getState().adoptSession(r.new_session_id, activeOf(useCouncil.getState()).prompt);
      useUi.getState().setTab("council");
    } catch (e) {
      toastError(e, "Không bắt đầu lại được");
    } finally {
      setRestarting(false);
    }
  };

  /** Nút chính cho các trạng thái "chạy tiếp"; không có version hợp lệ → "Bắt đầu lại từ yêu cầu". */
  const rerunAction = (label: string, focusBudget = false) =>
    noVersion ? (
      <Button size="sm" variant="primary" loading={restarting} icon={<RotateCcw className="size-3.5" />} onClick={() => void restart()}>
        Bắt đầu lại từ yêu cầu
      </Button>
    ) : (
      <Button size="sm" variant="primary" icon={<Play className="size-3.5" />} onClick={() => setDialog({ focusBudget })}>
        {label}
      </Button>
    );
  const noVersionNote = noVersion && (
    <p>Chưa có version plan hợp lệ nào để chạy tiếp, nên chỉ có thể bắt đầu lại từ yêu cầu (phiên mới).</p>
  );

  const items: ReactNode[] = [];

  if (s.status === "truncated" && truncReason !== "budget") {
    items.push(
      <Banner
        key="trunc"
        tone="warn"
        icon={<Scissors className="size-4" />}
        title="Output bị cắt"
        action={
          <Button size="sm" variant="primary" onClick={() => openSettings("providers")}>
            Tăng max_tokens
          </Button>
        }
      >
        Model dừng giữa chừng ({truncReason === "max_tokens_cap" ? "chạm trần max_tokens" : "hết độ dài cho phép"}); phiên giữ phiên bản hợp lệ gần nhất.
      </Banner>,
    );
  }

  if ((s.status === "truncated" && truncReason === "budget") || s.status === "budget_exhausted") {
    items.push(
      <Banner
        key="budget"
        tone="warn"
        icon={<Wallet className="size-4" />}
        title="Hết ngân sách (giới hạn mềm)"
        action={rerunAction("Tăng ngân sách và tiếp tục", true)}
      >
        Phiên dừng vì chạm ngân sách token (ước tính). {noVersionNote}
      </Banner>,
    );
  }

  if (s.status === "interrupted") {
    items.push(
      <Banner
        key="int"
        tone="warn"
        icon={<OctagonAlert className="size-4" />}
        title="Output đang stream khi app đóng không được lưu"
        action={rerunAction(`Chạy tiếp từ version ${version}`)}
      >
        {noVersionNote || "Các version đã hoàn tất vẫn còn nguyên."}
      </Banner>,
    );
  }

  if (s.status === "unresolved") {
    const groups = new Map<string, string[]>();
    for (const c of s.consistency?.conflicts ?? []) {
      for (const sec of c.sections.length ? c.sections : ["(chung)"]) groups.set(sec, [...(groups.get(sec) ?? []), c.text]);
    }
    items.push(
      <Banner
        key="unres"
        tone="warn"
        icon={<TriangleAlert className="size-4" />}
        title="Còn mâu thuẫn giữa các section"
        action={rerunAction("Chạy tiếp từ version cuối")}
      >
        {groups.size > 0 ? (
          <ul className="space-y-1">
            {[...groups].map(([sec, texts]) => (
              <li key={sec}>
                <b className="text-fg">Section {sec}</b>
                <ul className="list-disc pl-4">
                  {texts.map((t, i) => (
                    <li key={i}>{t}</li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        ) : (
          <span>Kiểm tra nhất quán chưa đạt; xem phụ lục trong Final.</span>
        )}
        {noVersionNote}
      </Banner>,
    );
  }

  if (s.fallbackStreak >= 2 && config?.meeting.revise_mode !== "full") {
    items.push(
      <Banner
        key="fallback"
        tone="warn"
        icon={<CircleAlert className="size-4" />}
        title="Revise theo section liên tục bị loại"
        action={
          <Button size="sm" onClick={() => openSettings("meeting")}>
            Mở Cài đặt → Cuộc họp
          </Button>
        }
      >
        Đã {s.fallbackStreak} lần liên tiếp phải viết lại toàn bộ plan. Gợi ý: đặt <code>revise_mode=full</code> để bỏ qua bước thử sửa theo section.
      </Banner>,
    );
  }

  if (isLive && startError?.kind === "secret_missing") {
    items.push(
      <Banner
        key="secret"
        tone="danger"
        alert
        icon={<KeyRound className="size-4" />}
        title="Thiếu API key (chế độ chỉ RAM)"
        action={
          <Button size="sm" variant="primary" onClick={() => openSettings("providers")}>
            Mở Cài đặt → Provider
          </Button>
        }
      >
        {startError.message || "Key chỉ lưu trong bộ nhớ và đã mất sau khi mở lại app — nhập lại key để bắt đầu họp."}
      </Banner>,
    );
  }

  return (
    <>
      {items.length > 0 && <div className="flex shrink-0 flex-col gap-2">{items}</div>}
      {dialog && s.id && (
        <RerunDialog
          open
          onClose={() => setDialog(null)}
          sessionId={s.id}
          scope={MAIN_SCOPE}
          version={version}
          reviewerIds={s.reviewerIds}
          defaultMaxRounds={s.maxRounds}
          focusBudget={dialog.focusBudget}
          resolveName={(id) => resolve(id).name || id}
        />
      )}
    </>
  );
});
