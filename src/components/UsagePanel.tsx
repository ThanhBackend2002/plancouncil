import { memo } from "react";
import { Coins, Info, Scissors } from "lucide-react";
import { USAGE_KIND_LABEL, formatTokens, type RoleResolver } from "../lib/agents";
import { MAIN_SCOPE, type ModelPrice, type TokenUsage } from "../lib/contracts";
import { useActiveShallow } from "../store/council";
import { useConfig } from "../store/config";
import { PixelPortrait } from "./PixelCharacter";
import { cx } from "./ui";
import { fmtUsd } from "./ProgressBar";

const total = (u: TokenUsage) => u.prompt_tokens + u.completion_tokens;

/** Chi phí theo bảng giá HIỆN TẠI (giống backend: `prompt` đã gồm phần `cached`). */
function costOf(u: TokenUsage, price: ModelPrice | undefined): number | null {
  if (!price) return null;
  const cached = Math.min(u.cached_tokens ?? 0, u.prompt_tokens);
  return ((u.prompt_tokens - cached) * price.input_per_m + cached * price.cached_per_m + u.completion_tokens * price.output_per_m) / 1_000_000;
}

/** Bảng token tiêu thụ của phiên: tổng, theo loại việc, theo role, theo nơi họp. */
export const UsagePanel = memo(function UsagePanel({ resolve }: { resolve: RoleResolver }) {
  // `usage` chỉ đổi khi có usage_updated / tokens_saved; `scopes` giữ tham chiếu khi plan_delta.
  const { usage: u, scopes: allScopes } = useActiveShallow((s) => ({ usage: s.usage, scopes: s.scopes }));
  const pricing = useConfig((s) => s.config?.pricing);
  const sum = total(u.total);
  if (u.total.calls === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center text-sm text-fg-subtle">
        <Coins className="size-8 text-fg-faint" />
        Chưa có lần gọi model nào trong phiên này.
      </div>
    );
  }
  const roles = Object.entries(u.by_role).sort((a, b) => total(b[1]) - total(a[1]));
  const kinds = Object.entries(u.by_kind).sort((a, b) => total(b[1]) - total(a[1]));
  const models = Object.entries(u.by_model ?? {}).sort((a, b) => total(b[1]) - total(a[1]));
  const scopes = Object.entries(u.by_scope).sort((a, b) => total(b[1]) - total(a[1]));
  const scopeName = (id: string) => (id === MAIN_SCOPE ? "Họp chính" : (allScopes[id]?.name ?? id));

  return (
    <div className="mx-auto max-w-[860px] space-y-5 px-8 py-6">
      <div className="grid grid-cols-4 gap-3">
        <Stat label="Tổng token" value={formatTokens(sum)} strong />
        <Stat label="Input (prompt)" value={formatTokens(u.total.prompt_tokens)} />
        <Stat label="Output (completion)" value={formatTokens(u.total.completion_tokens)} />
        <Stat label="Số lần gọi" value={String(u.total.calls)} />
      </div>
      {(u.saved_tokens ?? 0) > 0 && (
        <p className="flex items-start gap-2 rounded-control border border-success-line bg-success-subtle px-3 py-2 text-xs text-success">
          <Scissors className="mt-px size-3.5 shrink-0" />
          <span>
            Tiết kiệm ~<b>{formatTokens(u.saved_tokens ?? 0)}</b> token nhờ review theo section
            {(u.carried_reviews ?? 0) > 0 && <> · {u.carried_reviews} lượt review giữ nguyên APPROVE, không gọi model</>}.
          </span>
        </p>
      )}
      {u.total.estimated_calls > 0 && (
        <p className="flex items-start gap-2 rounded-control border border-warn-line bg-warn-subtle px-3 py-2 text-xs text-warn">
          <Info className="mt-px size-3.5 shrink-0" />
          {u.total.estimated_calls}/{u.total.calls} lần gọi provider không trả số token, nên được ước lượng theo độ dài văn bản (~3 ký tự/token).
        </p>
      )}

      {models.length > 0 && (
        <Section title={`Theo model (${models.length})`}>
          {models.map(([name, v]) => {
            const cost = costOf(v, pricing?.[name]);
            return (
              <BarRow
                key={name}
                label={name}
                usage={v}
                max={sum}
                color="var(--success-solid)"
                extra={
                  <span title={cost == null ? "Chưa có giá model — nhập ở Cài đặt → Giá model" : "Chi phí ước tính theo bảng giá hiện tại"}>
                    {fmtUsd(cost)}
                  </span>
                }
              />
            );
          })}
        </Section>
      )}

      <Section title="Theo loại việc">
        {kinds.map(([k, v]) => (
          <BarRow key={k} label={USAGE_KIND_LABEL[k] ?? k} usage={v} max={sum} color="var(--accent)" />
        ))}
      </Section>

      <Section title={`Theo role (${roles.length})`}>
        {roles.map(([id, v]) => {
          const m = resolve(id);
          return <BarRow key={id} label={m.name} usage={v} max={sum} color={m.color} icon={<PixelPortrait meta={m} size={22} />} />;
        })}
      </Section>

      {scopes.length > 1 && (
        <Section title="Theo nơi họp">
          {scopes.map(([id, v]) => (
            <BarRow key={id} label={scopeName(id)} usage={v} max={sum} color="var(--info-solid)" />
          ))}
        </Section>
      )}
      <p className="text-[11px] text-fg-subtle">
        Mẹo giảm token: giảm số reviewer/phòng ban, dùng model rẻ hơn cho Staff (Gán model hàng loạt), đặt "Ai được dùng công cụ" = chỉ người viết plan,
        giảm số vòng họp.
      </p>
    </div>
  );
});

