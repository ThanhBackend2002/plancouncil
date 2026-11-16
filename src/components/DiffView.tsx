import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { diffLines, diffWords, type ChangeObject } from "diff";
import { ChevronDown, ChevronRight } from "lucide-react";
import { headingNumber, numbersMatch } from "../lib/sectionStatus";
import { pairSections, type SectionPair, type SectionStatus } from "../lib/sections";
import { Badge, cx } from "./ui";

export interface DiffSource {
  id: string;
  label: string;
  markdown: string;
}

const STATUS: Record<SectionStatus, { label: string; tone: "success" | "danger" | "warning" | "neutral" }> = {
  added: { label: "Thêm mới", tone: "success" },
  removed: { label: "Đã xoá", tone: "danger" },
  modified: { label: "Sửa đổi", tone: "warning" },
  unchanged: { label: "Không đổi", tone: "neutral" },
};

export function DiffView({
  sources,
  initialPair,
  focusNumber,
}: {
  sources: DiffSource[];
  /** Cặp version mở sẵn (id nguồn, vd "v1" → "v2"); mặc định "trước → mới nhất". */
  initialPair?: { a: string; b: string };
  /** Số hiệu section cần cuộn tới (vd "3.3"); `###` rơi về section `##` cha. */
  focusNumber?: string;
}) {
  const versions = sources.filter((s) => s.id !== "final");
  const has = (id?: string) => !!id && sources.some((s) => s.id === id);
  const [aId, setAId] = useState(() =>
    has(initialPair?.a) ? initialPair!.a : (versions.at(-2)?.id ?? sources[0]?.id ?? ""),
  );
  const [bId, setBId] = useState(() =>
    has(initialPair?.b) ? initialPair!.b : (versions.at(-1)?.id ?? sources.at(-1)?.id ?? ""),
  );
  const [onlyChanged, setOnlyChanged] = useState(!focusNumber);
  const listRef = useRef<HTMLDivElement>(null);

  const a = sources.find((s) => s.id === aId) ?? sources[0];
  const b = sources.find((s) => s.id === bId) ?? sources.at(-1);
  const pairs = useMemo(() => (a && b ? pairSections(a.markdown, b.markdown) : []), [a, b]);

  // Cuộn tới section được yêu cầu (hook phải đứng trước mọi return sớm).
  useEffect(() => {
    if (!focusNumber) return;
    const target = listRef.current?.querySelector<HTMLElement>("[data-diff-focus]");
    target?.scrollIntoView({ block: "start" });
  }, [focusNumber, pairs]);

  if (!a || !b) return <p className="p-6 text-sm text-fg-subtle">Cần ít nhất 2 version để so sánh.</p>;
  // Section `##` cha của số hiệu cần xem (pairSections chỉ tách theo `##`).
  const focusKey = focusNumber
    ? pairs.find((p) => {
        const n = headingNumber(p.titleB ?? p.titleA ?? "");
        return !!n && numbersMatch(n, focusNumber) && (n.split(".").length <= focusNumber.split(".").length || n === focusNumber);
      })?.key
    : undefined;

  const counts = pairs.reduce<Record<SectionStatus, number>>(
    (acc, p) => ({ ...acc, [p.status]: acc[p.status] + 1 }),
    { added: 0, removed: 0, modified: 0, unchanged: 0 },
  );
  const shown = onlyChanged ? pairs.filter((p) => p.status !== "unchanged") : pairs;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2 text-xs">
        <VersionSelect value={a.id} onChange={setAId} sources={sources} label="Từ" />
        <span className="text-fg-faint">→</span>
        <VersionSelect value={b.id} onChange={setBId} sources={sources} label="Đến" />
        <div className="ml-2 flex gap-1.5">
          {(["modified", "added", "removed"] as const).map(
            (k) => counts[k] > 0 && <Badge key={k} tone={STATUS[k].tone}>{counts[k]} {STATUS[k].label.toLowerCase()}</Badge>,
          )}
        </div>
        <label className="ml-auto inline-flex cursor-pointer items-center gap-1.5 text-fg-muted">
          <input type="checkbox" checked={onlyChanged} onChange={(e) => setOnlyChanged(e.target.checked)} className="accent-accent" />
          Chỉ xem section thay đổi
        </label>
      </div>
      <div ref={listRef} className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4">
        {a.id === b.id && <p className="text-sm text-fg-subtle">Hai bên đang chọn cùng một version.</p>}
        {shown.length === 0 && a.id !== b.id && <p className="text-sm text-fg-subtle">Không có section nào thay đổi.</p>}
        {shown.map((p) => (
          <div key={p.key} data-diff-focus={p.key === focusKey ? "" : undefined}>
            <SectionDiff pair={p} />
          </div>
        ))}
      </div>
    </div>
  );
}

