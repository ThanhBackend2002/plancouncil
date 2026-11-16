import { Clock, Coins, Settings, Users } from "lucide-react";
import { formatTokens } from "../lib/agents";
import { MAIN_SCOPE } from "../lib/contracts";
import { useShallow } from "zustand/react/shallow";
import { useCouncil } from "../store/council";
import { mainOf } from "../store/session";
import { useUi, type Tab } from "../store/ui";
import { StatusBadge, cx } from "./ui";

const TABS: { id: Tab; label: string; icon: typeof Users }[] = [
  { id: "council", label: "Phòng họp", icon: Users },
  { id: "history", label: "Lịch sử", icon: Clock },
  { id: "settings", label: "Cài đặt", icon: Settings },
];

export function Header() {
  const { tab, setTab } = useUi(useShallow((s) => ({ tab: s.tab, setTab: s.setTab })));
  // Chỉ lấy giá trị nguyên thuỷ + `usage.total` → không re-render theo từng plan_delta.
  const { status, round, maxRounds, hasPlan, deptCount, deptsDone, total } = useCouncil(
    useShallow((s) => {
      const live = s.live;
      const main = mainOf(live);
      const depts = live.scopeOrder.filter((id) => id !== MAIN_SCOPE).map((id) => live.scopes[id]!);
      return {
        status: live.status,
        round: main.currentRound,
        maxRounds: main.maxRounds,
        hasPlan: main.plans.length > 0,
        deptCount: depts.length,
        deptsDone: depts.filter((d) => d.status !== "running").length,
        total: live.usage.total,
      };
    }),
  );
  const running = status === "running";

  return (
    <header className="flex h-12 shrink-0 items-center gap-6 border-b border-line bg-canvas px-4">
      <div className="flex items-center gap-2">
        <div className="grid size-6 place-items-center rounded-control bg-gradient-to-br from-violet-500 to-fuchsia-500 text-[11px] font-bold text-white">
          PC
        </div>
        <span className="text-sm font-semibold tracking-tight">PlanCouncil</span>
      </div>

      <nav className="flex items-center gap-1" aria-label="Điều hướng">
        {TABS.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            type="button"
            onClick={() => setTab(id)}
            aria-current={tab === id ? "page" : undefined}
            className={cx(
              "inline-flex h-8 items-center gap-1.5 rounded-control px-3 text-sm transition-colors",
              tab === id ? "bg-surface-2 text-fg" : "text-fg-muted hover:bg-surface hover:text-fg-soft",
            )}
          >
            <Icon className="size-4" />
            {label}
          </button>
        ))}
      </nav>

      <div className="ml-auto flex items-center gap-3 text-xs text-fg-muted">
        {running && (
          <span>
            {round > 0
              ? `Họp chính · Round ${round}/${maxRounds || "?"}`
              : hasPlan
                ? "Chủ trì đang viết plan tổng…"
                : deptCount > 0
                  ? `Phòng ban đang họp (${deptsDone}/${deptCount} xong)`
                  : "Đang khởi động…"}
          </span>
        )}
        {total.calls > 0 && (
          <span
            className="inline-flex items-center gap-1 rounded-full bg-surface px-2 py-0.5 tabular-nums ring-1 ring-line"
            title={`Input ${total.prompt_tokens.toLocaleString("vi-VN")} · Output ${total.completion_tokens.toLocaleString("vi-VN")} · ${total.calls} lần gọi`}
          >
            <Coins className="size-3.5 text-warn" />
            {formatTokens(total.prompt_tokens + total.completion_tokens)} token
          </span>
        )}
        <StatusBadge status={status} />
      </div>
    </header>
  );
}