function Stat({ label, value, strong }: { label: string; value: string; strong?: boolean }) {
  return (
    <div className="rounded-lg border border-line bg-surface/50 px-3 py-2">
      <div className="text-[11px] text-fg-subtle">{label}</div>
      <div className={strong ? "text-xl font-semibold text-accent-fg tabular-nums" : "text-lg font-medium text-fg tabular-nums"}>
        {value}
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section>
      <h3 className="mb-2 text-xs font-semibold tracking-wide text-fg-muted uppercase">{title}</h3>
      <div className="space-y-1.5">{children}</div>
    </section>
  );
}

function BarRow({
  label,
  usage,
  max,
  color,
  icon,
  extra,
}: {
  label: string;
  usage: TokenUsage;
  max: number;
  color: string;
  icon?: React.ReactNode;
  /** Cột phụ bên phải (vd. chi phí theo model). */
  extra?: React.ReactNode;
}) {
  const t = total(usage);
  const pct = max > 0 ? (t / max) * 100 : 0;
  return (
    <div
      className={cx(
        "grid items-center gap-3 text-xs",
        extra === undefined ? "grid-cols-[minmax(140px,1.2fr)_2fr_auto]" : "grid-cols-[minmax(140px,1.2fr)_2fr_auto_auto]",
      )}
    >
      <span className="flex min-w-0 items-center gap-2">
        {icon}
        <span className="truncate text-fg-soft" title={label}>
          {label}
        </span>
      </span>
      <span className="h-2 overflow-hidden rounded-full bg-surface-2" title={`${pct.toFixed(1)}%`}>
        <span className="block h-full rounded-full" style={{ width: `${Math.max(pct, 1)}%`, background: color }} />
      </span>
      <span className="w-48 text-right text-fg-muted tabular-nums">
        <b className="text-fg-soft">{formatTokens(t)}</b> · in {formatTokens(usage.prompt_tokens)} / out {formatTokens(usage.completion_tokens)} ·{" "}
        {usage.calls}×{usage.estimated_calls > 0 && <span className="text-warn">*</span>}
      </span>
      {extra !== undefined && <span className="w-16 text-right font-medium text-fg-soft tabular-nums">{extra}</span>}
    </div>
  );
}
