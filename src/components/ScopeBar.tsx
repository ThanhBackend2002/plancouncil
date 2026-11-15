import { CircleCheck, CircleX, LoaderCircle, Network, Users } from "lucide-react";
import { MAIN_SCOPE } from "../lib/contracts";
import type { SessionState } from "../store/session";
import { useCouncil } from "../store/council";
import { cx } from "./ui";

/** Chọn xem cuộc họp chính hay phòng ban nào. Chỉ hiện khi phiên có phòng ban. */
export function ScopeBar({ session, active }: { session: SessionState; active: string }) {
  const { scope: sel, setScope } = useCouncil();
  if (session.scopeOrder.length <= 1) return null;

  return (
    <div className="flex shrink-0 items-center gap-1 overflow-x-auto rounded-xl border border-neutral-800 bg-neutral-900/40 p-1">
      {session.scopeOrder.map((id) => {
        const sc = session.scopes[id];
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
              active === id ? "bg-neutral-800 text-neutral-50" : "text-neutral-400 hover:bg-neutral-800/60 hover:text-neutral-200",
            )}
          >
            <Icon className="size-3.5" />
            <span className="max-w-[140px] truncate">{isMain ? "Họp chính" : sc.name}</span>
            {thinking ? (
              <LoaderCircle className="size-3 animate-spin text-sky-400" />
            ) : StatusIcon ? (
              <StatusIcon className={cx("size-3", sc.status === "failed" ? "text-red-400" : "text-emerald-400")} />
            ) : sc.currentRound > 0 ? (
              <span className="text-[10px] text-neutral-500">
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
          sel === "auto" ? "text-sky-300" : "text-neutral-500 hover:text-neutral-300",
        )}
        title="Tự chuyển sang nơi đang họp"
      >
        Tự theo dõi
      </button>
    </div>
  );
}
