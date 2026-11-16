import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Coins, Copy, Download, FileText, GitCompareArrows, LoaderCircle, Sparkles } from "lucide-react";
import { formatTokens, type RoleResolver } from "../lib/agents";
import { activeOf, useActive, useActiveShallow, useCouncil, usePlanText } from "../store/council";
import { ipc } from "../lib/ipc";
import { planText, type PlanView, type ScopeState, type SessionState } from "../store/session";
import { useUi } from "../store/ui";
import { toast, toastError } from "../store/toast";
import { DiffView, type DiffSource } from "./DiffView";
import { UsagePanel } from "./UsagePanel";
import { PlanMarkdown } from "./Markdown";
import { Button, Panel, StatusBadge, cx } from "./ui";

type Selection = "auto" | "final" | "diff" | "usage" | number;
type Resolved = Exclude<Selection, "auto"> | null;

/** Text đang hiển thị (đọc cả bản nháp đang stream). Dùng cho selector và lúc bấm Copy/Export. */
function contentOf(s: SessionState, scopeId: string, resolved: Resolved, plan: PlanView | undefined, final: string): string {
  return resolved === "final" ? final : plan ? planText(s, scopeId, plan) : "";
}

// PlanViewer KHÔNG đọc text stream (chỉ đọc cờ "có nội dung chưa") → thanh tab/nút không re-render
// theo từng plan_delta; chỉ `PlanBody` bên dưới subscribe vào text.
export const PlanViewer = memo(function PlanViewer({ scope, resolve }: { scope: ScopeState; resolve: RoleResolver }) {
  const [sel, setSel] = useState<Selection>("auto");
  const [exporting, setExporting] = useState(false);
  const planFocus = useUi((s) => s.planFocus);
  const session = useActiveShallow((s) => ({ id: s.id, status: s.status, total: s.usage.total }));

  // Phiên mới / đổi phòng ban → quay về chế độ tự theo dõi.
  useEffect(() => setSel("auto"), [session.id, scope.id]);
  // Bấm "Xem vN" trong khung hội thoại.
  useEffect(() => {
    if (planFocus) setSel(planFocus.version);
  }, [planFocus]);

  const latest = scope.plans.at(-1);
  const resolved: Resolved = sel === "auto" ? (scope.final ? "final" : (latest?.version ?? null)) : sel;

  const plan = typeof resolved === "number" ? scope.plans.find((p) => p.version === resolved) : undefined;
  const running = scope.status === "running" && session.status === "running";
  const streaming = !!plan && !plan.complete && running;
  const finalMd = scope.final?.markdown ?? "";
  // Boolean → chỉ đổi 1 lần khi text bắt đầu có nội dung, không theo từng delta.
  const hasContent = useActive((s) => contentOf(s, scope.id, resolved, plan, finalMd).trim() !== "");
  /** Đọc text lúc cần (Copy/Export) thay vì subscribe. */
  const readContent = () => contentOf(activeOf(useCouncil.getState()), scope.id, resolved, plan, finalMd);

  const diffSources = useMemo<DiffSource[]>(() => {
    const list: DiffSource[] = scope.plans
      .filter((p) => p.complete)
      .map((p) => ({ id: `v${p.version}`, label: `v${p.version}`, markdown: p.markdown }));
    if (scope.final) list.push({ id: "final", label: "Final", markdown: scope.final.markdown });
    return list;
  }, [scope.plans, scope.final]);

  const label = resolved === "final" ? "final" : typeof resolved === "number" ? `v${resolved}` : "";
  const canAct = resolved !== "diff" && resolved !== "usage" && hasContent && !streaming;
  const tokens = session.total.prompt_tokens + session.total.completion_tokens;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(readContent());
      toast("success", "Đã copy plan", `${label} → clipboard`);
    } catch (e) {
      toastError(e, "Không copy được");
    }
  };

  const exportMd = async () => {
    setExporting(true);
    try {
      const path = await ipc.exportMarkdown(readContent(), `plancouncil-${(session.id ?? "plan").slice(0, 8)}-${scope.kind === "main" ? "" : `${scope.id}-`}${label}.md`);
      if (path) toast("success", "Đã export", path);
    } catch (e) {
      toastError(e, "Export thất bại");
    } finally {
      setExporting(false);
    }
  };

  const tabs: { key: Selection; label: React.ReactNode }[] = [
    ...scope.plans.map((p) => ({
      key: p.version as Selection,
      label: (
        <>
          v{p.version}
          {!p.complete && running && <LoaderCircle className="size-3 animate-spin" />}
        </>
      ),
    })),
    ...(scope.final
      ? [{ key: "final" as Selection, label: (<><Sparkles className="size-3" />Final</>) }]
      : []),
    ...(diffSources.length >= 2
      ? [{ key: "diff" as Selection, label: (<><GitCompareArrows className="size-3.5" />Diff</>) }]
      : []),
    ...(session.total.calls > 0
      ? [{ key: "usage" as Selection, label: (<><Coins className="size-3.5" />Token {formatTokens(tokens)}</>) }]
      : []),
  ];

  return (
    <Panel className="flex h-full flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-2 border-b border-neutral-800 px-3 py-2">
        <div role="tablist" className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {tabs.length === 0 && <span className="px-1 text-xs text-neutral-500">Plan Viewer</span>}
          {tabs.map((t) => {
            const active = resolved === t.key;
            return (
              <button
                key={String(t.key)}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => setSel(t.key)}
                className={cx(
                  "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium transition-colors",
                  active ? "bg-neutral-800 text-neutral-50" : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200",
                  t.key === "final" && !active && "text-violet-300",
                )}
              >
                {t.label}
              </button>
            );
          })}
          {sel !== "auto" && running && (
            <button
              type="button"
              onClick={() => setSel("auto")}
              className="ml-1 shrink-0 text-[11px] text-sky-400 hover:text-sky-300"
              title="Tự chuyển sang version mới nhất"
            >
              Theo dõi trực tiếp
            </button>
          )}
        </div>
        {scope.status !== "idle" && <StatusBadge status={scope.status} />}
        <Button size="sm" icon={<Copy className="size-3.5" />} disabled={!canAct} onClick={() => void copy()}>
          Copy Plan
        </Button>
        <Button size="sm" icon={<Download className="size-3.5" />} disabled={!canAct} loading={exporting} onClick={() => void exportMd()}>
          Export .md
        </Button>
      </div>

      {resolved === "usage" ? (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <UsagePanel resolve={resolve} />
        </div>
      ) : resolved === "diff" ? (
        <div className="min-h-0 flex-1">
          {/* key theo số version → khi có version mới, mặc định lại "trước → mới nhất". */}
          <DiffView key={`${session.id ?? "none"}-${scope.id}-${diffSources.length}`} sources={diffSources} />
        </div>
      ) : (
        <PlanBody
          scopeId={scope.id}
          resolved={resolved}
          plan={plan}
          finalMd={finalMd}
          streaming={streaming}
          running={running}
          label={label}
        />
      )}
    </Panel>
  );
});

