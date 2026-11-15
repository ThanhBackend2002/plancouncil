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

const Rendered = memo(function Rendered({ markdown }: { markdown: string }) {
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
      {markdown}
    </ReactMarkdown>
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
