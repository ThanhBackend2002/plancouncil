import { CircleCheck, CircleX, LoaderCircle, Network, Users } from "lucide-react";
import { MAIN_SCOPE } from "../lib/contracts";
import { memo } from "react";
import { useShallow } from "zustand/react/shallow";
import { activeOf, useCouncil } from "../store/council";
import { cx } from "./ui";

/** Chọn xem cuộc họp chính hay phòng ban nào. Chỉ hiện khi phiên có phòng ban. */
export const ScopeBar = memo(function ScopeBar({ active }: { active: string }) {
  // `scopes` giữ nguyên tham chiếu khi plan_delta (text stream nằm ở `drafts`) → không re-render theo delta.
  const { sel, setScope, scopeOrder, scopes } = useCouncil(
    useShallow((s) => {
      const a = activeOf(s);
      return { sel: s.scope, setScope: s.setScope, scopeOrder: a.scopeOrder, scopes: a.scopes };
    }),
  );
  if (scopeOrder.length <= 1) return null;

  return (
    <div className="flex shrink-0 items-center gap-1 overflow-x-auto rounded-panel border border-line bg-panel p-1">
      {scopeOrder.map((id) => {
        const sc = scopes[id];
        if (!sc) return null;
        const isMain = id === MAIN_SCOPE;
        const thinking = Object.values(sc.agents).some((a) => a.status === "thinking");
        const Icon = isMain ? Users : Network;
        const StatusIcon =
          sc.status === "converged" || sc.status === "partially_approved"
            ? CircleCheck
            : sc.status === "failed"
              ? CircleX
              : null;
        return (
          <button
            key={id}
            type="button"
            onClick={() => setScope(id)}
            aria-pressed={active === id}
            title={sc.task ?? sc.name}
            className={cx(
              "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs transition-colors",
              active === id ? "bg-surface-2 text-fg" : "text-fg-muted hover:bg-surface-2/60 hover:text-fg-soft",
            )}
          >
            <Icon className="size-3.5" />
            <span className="max-w-[140px] truncate">{isMain ? "Họp chính" : sc.name}</span>
            {thinking ? (
              <LoaderCircle className="size-3 animate-spin text-info" />
            ) : StatusIcon ? (
              <StatusIcon className={cx("size-3", sc.status === "failed" ? "text-danger" : "text-success")} />
            ) : sc.currentRound > 0 ? (
              <span className="text-[10px] text-fg-subtle">
                R{sc.currentRound}/{sc.maxRounds}
              </span>
            ) : null}
          </button>
        );
      })}
      <button
        type="button"
        onClick={() => setScope("auto")}
        className={cx(
          "ml-auto shrink-0 rounded-lg px-2 py-1 text-[11px]",
          sel === "auto" ? "text-info" : "text-fg-subtle hover:text-fg-soft",
        )}
        title="Tự chuyển sang nơi đang họp"
      >
        Tự theo dõi
      </button>
    </div>
  );
});
