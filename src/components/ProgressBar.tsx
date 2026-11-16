import { memo, useEffect, useState } from "react";
import { Coins } from "lucide-react";
import { STATUS_META, formatTokens } from "../lib/agents";
import { MAIN_SCOPE, type UsageSummary } from "../lib/contracts";
import { ipc } from "../lib/ipc";
import { useActive } from "../store/council";
import { isTerminal, mainOf } from "../store/session";
import { toastError } from "../store/toast";
import { Badge, Panel, cx } from "./ui";

/** Chỉ báo lỗi get_usage 1 lần / phiên để không spam toast khi backend chưa có lệnh. */
const usageErrorShown = new Set<string>();

const fmtUsd = (v: number | null | undefined): string => (v == null ? "—" : v < 0.01 && v > 0 ? "<$0.01" : `$${v.toFixed(v < 1 ? 3 : 2)}`);

/**
 * Thanh tiến độ phía trên hội thoại: phase + round, ngân sách token (giới hạn mềm), chi phí.
 * Mọi selector đều hẹp (giá trị nguyên thuỷ / tham chiếu hiếm đổi) → không render lại theo plan_delta.
 */
export const ProgressBar = memo(function ProgressBar() {
  const id = useActive((s) => s.id);
  const status = useActive((s) => s.status);
  const round = useActive((s) => mainOf(s).currentRound);
  const maxRounds = useActive((s) => mainOf(s).maxRounds);
  const versions = useActive((s) => mainOf(s).plans.length);
  const departments = useActive((s) => s.scopeOrder.length - (s.scopes[MAIN_SCOPE] ? 1 : 0));
  const budget = useActive((s) => s.budget);
  const total = useActive((s) => s.usage.total);

  const [usage, setUsage] = useState<UsageSummary | null>(null);
  const terminal = isTerminal(status);

  // Lấy chi phí khi mở tab họp (mount) hoặc phiên kết thúc / đổi phiên. Lỗi không chặn hiển thị.
  useEffect(() => {
    setUsage(null);
    if (!id) return;
    let alive = true;
    ipc
      .getUsage(id)
      .then((u) => alive && setUsage(u))
      .catch((e) => {
        if (!alive || usageErrorShown.has(id)) return;
        usageErrorShown.add(id);
        toastError(e, "Không lấy được chi phí");
      });
    return () => {
      alive = false;
    };
  }, [id, terminal]);

  if (status === "idle" && !id) return null;

  const meta = STATUS_META[status];
  const tokens = total.prompt_tokens + total.completion_tokens;
  const used = budget?.used ?? tokens;
  const limit = budget && budget.limit > 0 ? budget.limit : null;
  const reserved = budget?.reserved ?? 0;
  const pct = (n: number) => (limit ? Math.min(100, (n / limit) * 100) : 0);
  const hot = budget?.state === "warn" || budget?.state === "exhausted";
  const stateText = budget?.state === "exhausted" ? "đã hết" : budget?.state === "warn" ? "sắp hết" : null;
  const cost = usage?.totals.cost_usd ?? null;
  const unpriced = usage?.unpriced_models ?? [];
  const costTitle =
    unpriced.length > 0 ? `Chưa có giá cho: ${unpriced.join(", ")}` : cost == null ? "Chưa có giá model — nhập ở Cài đặt → Giá model" : "Chi phí ước tính theo giá bạn nhập";

  return (
    <Panel className="shrink-0 space-y-1.5 px-3 py-2 text-xs">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Badge tone={meta.tone}>{meta.label}</Badge>
        <span className="text-fg-muted">
          Round {Math.max(round, 0)}/{maxRounds || "—"}
          {versions > 0 && <> · v{versions}</>}
          {departments > 0 && <> · {departments} phòng ban</>}
        </span>
        <span className="ml-auto inline-flex items-center gap-1 text-fg-muted" title={costTitle}>
          <Coins className="size-3.5" aria-hidden />
          Chi phí: <b className="font-semibold text-fg-soft">{fmtUsd(cost)}</b>
          {usage?.totals.estimated && cost != null && <span className="text-fg-subtle">(ước tính)</span>}
        </span>
      </div>
      <div className="flex items-center gap-2 text-fg-muted">
        <span>
          Token: {formatTokens(used)}
          {reserved > 0 && <> + {formatTokens(reserved)} giữ chỗ</>}
          {limit && <> / {formatTokens(limit)}</>}
        </span>
        {limit && (
          <>
            <div
              role="progressbar"
              aria-label="Giới hạn mềm · ước tính"
              aria-valuemin={0}
              aria-valuemax={limit}
              aria-valuenow={Math.min(used, limit)}
              aria-valuetext={`${formatTokens(used)} trên ${formatTokens(limit)} token${stateText ? `, ${stateText}` : ""}`}
              className="relative h-2 min-w-16 flex-1 overflow-hidden rounded-full bg-surface-3"
            >
              <div className={cx("absolute inset-y-0 left-0", hot ? "bg-warn-solid" : "bg-accent")} style={{ width: `${pct(used)}%` }} />
              <div
                className={cx("absolute inset-y-0 opacity-50", hot ? "bg-warn-solid" : "bg-accent")}
                style={{ left: `${pct(used)}%`, width: `${Math.min(pct(reserved), 100 - pct(used))}%` }}
              />
            </div>
            <span className={cx("shrink-0", hot ? "font-medium text-warn" : "text-fg-subtle")}>
              Giới hạn mềm · ước tính{stateText ? ` · ${stateText}` : ""}
            </span>
          </>
        )}
        {!limit && <span className="text-fg-subtle">không đặt giới hạn</span>}
      </div>
      {hot && (
        <p role="status" className="text-warn">
          {budget?.state === "exhausted" ? "Ngân sách token (giới hạn mềm) đã hết." : "Ngân sách token (giới hạn mềm) sắp hết."}
        </p>
      )}
    </Panel>
  );
});
