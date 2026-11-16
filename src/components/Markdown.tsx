import { Children, createContext, isValidElement, memo, useContext, useDeferredValue, useMemo, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { CircleCheck, CircleHelp, GitCompareArrows, TriangleAlert } from "lucide-react";
import { headingNumber, headingNumbers, type SectionBadgeInfo, type SectionStatusKind } from "../lib/sectionStatus";
import { Mermaid } from "./Mermaid";

/** Trạng thái từng section theo số hiệu heading ("3", "3.3"). */
export type BadgeMap = Record<string, SectionBadgeInfo>;

export interface SectionActions {
  /** Bấm badge → lọc phản hồi trong Timeline theo section. */
  onFilter: (number: string) => void;
  /** "So sánh với version trước" tại section. */
  onCompare?: (number: string) => void;
}
const ActionsContext = createContext<SectionActions | null>(null);

const BADGE: Record<SectionStatusKind, { icon: typeof CircleCheck; label: string; cls: string }> = {
  changes_requested: { icon: TriangleAlert, label: "cần sửa", cls: "border-warn-line bg-warn-subtle text-warn" },
  approved: { icon: CircleCheck, label: "đã duyệt", cls: "border-success-line bg-success-subtle text-success" },
  pending: { icon: CircleHelp, label: "chưa xác nhận", cls: "border-line bg-surface-2 text-fg-muted" },
};

/** key ổn định cho memo: chỉ gồm trạng thái các heading nằm trong 1 khối. */
function encodeBadges(map: BadgeMap | undefined, numbers: string[]): string {
  if (!map) return "";
  return numbers
    .filter((n) => map[n])
    .map((n) => `${n}=${map[n]!.status}:${map[n]!.count}:${map[n]!.changed ? 1 : 0}`)
    .join("|");
}

function decodeBadges(key: string): BadgeMap {
  const out: BadgeMap = {};
  for (const part of key.split("|")) {
    if (!part) continue;
    const [n, rest] = part.split("=");
    const [status, count, changed] = (rest ?? "").split(":");
    if (n) out[n] = { status: status as SectionStatusKind, count: Number(count), changed: changed === "1" };
  }
  return out;
}

const textOfNode = (node: ReactNode): string =>
  Children.toArray(node)
    .map((c) => (typeof c === "string" || typeof c === "number" ? String(c) : isValidElement(c) ? textOfNode((c.props as { children?: ReactNode }).children) : ""))
    .join("");

function BadgeButtons({ number, info, level }: { number: string; info: SectionBadgeInfo; level: 2 | 3 }) {
  const actions = useContext(ActionsContext);
  const b = BADGE[info.status];
  const Icon = b.icon;
  const text = info.status === "changes_requested" ? `${b.label} (${info.count})` : b.label;
  const canCompare = actions?.onCompare && (info.status === "changes_requested" || (level === 2 && info.changed));
  return (
    <span className="ml-2 inline-flex flex-wrap items-center gap-1.5 align-middle text-[11px] font-medium not-italic">
      <button
        type="button"
        disabled={!actions || info.status !== "changes_requested"}
        onClick={() => actions?.onFilter(number)}
        title={info.status === "changes_requested" ? `Lọc hội thoại theo phản hồi về mục ${number}` : `Mục ${number}: ${b.label}`}
        aria-label={`Mục ${number}: ${text}`}
        className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 ${b.cls} disabled:cursor-default enabled:hover:brightness-110`}
      >
        <Icon className="size-3" aria-hidden />
        {text}
      </button>
      {canCompare && (
        <button
          type="button"
          onClick={() => actions.onCompare?.(number)}
          className="inline-flex items-center gap-1 rounded-full border border-line-strong px-2 py-0.5 text-fg-muted hover:bg-surface-2 hover:text-fg"
        >
          <GitCompareArrows className="size-3" aria-hidden />
          So sánh với version trước
        </button>
      )}
    </span>
  );
}

function makeHeading(level: 2 | 3, map: BadgeMap): Components["h2"] {
  const Tag = level === 2 ? "h2" : "h3";
  return function SectionHeading({ node: _node, children, ...rest }) {
    const n = headingNumber(textOfNode(children));
    const info = n ? map[n] : undefined;
    return (
      <Tag {...rest}>
        {children}
        {n && info && <BadgeButtons number={n} info={info} level={level} />}
      </Tag>
    );
  };
}

interface HastLike {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastLike[];
}

const textOf = (n: HastLike): string => (n.type === "text" ? (n.value ?? "") : (n.children ?? []).map(textOf).join(""));

const components: Components = {
  pre({ node, children, ...rest }) {
    const code = (node as HastLike | undefined)?.children?.find((c) => c.type === "element" && c.tagName === "code");
    const cls = code?.properties?.className;
    const classes = Array.isArray(cls) ? cls.map(String) : [];
    if (code && classes.includes("language-mermaid")) {
      return <Mermaid code={textOf(code).trim()} />;
    }
    return <pre {...rest}>{children}</pre>;
  },
  // Không cho webview điều hướng ra ngoài (Tauri sẽ thay cả app bằng trang web).
  a({ node: _node, href, children, ...rest }) {
    return (
      <a {...rest} href={href} title={href} onClick={(e) => e.preventDefault()}>
        {children}
      </a>
    );
  },
  img({ node: _node, alt }) {
    return <span className="text-fg-subtle">[ảnh: {alt}]</span>;
  },
};

const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/;

/**
 * Tách phần cuối nếu đang có code fence chưa đóng (lúc stream). Phần `closed`
 * chắc chắn chỉ chứa fence đã đóng → Mermaid chỉ render khi khối đã hoàn chỉnh.
 */
export function splitOpenFence(md: string): { closed: string; open: string | null } {
  let inFence = false;
  let char = "";
  let len = 0;
  let openAt = -1;
  let offset = 0;
  for (const line of md.split("\n")) {
    const m = FENCE.exec(line);
    const marker = m?.[1];
    if (marker) {
      if (!inFence) {
        inFence = true;
        char = marker[0]!;
        len = marker.length;
        openAt = offset;
      } else if (marker[0] === char && marker.length >= len && (m[2] ?? "").trim() === "") {
        inFence = false;
      }
    }
    offset += line.length + 1;
  }
  return inFence ? { closed: md.slice(0, openAt), open: md.slice(openAt) } : { closed: md, open: null };
}

/** Lúc stream, model đôi khi bọc cả plan trong ```markdown (backend chỉ gỡ khi plan_ready). */
function stripWrappingFence(md: string): string {
  const m = /^\s*```(?:markdown|md)?[ \t]*\n/.exec(md);
  return m ? md.slice(m[0].length) : md;
}

const remarkPlugins = [remarkGfm];

/** 1 khối markdown → React. memo theo chuỗi: khối không đổi thì không parse lại. */
const Section = memo(function Section({ markdown, badgeKey }: { markdown: string; badgeKey: string }) {
  // Chỉ nhận trạng thái của các heading trong CHÍNH khối này (badgeKey là chuỗi) → khối khác đổi
  // trạng thái không làm khối này parse lại.
  const comps = useMemo<Components>(() => {
    if (!badgeKey) return components;
    const map = decodeBadges(badgeKey);
    return { ...components, h2: makeHeading(2, map), h3: makeHeading(3, map) };
  }, [badgeKey]);
  return (
    <ReactMarkdown remarkPlugins={remarkPlugins} components={comps}>
      {markdown}
    </ReactMarkdown>
  );
});

const H2_LINE = /^##(?:[ \t]|$)/;
/** Định nghĩa link tham chiếu / footnote (`[x]: url`, `[^1]: …`) — có thể được dùng ở section khác. */
const DEFINITION = /^ {0,3}\[[^\]\n]+\]:/m;
/** Dòng mở HTML block (`<div>`, `<!-- … -->`…): HTML block có thể nuốt cả dòng `## ` phía sau. */
const HTML_BLOCK = /^ {0,3}<[A-Za-z!?/]/m;

/**
 * Cắt markdown thành các khối, mỗi khối bắt đầu ở 1 dòng `## ` (cột 0, ngoài code fence; giữ
 * nguyên dòng heading trong khối). Không dùng `splitSections` của lib/sections vì hàm đó chuẩn
 * hoá title/body cho Diff View, còn ở đây cần chuỗi gốc để render y hệt.
 *
 * Heading ATX ở cột 0 luôn kết thúc đoạn văn / list / blockquote / bảng trước nó, nên render
 * từng khối riêng ra cùng DOM như render cả tài liệu (react-markdown 10 không bọc thẻ ngoài).
 * Ngoại lệ: có định nghĩa link tham chiếu / footnote (dùng xuyên khối) hoặc HTML block
 * → không cắt, render nguyên tài liệu như cũ.
 */
export function splitBlocks(md: string): string[] {
  if (DEFINITION.test(md) || HTML_BLOCK.test(md)) return [md];
  const lines = md.split("\n");
  const out: string[] = [];
  let start = 0;
  let inFence = false;
  let char = "";
  let len = 0;
  lines.forEach((line, i) => {
    const m = FENCE.exec(line);
    const marker = m?.[1];
    if (marker) {
      if (!inFence) {
        inFence = true;
        char = marker[0]!;
        len = marker.length;
      } else if (marker[0] === char && marker.length >= len && (m[2] ?? "").trim() === "") {
        inFence = false;
      }
      return;
    }
    if (!inFence && i > start && H2_LINE.test(line)) {
      out.push(lines.slice(start, i).join("\n"));
      start = i;
    }
  });
  out.push(lines.slice(start).join("\n"));
  return out;
}

/**
 * Render theo từng section `##`, mỗi section là 1 `Section` memo (key = vị trí, so sánh theo nội
 * dung). Lúc stream chỉ section cuối đổi → chỉ section đó parse lại, các section trước giữ nguyên
 * (kể cả Mermaid đã vẽ, vì key theo vị trí nên không bị remount).
 */
const Rendered = memo(function Rendered({ markdown, badges }: { markdown: string; badges?: BadgeMap }) {
  const blocks = useMemo(() => splitBlocks(markdown), [markdown]);
  const numbers = useMemo(() => blocks.map((b) => headingNumbers(b)), [blocks]);
  return (
    <>
      {blocks.map((b, i) => (
        <Section key={i} markdown={b} badgeKey={encodeBadges(badges, numbers[i] ?? [])} />
      ))}
    </>
  );
});

export function PlanMarkdown({
  markdown,
  streaming,
  badges,
  actions,
}: {
  markdown: string;
  streaming: boolean;
  /** Badge theo section; bỏ trống (hoặc đang stream) = không hiện badge. */
  badges?: BadgeMap;
  actions?: SectionActions;
}) {
  // Hoãn re-render khi delta tới dày → UI không bị giật.
  const deferred = useDeferredValue(markdown);
  const { closed, open } = useMemo(() => {
    if (!streaming) return { closed: deferred, open: null };
    return splitOpenFence(stripWrappingFence(deferred));
  }, [deferred, streaming]);

  return (
    <div className={`plan-md ${streaming && !open ? "stream-caret" : ""}`}>
      <ActionsContext.Provider value={actions ?? null}>
        <Rendered markdown={closed} badges={streaming ? undefined : badges} />
      </ActionsContext.Provider>
      {open !== null && (
        <pre className="stream-caret opacity-70">
          <code>{open.replace(/^[^\n]*\n?/, "")}</code>
        </pre>
      )}
    </div>
  );
}
