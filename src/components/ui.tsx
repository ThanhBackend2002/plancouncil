// Primitive UI dùng chung.

import type { ButtonHTMLAttributes, ReactNode } from "react";
import { LoaderCircle } from "lucide-react";
import { STATUS_META, TONE_CLASS, type Tone } from "../lib/agents";
import type { Phase } from "../store/session";

export function cx(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

type Variant = "primary" | "danger" | "ghost" | "outline";

const VARIANT: Record<Variant, string> = {
  primary: "bg-violet-500 text-white hover:bg-violet-400 disabled:bg-violet-500/40",
  danger: "bg-red-500/90 text-white hover:bg-red-500 disabled:bg-red-500/40",
  ghost: "text-neutral-300 hover:bg-neutral-800 hover:text-neutral-100",
  outline: "border border-neutral-700 text-neutral-200 hover:bg-neutral-800 hover:border-neutral-600",
};

export function Button({
  variant = "outline",
  size = "md",
  loading,
  icon,
  className,
  children,
  disabled,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: "sm" | "md";
  loading?: boolean;
  icon?: ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={disabled || loading}
      className={cx(
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-md font-medium transition-colors select-none",
        "disabled:cursor-not-allowed disabled:opacity-60",
        size === "sm" ? "h-7 px-2.5 text-xs" : "h-9 px-3.5 text-sm",
        VARIANT[variant],
        className,
      )}
      {...rest}
    >
      {loading ? <LoaderCircle className="size-4 animate-spin" /> : icon}
      {children}
    </button>
  );
}

export function Badge({ tone = "neutral", children, className }: { tone?: Tone; children: ReactNode; className?: string }) {
  return (
    <span
      className={cx(
        "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium ring-1 ring-inset whitespace-nowrap",
        TONE_CLASS[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

export function StatusBadge({ status }: { status: Phase }) {
  const m = STATUS_META[status];
  return (
    <Badge tone={m.tone}>
      {status === "running" && <span className="size-1.5 animate-pulse rounded-full bg-sky-400" />}
      {m.label}
    </Badge>
  );
}

export function VerdictBadge({ verdict, carried }: { verdict: "APPROVE" | "REJECT" | "ABSTAIN"; carried?: boolean }) {
  if (verdict === "ABSTAIN") return <Badge tone="neutral">RÚT</Badge>;
  return (
    <Badge tone={verdict === "APPROVE" ? "success" : "danger"}>
      {verdict}
      {carried && <span className="opacity-70">· giữ nguyên</span>}
    </Badge>
  );
}

export function Panel({ className, children }: { className?: string; children: ReactNode }) {
  return <section className={cx("rounded-xl border border-neutral-800 bg-neutral-900/40", className)}>{children}</section>;
}
