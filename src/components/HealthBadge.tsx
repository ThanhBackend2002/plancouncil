import { LoaderCircle } from "lucide-react";
import type { ModelHealth } from "../lib/contracts";
import { formatDate } from "../lib/agents";
import { HEALTH_META } from "../store/health";
import { Badge } from "./ui";

/** Badge trạng thái model của role (tooltip: chi tiết lỗi + số dư nếu có). */
export function HealthBadge({ health, checking }: { health: ModelHealth | undefined; checking?: boolean }) {
  if (checking) {
    return (
      <Badge tone="info">
        <LoaderCircle className="size-3 animate-spin" /> đang kiểm tra
      </Badge>
    );
  }
  if (!health) return null;
  const m = HEALTH_META[health.status];
  const title = [
    `${m.label}: ${m.hint}`,
    health.message,
    health.balance ? `Số dư: ${health.balance}` : "",
    `${health.provider_name} · ${health.model} · kiểm tra lúc ${formatDate(health.checked_at)}`,
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <span title={title}>
      <Badge tone={m.tone}>
        {m.label}
        {health.status === "ok" && health.latency_ms != null && <span className="opacity-70">· {health.latency_ms} ms</span>}
      </Badge>
    </span>
  );
}
