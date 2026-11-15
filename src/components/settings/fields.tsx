// Primitive form dùng chung cho các tab Settings.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ChevronDown, Eye, EyeOff, Plus, Undo2, X } from "lucide-react";
import { LEVELS, makeResolver } from "../../lib/agents";
import { MASK_PREFIX, type RoleDef, type RoleId } from "../../lib/contracts";
import { PixelPortrait } from "../PixelCharacter";
import { Button, cx } from "../ui";

export type Errors = Partial<Record<string, string>>;

export const INPUT =
  "h-9 w-full rounded-md border border-neutral-700 bg-neutral-950 px-2.5 text-sm text-neutral-100 placeholder:text-neutral-600 focus:border-violet-500/60 focus:outline-none";

export function SectionTitle({ title, desc, children }: { title: string; desc?: string; children?: ReactNode }) {
  return (
    <div className="mb-4 flex items-start gap-3">
      <div className="min-w-0 flex-1">
        <h2 className="text-sm font-semibold text-neutral-100">{title}</h2>
        {desc && <p className="mt-0.5 text-xs text-neutral-500">{desc}</p>}
      </div>
      {children}
    </div>
  );
}

export function Field({
  label,
  error,
  hint,
  className,
  children,
}: {
  label: string;
  error?: string;
  hint?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cx("block min-w-0", className)}>
      <span className="mb-1 block text-xs font-medium text-neutral-400">{label}</span>
      {children}
      {error ? (
        <span className="mt-1 block text-[11px] text-red-400">{error}</span>
      ) : hint ? (
        <span className="mt-1 block text-[11px] text-neutral-600">{hint}</span>
      ) : null}
    </div>
  );
}

export function TextInput({
  value,
  onChange,
  onBlur,
  placeholder,
  mono,
}: {
  value: string;
  onChange: (v: string) => void;
  onBlur?: () => void;
  placeholder?: string;
  mono?: boolean;
}) {
  return (
    <input
      value={value}
      onChange={(e) => onChange(e.target.value)}
      onBlur={onBlur}
      placeholder={placeholder}
      spellCheck={false}
      autoComplete="off"
      className={cx(INPUT, mono && "font-mono text-[13px]")}
    />
  );
}

export function TextArea({
  value,
  onChange,
  placeholder,
  rows = 3,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  rows?: number;
}) {
  return (
    <textarea
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      rows={rows}
      className="w-full resize-y rounded-md border border-neutral-700 bg-neutral-950 px-2.5 py-2 text-sm text-neutral-100 placeholder:text-neutral-600 focus:border-violet-500/60 focus:outline-none"
    />
  );
}

/** Giữ text thô cục bộ để gõ "0." hay xoá trắng không bị ép giá trị. */
export function NumberInput({
  value,
  onChange,
  nullable,
  step = 1,
}: {
  value: number | null;
  onChange: (v: number | null) => void;
  nullable?: boolean;
  step?: number;
}) {
  const fmt = (v: number | null) => (v == null || Number.isNaN(v) ? "" : String(v));
  const [text, setText] = useState(fmt(value));
  const last = useRef(value);
  useEffect(() => {
    if (!Object.is(value, last.current)) {
      last.current = value;
      setText(fmt(value));
    }
  }, [value]);

  return (
    <input
      type="number"
      inputMode="decimal"
      step={step}
      value={text}
      onChange={(e) => {
        const t = e.target.value;
        setText(t);
        const v = t.trim() === "" ? (nullable ? null : Number.NaN) : Number(t);
        last.current = v;
        onChange(v);
      }}
      className={cx(INPUT, "tabular-nums")}
    />
  );
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <label className="flex h-9 cursor-pointer items-center gap-2 text-sm text-neutral-300">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} className="size-4 accent-violet-500" />
      {label}
    </label>
  );
}

/**
 * Ô API key. Backend trả key đã mask (`••••abcd`):
 * - giữ nguyên mask = giữ key cũ;
 * - "Đổi key" = nhập key mới; để trống khi lưu = xoá key.
 */
