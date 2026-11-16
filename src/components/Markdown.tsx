import { memo, useDeferredValue, useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Mermaid } from "./Mermaid";

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
    return <span className="text-neutral-500">[ảnh: {alt}]</span>;
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
const Section = memo(function Section({ markdown }: { markdown: string }) {
  return (
    <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>
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
const Rendered = memo(function Rendered({ markdown }: { markdown: string }) {
  const blocks = useMemo(() => splitBlocks(markdown), [markdown]);
  return (
    <>
      {blocks.map((b, i) => (
        <Section key={i} markdown={b} />
      ))}
    </>
  );
});

export function PlanMarkdown({ markdown, streaming }: { markdown: string; streaming: boolean }) {
  // Hoãn re-render khi delta tới dày → UI không bị giật.
  const deferred = useDeferredValue(markdown);
  const { closed, open } = useMemo(() => {
    if (!streaming) return { closed: deferred, open: null };
    return splitOpenFence(stripWrappingFence(deferred));
  }, [deferred, streaming]);

  return (
    <div className={`plan-md ${streaming && !open ? "stream-caret" : ""}`}>
      <Rendered markdown={closed} />
      {open !== null && (
        <pre className="stream-caret opacity-70">
          <code>{open.replace(/^[^\n]*\n?/, "")}</code>
        </pre>
      )}
    </div>
  );
}