function VersionSelect({
  value,
  onChange,
  sources,
  label,
}: {
  value: string;
  onChange: (id: string) => void;
  sources: DiffSource[];
  label: string;
}) {
  return (
    <label className="inline-flex items-center gap-1.5 text-fg-muted">
      {label}
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="rounded border border-line-strong bg-surface px-2 py-1 text-fg-soft"
      >
        {sources.map((s) => (
          <option key={s.id} value={s.id}>
            {s.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function SectionDiff({ pair }: { pair: SectionPair }) {
  const [open, setOpen] = useState(pair.status !== "unchanged");
  const parts = useMemo(() => (open ? diffLines(pair.a, pair.b) : []), [open, pair.a, pair.b]);
  const stats = useMemo(() => {
    const all = diffLines(pair.a, pair.b);
    let add = 0;
    let del = 0;
    for (const c of all) {
      if (c.added) add += c.count ?? 0;
      else if (c.removed) del += c.count ?? 0;
    }
    return { add, del };
  }, [pair.a, pair.b]);

  const title = pair.titleB ?? pair.titleA ?? "";
  const renamed = pair.titleA && pair.titleB && pair.titleA !== pair.titleB;

  return (
    <div
      className={cx(
        "overflow-hidden rounded-lg border",
        pair.status === "added"
          ? "border-success-line"
          : pair.status === "removed"
            ? "border-danger-line"
            : pair.status === "modified"
              ? "border-warn-line"
              : "border-line",
      )}
    >
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 bg-surface/70 px-3 py-2 text-left text-sm hover:bg-surface"
      >
        {open ? <ChevronDown className="size-4 text-fg-subtle" /> : <ChevronRight className="size-4 text-fg-subtle" />}
        <span className="font-medium text-fg">{title}</span>
        {renamed && <span className="text-xs text-fg-subtle line-through">{pair.titleA}</span>}
        <span className="ml-auto flex items-center gap-2 text-xs tabular-nums">
          {stats.add > 0 && <span className="text-success">+{stats.add}</span>}
          {stats.del > 0 && <span className="text-danger">−{stats.del}</span>}
          <Badge tone={STATUS[pair.status].tone}>{STATUS[pair.status].label}</Badge>
        </span>
      </button>
      {open && (
        <div className="overflow-x-auto bg-canvas py-1 font-mono text-[12.5px] leading-relaxed">
          <Hunks parts={parts} />
        </div>
      )}
    </div>
  );
}

const CONTEXT = 3;

function Hunks({ parts }: { parts: ChangeObject<string>[] }) {
  const rows: ReactNode[] = [];
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i]!;
    const next = parts[i + 1];
    if (p.removed && next?.added) {
      rows.push(<WordPair key={i} removed={p.value} added={next.value} />);
      i++;
    } else if (p.removed) {
      rows.push(<Lines key={i} text={p.value} kind="del" />);
    } else if (p.added) {
      rows.push(<Lines key={i} text={p.value} kind="add" />);
    } else {
      rows.push(<Context key={i} text={p.value} first={i === 0} last={i === parts.length - 1} />);
    }
  }
  return <>{rows}</>;
}

const ROW = {
  add: "bg-success-subtle text-success border-l-2 border-success-solid/60",
  del: "bg-danger-subtle text-danger border-l-2 border-danger-solid/60",
  ctx: "text-fg-muted border-l-2 border-transparent",
};
const SIGN = { add: "+", del: "−", ctx: " " };

function splitLines(text: string): string[] {
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

function Lines({ text, kind }: { text: string; kind: "add" | "del" | "ctx" }) {
  return (
    <>
      {splitLines(text).map((l, i) => (
        <div key={i} className={cx("flex px-3 whitespace-pre-wrap", ROW[kind])}>
          <span className="w-4 shrink-0 text-fg-faint select-none">{SIGN[kind]}</span>
          <span className="min-w-0 break-words">{l || " "}</span>
        </div>
      ))}
    </>
  );
}

function Context({ text, first, last }: { text: string; first: boolean; last: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const lines = splitLines(text);
  const head = first ? 0 : CONTEXT;
  const tail = last ? 0 : CONTEXT;
  if (expanded || lines.length <= head + tail + 2) return <Lines text={text} kind="ctx" />;
  const hidden = lines.length - head - tail;
  return (
    <>
      {head > 0 && <Lines text={lines.slice(0, head).join("\n")} kind="ctx" />}
      <button
        type="button"
        onClick={() => setExpanded(true)}
        className="my-0.5 w-full bg-surface/70 px-3 py-0.5 text-left text-[11px] text-fg-subtle hover:text-fg-soft"
      >
        ⋯ {hidden} dòng không đổi
      </button>
      {tail > 0 && <Lines text={lines.slice(-tail).join("\n")} kind="ctx" />}
    </>
  );
}

/** Cặp xoá→thêm liền nhau: highlight từng từ thay đổi. */
function WordPair({ removed, added }: { removed: string; added: string }) {
  const words = useMemo(() => diffWords(removed, added), [removed, added]);
  return (
    <>
      <div className={cx("flex px-3 whitespace-pre-wrap", ROW.del)}>
        <span className="w-4 shrink-0 text-fg-faint select-none">−</span>
        <span className="min-w-0 break-words">
          {words.map((w, i) =>
            w.added ? null : (
              <span key={i} className={w.removed ? "rounded-sm bg-danger-solid/35 text-fg" : undefined}>
                {w.value}
              </span>
            ),
          )}
        </span>
      </div>
      <div className={cx("flex px-3 whitespace-pre-wrap", ROW.add)}>
        <span className="w-4 shrink-0 text-fg-faint select-none">+</span>
        <span className="min-w-0 break-words">
          {words.map((w, i) =>
            w.removed ? null : (
              <span key={i} className={w.added ? "rounded-sm bg-success-solid/35 text-fg" : undefined}>
                {w.value}
              </span>
            ),
          )}
        </span>
      </div>
    </>
  );
}