export function KeyInput({
  value,
  saved,
  onChange,
  onCommit,
  placeholder,
}: {
  value: string | null;
  saved: string | null;
  onChange: (v: string | null) => void;
  onCommit?: () => void;
  placeholder?: string;
}) {
  const [reveal, setReveal] = useState(false);
  const ref = useRef<HTMLInputElement>(null);
  const masked = !!value && value.startsWith(MASK_PREFIX);
  const hasSaved = !!saved && saved.startsWith(MASK_PREFIX);

  if (masked) {
    return (
      <div className="flex gap-2">
        <input readOnly value={value ?? ""} className={cx(INPUT, "font-mono text-neutral-400")} aria-label="API key đã lưu" />
        <Button
          onClick={() => {
            onChange("");
            setTimeout(() => ref.current?.focus(), 0);
          }}
        >
          Đổi key
        </Button>
      </div>
    );
  }

  return (
    <div>
      <div className="flex gap-2">
        <div className="relative flex-1">
          <input
            ref={ref}
            type={reveal ? "text" : "password"}
            value={value ?? ""}
            onChange={(e) => onChange(e.target.value)}
            onBlur={onCommit}
            placeholder={placeholder ?? "sk-…"}
            spellCheck={false}
            autoComplete="off"
            className={cx(INPUT, "pr-9 font-mono text-[13px]")}
          />
          <button
            type="button"
            onClick={() => setReveal((r) => !r)}
            className="absolute top-1/2 right-2 -translate-y-1/2 text-neutral-500 hover:text-neutral-200"
            aria-label={reveal ? "Ẩn key" : "Hiện key"}
          >
            {reveal ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
          </button>
        </div>
        {hasSaved && (
          <Button icon={<Undo2 className="size-3.5" />} onClick={() => onChange(saved)} title="Giữ key đã lưu">
            Giữ key cũ
          </Button>
        )}
      </div>
      {hasSaved && !(value ?? "").trim() && (
        <span className="mt-1 block text-[11px] text-amber-400">Để trống khi lưu sẽ xoá key đã lưu.</span>
      )}
    </div>
  );
}

const MAX_OPTIONS = 300;

/** Combobox: gõ để lọc danh sách model của provider; vẫn cho nhập tay model không có trong list. */
export function ModelPicker({ value, models, onChange }: { value: string; models: string[]; onChange: (m: string) => void }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);

  const filtered = useMemo(() => {
    const q = (query ?? "").trim().toLowerCase();
    const list = q ? models.filter((m) => m.toLowerCase().includes(q)) : models;
    return list.slice(0, MAX_OPTIONS);
  }, [models, query]);

  useEffect(() => setActive(0), [query, open]);
  useEffect(() => {
    listRef.current?.children[active]?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const pick = (m: string) => {
    onChange(m);
    setOpen(false);
    setQuery(null);
  };
  const unknown = models.length > 0 && value.trim() !== "" && !models.includes(value.trim());

  return (
    <div className="relative">
      <div className="relative">
        <input
          value={query ?? value}
          onChange={(e) => {
            setQuery(e.target.value);
            onChange(e.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onBlur={() => {
            setOpen(false);
            setQuery(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setOpen(true);
              setActive((a) => Math.min(a + 1, filtered.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => Math.max(a - 1, 0));
            } else if (e.key === "Enter" && open && filtered[active]) {
              e.preventDefault();
              pick(filtered[active]);
            } else if (e.key === "Escape") {
              setOpen(false);
              setQuery(null);
            }
          }}
          placeholder={models.length ? `Chọn trong ${models.length} model…` : "Nhập tên model"}
          spellCheck={false}
          autoComplete="off"
          role="combobox"
          aria-expanded={open}
          className={cx(INPUT, "pr-8 font-mono text-[13px]")}
        />
        <ChevronDown className="pointer-events-none absolute top-1/2 right-2.5 size-4 -translate-y-1/2 text-neutral-500" />
      </div>
      {open && models.length > 0 && (
        <ul
          ref={listRef}
          role="listbox"
          className="absolute z-30 mt-1 max-h-64 w-full overflow-y-auto rounded-lg border border-neutral-700 bg-neutral-900 py-1 shadow-xl shadow-black/50"
        >
          {filtered.length === 0 ? (
            <li className="px-3 py-1.5 text-xs text-neutral-500">Không có model khớp, sẽ dùng đúng tên bạn nhập.</li>
          ) : (
            filtered.map((m, i) => (
              <li
                key={m}
                role="option"
                aria-selected={m === value}
                onMouseDown={(e) => {
                  e.preventDefault();
                  pick(m);
                }}
                onMouseEnter={() => setActive(i)}
                className={cx(
                  "cursor-pointer truncate px-3 py-1 font-mono text-[12.5px]",
                  i === active ? "bg-violet-500/20 text-neutral-50" : "text-neutral-300",
                  m === value && "text-violet-300",
                )}
              >
                {m}
              </li>
            ))
          )}
          {models.length > MAX_OPTIONS && filtered.length === MAX_OPTIONS && (
            <li className="px-3 py-1 text-[11px] text-neutral-500">Gõ để lọc thêm ({models.length} model)…</li>
          )}
        </ul>
      )}
      {unknown && <span className="mt-1 block text-[11px] text-amber-400">Không có trong danh sách của provider</span>}
    </div>
  );
}

/** Dropdown chọn role, nhóm theo cấp bậc. */
export function RoleSelect({
  roles,
  value,
  onChange,
  exclude = [],
  placeholder = "-- chọn role --",
}: {
  roles: RoleDef[];
  value: RoleId | "";
  onChange: (id: RoleId) => void;
  exclude?: RoleId[];
  placeholder?: string;
}) {
  const known = roles.some((r) => r.id === value);
  return (
    <select value={known ? value : ""} onChange={(e) => e.target.value && onChange(e.target.value)} className={INPUT}>
      <option value="">{placeholder}</option>
      {LEVELS.map((l) => {
        const list = roles.filter((r) => r.level === l.id && (r.id === value || !exclude.includes(r.id)));
        if (list.length === 0) return null;
        return (
          <optgroup key={l.id} label={l.label}>
            {list.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name || "(chưa đặt tên)"}
                {r.title && r.title !== r.name ? ` — ${r.title}` : ""}
              </option>
            ))}
          </optgroup>
        );
      })}
    </select>
  );
}

/** Danh sách role dạng chip (có avatar pixel) + ô thêm. */
export function RoleChips({
  roles,
  value,
  onChange,
  exclude = [],
  max = 12,
}: {
  roles: RoleDef[];
  value: RoleId[];
  onChange: (ids: RoleId[]) => void;
  exclude?: RoleId[];
  max?: number;
}) {
  const resolve = useMemo(() => makeResolver([], roles), [roles]);
  const [adding, setAdding] = useState(false);
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {value.map((id) => {
        const m = resolve(id);
        const missing = !roles.some((r) => r.id === id);
        return (
          <span
            key={id}
            className={cx(
              "inline-flex items-center gap-1.5 rounded-full border py-0.5 pr-1 pl-0.5 text-xs",
              missing ? "border-red-500/50 text-red-300" : "border-neutral-700 text-neutral-200",
            )}
            title={missing ? "Role không tồn tại" : m.title}
          >
            <PixelPortrait meta={m} size={18} />
            {m.name}
            <button
              type="button"
              onClick={() => onChange(value.filter((x) => x !== id))}
              className="rounded-full p-0.5 text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
              aria-label={`Bỏ ${m.name}`}
            >
              <X className="size-3" />
            </button>
          </span>
        );
      })}
      {value.length < max &&
        (adding ? (
          <div className="w-60">
            <RoleSelect
              roles={roles}
              value=""
              exclude={[...value, ...exclude]}
              placeholder="-- thêm role --"
              onChange={(id) => {
                onChange([...value, id]);
                setAdding(false);
              }}
            />
          </div>
        ) : (
          <button
            type="button"
            onClick={() => setAdding(true)}
            className="inline-flex items-center gap-1 rounded-full border border-dashed border-neutral-700 px-2 py-0.5 text-xs text-neutral-400 hover:border-neutral-500 hover:text-neutral-200"
          >
            <Plus className="size-3" /> Thêm
          </button>
        ))}
    </div>
  );
}

export const newId = (prefix: string) =>
  typeof crypto !== "undefined" && "randomUUID" in crypto
    ? `${prefix}-${crypto.randomUUID().slice(0, 8)}`
    : `${prefix}-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
