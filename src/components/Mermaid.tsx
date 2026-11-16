import { memo, useEffect, useState } from "react";
import { LoaderCircle, TriangleExclamationPoint } from "lucide-react";
import { useUi } from "../store/ui";

type MermaidApi = (typeof import("mermaid"))["default"];
type Result = { svg: string } | { error: string };
type Theme = "dark" | "light";

let apiPromise: Promise<MermaidApi> | null = null;
let initedTheme: Theme | null = null;

/** Lazy-load: mermaid khá nặng, chỉ tải khi plan có sơ đồ. */
function loadMermaid(): Promise<MermaidApi> {
  apiPromise ??= import("mermaid").then(({ default: api }) => api);
  return apiPromise;
}

/** Khởi tạo lại mermaid khi đổi theme (chỉ gọi trong hàng đợi render nên không chen ngang). */
function ensureTheme(api: MermaidApi, theme: Theme) {
  if (initedTheme === theme) return;
  initedTheme = theme;
  // Nền sơ đồ lấy từ token --code-bg của theme hiện tại.
  const background = getComputedStyle(document.documentElement).getPropertyValue("--code-bg").trim() || undefined;
  api.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: theme === "dark" ? "dark" : "default",
    fontFamily: "inherit",
    themeVariables: background ? { background } : {},
  });
}

// Cache theo theme + source → khối đã render không render lại khi Markdown re-render lúc stream;
// đổi theme thì key khác → vẽ lại một lần với theme mới.
const CACHE_LIMIT = 100;
const cache = new Map<string, Result>();
let counter = 0;
// mermaid.render dùng DOM tạm, không an toàn khi gọi đồng thời → xếp hàng tuần tự.
let queue: Promise<unknown> = Promise.resolve();

function remember(code: string, r: Result) {
  cache.set(code, r);
  if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
}

const keyOf = (theme: Theme, code: string) => `${theme}\u0000${code}`;

function renderMermaid(code: string, theme: Theme): Promise<Result> {
  const key = keyOf(theme, code);
  const hit = cache.get(key);
  if (hit) return Promise.resolve(hit);
  const job = queue.then(async (): Promise<Result> => {
    const again = cache.get(key);
    if (again) return again;
    const id = `mmd-${++counter}`;
    let result: Result;
    try {
      const api = await loadMermaid();
      ensureTheme(api, theme);
      const { svg } = await api.render(id, code);
      result = { svg };
    } catch (e) {
      result = { error: e instanceof Error ? e.message : String(e) };
      // Mermaid có thể để lại node lỗi trong <body>.
      document.getElementById(id)?.remove();
      document.getElementById(`d${id}`)?.remove();
    }
    remember(key, result);
    return result;
  });
  queue = job.catch(() => undefined);
  return job;
}

export const Mermaid = memo(function Mermaid({ code }: { code: string }) {
  const theme = useUi((s) => s.resolvedTheme);
  const [result, setResult] = useState<Result | null>(() => cache.get(keyOf(theme, code)) ?? null);

  useEffect(() => {
    let alive = true;
    const hit = cache.get(keyOf(theme, code));
    if (hit) {
      setResult(hit);
      return;
    }
    // Đổi theme: giữ sơ đồ cũ trong lúc vẽ lại, tránh nháy "Đang vẽ…".
    setResult((prev) => (prev && "svg" in prev ? prev : null));
    void renderMermaid(code, theme).then((r) => alive && setResult(r));
    return () => {
      alive = false;
    };
  }, [code, theme]);

  if (!result) {
    return (
      <div className="flex items-center gap-2 rounded-lg border border-line bg-canvas p-4 text-xs text-fg-subtle">
        <LoaderCircle className="size-4 animate-spin" /> Đang vẽ sơ đồ…
      </div>
    );
  }
  if ("error" in result) {
    return (
      <div className="rounded-lg border border-warn-line bg-warn-subtle/50">
        <div className="flex items-start gap-2 border-b border-warn-line px-3 py-2 text-xs text-warn">
          <TriangleExclamationPoint className="mt-px size-3.5 shrink-0" />
          <span className="break-words">
            Sơ đồ Mermaid lỗi cú pháp, đang hiện code gốc. <span className="text-warn/70">{firstLine(result.error)}</span>
          </span>
        </div>
        <pre className="!m-0 !rounded-t-none !border-0">
          <code>{code}</code>
        </pre>
      </div>
    );
  }
  return (
    <div
      className="mermaid-diagram flex justify-center overflow-x-auto rounded-lg border border-line bg-canvas p-4 [&_svg]:h-auto [&_svg]:max-w-full"
      // SVG đã được mermaid sanitize (securityLevel: "strict").
      dangerouslySetInnerHTML={{ __html: result.svg }}
    />
  );
});

function firstLine(s: string): string {
  const line = s.split("\n").find((l) => l.trim() !== "") ?? s;
  return line.length > 160 ? `${line.slice(0, 160)}…` : line;
}
