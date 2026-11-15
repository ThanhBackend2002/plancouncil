import { Clock, Coins, Settings, Users } from "lucide-react";
import { formatTokens } from "../lib/agents";
import { MAIN_SCOPE } from "../lib/contracts";
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
  const { tab, setTab } = useUi();
  const live = useCouncil((s) => s.live);
  const running = live.status === "running";
  const main = mainOf(live);
  const depts = live.scopeOrder.filter((id) => id !== MAIN_SCOPE).map((id) => live.scopes[id]!);
  const deptsDone = depts.filter((d) => d.status !== "running").length;

  return (
    <header className="flex h-12 shrink-0 items-center gap-6 border-b border-neutral-800 bg-neutral-950 px-4">
      <div className="flex items-center gap-2">
        <div className="grid size-6 place-items-center rounded-md bg-gradient-to-br from-violet-500 to-fuchsia-500 text-[11px] font-bold">
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
              "inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-sm transition-colors",
              tab === id ? "bg-neutral-800 text-neutral-50" : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200",
            )}
          >
            <Icon className="size-4" />
            {label}
          </button>
        ))}
      </nav>

      <div className="ml-auto flex items-center gap-3 text-xs text-neutral-400">
        {running && (
          <span>
            {main.currentRound > 0
              ? `Họp chính · Round ${main.currentRound}/${main.maxRounds || "?"}`
              : main.plans.length > 0
                ? "Chủ trì đang viết plan tổng…"
                : depts.length > 0
                  ? `Phòng ban đang họp (${deptsDone}/${depts.length} xong)`
                  : "Đang khởi động…"}
          </span>
        )}
        {live.usage.total.calls > 0 && (
          <span
            className="inline-flex items-center gap-1 rounded-full bg-neutral-900 px-2 py-0.5 tabular-nums ring-1 ring-neutral-800"
            title={`Input ${live.usage.total.prompt_tokens.toLocaleString("vi-VN")} · Output ${live.usage.total.completion_tokens.toLocaleString("vi-VN")} · ${live.usage.total.calls} lần gọi`}
          >
            <Coins className="size-3.5 text-amber-300" />
            {formatTokens(live.usage.total.prompt_tokens + live.usage.total.completion_tokens)} token
          </span>
        )}
        <StatusBadge status={live.status} />
      </div>
    </header>
  );
}