/** Vùng hiển thị text plan — component DUY NHẤT re-render theo từng plan_delta. */
const PlanBody = memo(function PlanBody({
  scopeId,
  resolved,
  plan,
  finalMd,
  streaming,
  running,
  label,
}: {
  scopeId: string;
  resolved: Resolved;
  plan: PlanView | undefined;
  finalMd: string;
  streaming: boolean;
  running: boolean;
  label: string;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const draft = usePlanText(scopeId, plan);
  const content = resolved === "final" ? finalMd : draft;

  // Tự cuộn theo nội dung khi stream; dừng nếu người dùng cuộn lên.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && streaming && stick.current) el.scrollTop = el.scrollHeight;
  }, [content, streaming]);

  useLayoutEffect(() => {
    stick.current = true;
    const el = scrollRef.current;
    if (el) el.scrollTop = 0;
  }, [resolved]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  return (
    <div ref={scrollRef} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto">
      {resolved === null ? (
        <EmptyState running={running} />
      ) : content === "" && streaming ? (
        <div className="flex items-center gap-2 p-8 text-sm text-neutral-500">
          <LoaderCircle className="size-4 animate-spin" /> Architect đang viết {label}…
        </div>
      ) : (
        <article className="mx-auto max-w-[860px] px-8 py-6">
          {plan && !plan.complete && !running && (
            <p className="mb-4 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
              Version này chưa viết xong (phiên đã dừng/lỗi giữa chừng).
            </p>
          )}
          <PlanMarkdown markdown={content} streaming={streaming} />
        </article>
      )}
    </div>
  );
});

function EmptyState({ running }: { running: boolean }) {
  if (running) {
    return (
      <div className="flex h-full items-center justify-center gap-2 text-sm text-neutral-500">
        <LoaderCircle className="size-4 animate-spin" /> Đang chờ người viết plan…
      </div>
    );
  }
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 p-8 text-center">
      <FileText className="size-10 text-neutral-700" />
      <div>
        <h2 className="text-base font-semibold text-neutral-200">Chưa có plan nào</h2>
        <p className="mt-1 max-w-md text-sm text-neutral-500">
          Nhập yêu cầu ở bên trái rồi bấm <b className="text-neutral-300">Start Council</b>. Các phòng ban được chọn họp
          trước, sau đó chủ trì viết plan tổng và các reviewer phản biện cho tới khi cùng APPROVE.
        </p>
      </div>
      <ol className="space-y-1 text-left text-xs text-neutral-500">
        <li>1. Cài đặt → Provider: nhập API key, model được kéo về tự động</li>
        <li>2. Cài đặt → Vai trò / Phòng ban / Cuộc họp: chọn ai tham gia</li>
        <li>3. Chạy phiên, xem Diff giữa các version, Export .md</li>
      </ol>
    </div>
  );
}
