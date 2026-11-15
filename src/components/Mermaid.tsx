import { memo, useEffect, useState } from "react";
import { LoaderCircle, TriangleExclamationPoint } from "lucide-react";

type MermaidApi = (typeof import("mermaid"))["default"];
type Result = { svg: string } | { error: string };

let apiPromise: Promise<MermaidApi> | null = null;

/** Lazy-load: mermaid khá nặng, chỉ tải khi plan có sơ đồ. */
function loadMermaid(): Promise<MermaidApi> {
  apiPromise ??= import("mermaid").then(({ default: api }) => {
    api.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "dark",
      fontFamily: "inherit",
      themeVariables: { background: "#0f0f12" },
    });
    return api;
  });
  return apiPromise;
}

// Cache theo source → khối đã render không render lại khi Markdown re-render lúc stream.
const CACHE_LIMIT = 100;
const cache = new Map<string, Result>();
let counter = 0;
// mermaid.render dùng DOM tạm, không an toàn khi gọi đồng thời → xếp hàng tuần tự.
let queue: Promise<unknown> = Promise.resolve();

function remember(code: string, r: Result) {
  cache.set(code, r);
  if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
}

function renderMermaid(code: string): Promise<Result> {
  const hit = cache.get(code);
  if (hit) return Promise.resolve(hit);
  const job = queue.then(async (): Promise<Result> => {
    const again = cache.get(code);
    if (again) return again;
    const id = `mmd-${++counter}`;
    let result: Result;
    try {
      const api = await loadMermaid();
      const { svg } = await api.render(id, code);
      result = { svg };
    } catch (e) {
      result = { error: e instanceof Error ? e.message : String(e) };
      // Mermaid có thể để lại node lỗi trong <body>.
      document.getElementById(id)?.remove();
      document.getElementById(`d${id}`)?.remove();
    }
    remember(code, result);
    return result;
  });
  queue = job.catch(() => undefined);
  return job;
}

export const Mermaid = memo(function Mermaid({ code }: { code: string }) {
  const [result, setResult] = useState<Result | null>(() => cache.get(code) ?? null);

  useEffect(() => {
    let alive = true;
    const hit = cache.get(code);
    if (hit) {
      setResult(hit);
      return;
    }
    setResult(null);
    void renderMermaid(code).then((r) => alive && setResult(r));
    return () => {
      alive = false;
    };
  }, [code]);

  if (!result) {
    return (
      <div className="flex items-center gap-2 rounded-lg border border-neutral-800 bg-neutral-950 p-4 text-xs text-neutral-500">
        <LoaderCircle className="size-4 animate-spin" /> Đang vẽ sơ đồ…
      </div>
    );
  }
  if ("error" in result) {
    return (
      <div className="rounded-lg border border-amber-500/30 bg-amber-500/5">
        <div className="flex items-start gap-2 border-b border-amber-500/20 px-3 py-2 text-xs text-amber-300">
          <TriangleExclamationPoint className="mt-px size-3.5 shrink-0" />
          <span className="break-words">
            Sơ đồ Mermaid lỗi cú pháp, đang hiện code gốc. <span className="text-amber-400/70">{firstLine(result.error)}</span>
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
      className="mermaid-diagram flex justify-center overflow-x-auto rounded-lg border border-neutral-800 bg-neutral-950 p-4 [&_svg]:h-auto [&_svg]:max-w-full"
      // SVG đã được mermaid sanitize (securityLevel: "strict").
      dangerouslySetInnerHTML={{ __html: result.svg }}
    />
  );
});

function firstLine(s: string): string {
  const line = s.split("\n").find((l) => l.trim() !== "") ?? s;
  return line.length > 160 ? `${line.slice(0, 160)}…` : line;
}
