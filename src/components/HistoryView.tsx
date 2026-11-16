import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { AlertTriangle, Eye, FileText, LoaderCircle, RotateCcw, Search, SearchX, Trash, X } from "lucide-react";
import { formatDate, formatTokens } from "../lib/agents";
import { MAIN_SCOPE, type HistorySearchHit, type SessionSummary } from "../lib/contracts";
import { ipc } from "../lib/ipc";
import { splitSnippet } from "../lib/snippet";
import { useCouncil } from "../store/council";
import { SEARCH_LIMIT, useHistory } from "../store/history";
import { toastError } from "../store/toast";
import { useUi } from "../store/ui";
import { Badge, Button, StatusBadge, cx } from "./ui";

const RESULTS_ID = "history-search-results";
const SHORTCUT_LABEL = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent) ? "⌘K" : "Ctrl K";
const hitId = (i: number) => `history-hit-${i}`;

/** Mở 1 phiên đã lưu sang màn Phòng họp (dùng chung cho danh sách và kết quả tìm). */
function useOpenSession() {
  const openRecord = useCouncil((st) => st.openRecord);
  const setTab = useUi((st) => st.setTab);
  return useCallback(
    async (id: string) => {
      try {
        openRecord(await ipc.loadSession(id));
        setTab("council");
      } catch (e) {
        toastError(e, "Không mở được phiên");
      }
    },
    [openRecord, setTab],
  );
}

export function HistoryView() {
  const sessions = useHistory((st) => st.sessions);
  const loading = useHistory((st) => st.loading);
  const loaded = useHistory((st) => st.loaded);
  const refresh = useHistory((st) => st.refresh);
  const query = useHistory((st) => st.query);
  const hits = useHistory((st) => st.hits);
  const hitsFor = useHistory((st) => st.hitsFor);
  const searchLoading = useHistory((st) => st.searchLoading);
  const searchError = useHistory((st) => st.searchError);
  const setQuery = useHistory((st) => st.setQuery);
  const clearSearch = useHistory((st) => st.clearSearch);
  const focusNonce = useHistory((st) => st.focusNonce);
  const tab = useUi((st) => st.tab);
  const openSession = useOpenSession();

  const inputRef = useRef<HTMLInputElement>(null);
  const [active, setActive] = useState(0);
  const searching = query.trim() !== "";

  // Ctrl/Cmd+K: view có thể vừa chuyển từ `hidden` sang hiện → chờ 1 frame rồi mới focus.
  useEffect(() => {
    if (focusNonce === 0 || tab !== "history") return;
    const id = requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
    return () => cancelAnimationFrame(id);
  }, [focusNonce, tab]);

  useEffect(() => setActive(0), [hits]);

  useEffect(() => {
    document.getElementById(hitId(active))?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" && hits.length > 0) {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, hits.length - 1));
    } else if (e.key === "ArrowUp" && hits.length > 0) {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      const h = hits[active];
      if (h) {
        e.preventDefault();
        void openSession(h.session_id);
      }
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (query) clearSearch();
      else inputRef.current?.blur();
    }
  };

  const status = searching
    ? searchLoading && hitsFor === null
      ? "Đang tìm…"
      : searchError
        ? "Lỗi tìm kiếm"
        : `${hits.length}${hits.length >= SEARCH_LIMIT ? "+" : ""} kết quả`
    : `${sessions.length} phiên`;

  return (
    <div className="mx-auto flex h-full max-w-5xl flex-col p-6">
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-lg font-semibold">Lịch sử phiên họp</h1>
        <span className="text-sm text-neutral-500" aria-live="polite">
          {status}
        </span>
        <Button
          size="sm"
          className="ml-auto"
          icon={<RotateCcw className="size-3.5" />}
          loading={loading}
          onClick={() => void refresh()}
        >
          Làm mới
        </Button>
      </div>

      <div className="relative mb-3">
        <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-neutral-500" />
        <input
          ref={inputRef}
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Tìm trong lịch sử (không cần dấu)…"
          aria-label="Tìm trong lịch sử"
          role="combobox"
          aria-expanded={searching && hits.length > 0}
          aria-controls={RESULTS_ID}
          aria-autocomplete="list"
          aria-activedescendant={searching && hits[active] ? hitId(active) : undefined}
          className="w-full rounded-lg border border-neutral-800 bg-neutral-900 py-2 pr-20 pl-9 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-violet-500/60 focus:outline-none [&::-webkit-search-cancel-button]:hidden"
        />
        <div className="absolute top-1/2 right-2 flex -translate-y-1/2 items-center gap-1.5">
          {searchLoading && <LoaderCircle className="size-3.5 animate-spin text-neutral-500" />}
          {query ? (
            <button
              type="button"
              className="rounded p-1 text-neutral-500 hover:bg-neutral-800 hover:text-neutral-200"
              onClick={() => {
                clearSearch();
                inputRef.current?.focus();
              }}
              aria-label="Xoá tìm kiếm"
              title="Xoá (Esc)"
            >
              <X className="size-3.5" />
            </button>
          ) : (
            <kbd className="rounded border border-neutral-700 px-1.5 text-[10px] text-neutral-500">{SHORTCUT_LABEL}</kbd>
          )}
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto rounded-xl border border-neutral-800">
        {searching ? (
          <SearchResults active={active} setActive={setActive} onOpen={openSession} />
        ) : !loaded && loading ? (
          <div className="flex items-center justify-center gap-2 p-10 text-sm text-neutral-500">
            <LoaderCircle className="size-4 animate-spin" /> Đang tải…
          </div>
        ) : sessions.length === 0 ? (
          <div className="flex flex-col items-center gap-2 p-12 text-center text-sm text-neutral-500">
            <FileText className="size-8 text-neutral-700" />
            Chưa có phiên nào. Phiên sẽ được lưu tự động khi kết thúc.
          </div>
        ) : (
          <ul className="divide-y divide-neutral-800">
            {sessions.map((s) => (
              <Row key={s.id} s={s} />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function SearchResults({
  active,
  setActive,
  onOpen,
}: {
  active: number;
  setActive: (i: number) => void;
  onOpen: (id: string) => Promise<void>;
}) {
  const hits = useHistory((st) => st.hits);
  const hitsFor = useHistory((st) => st.hitsFor);
  const error = useHistory((st) => st.searchError);

  if (error) {
    return (
      <div role="alert" className="flex flex-col items-center gap-2 p-12 text-center text-sm text-neutral-400">
        <AlertTriangle className="size-8 text-amber-500/70" />
        <p className="font-medium text-neutral-200">
          {error.kind === "index_unavailable" ? "Tìm kiếm chưa khả dụng" : "Không tìm được"}
        </p>
        <p className="max-w-md text-neutral-500">{error.message}</p>
      </div>
    );
  }
  if (hitsFor === null) {
    return (
      <div className="flex items-center justify-center gap-2 p-10 text-sm text-neutral-500">
        <LoaderCircle className="size-4 animate-spin" /> Đang tìm…
      </div>
    );
  }
  if (hits.length === 0) {
    return (
      <div className="flex flex-col items-center gap-2 p-12 text-center text-sm text-neutral-500">
        <SearchX className="size-8 text-neutral-700" />
        Không có kết quả cho “{hitsFor}”. Thử từ khoá khác hoặc ngắn hơn.
      </div>
    );
  }
  return (
    <ul id={RESULTS_ID} role="listbox" aria-label="Kết quả tìm kiếm" className="divide-y divide-neutral-800">
      {hits.map((h, i) => (
        <HitRow
          key={`${h.session_id}:${h.scope}:${h.kind}:${h.section_key ?? ""}:${i}`}
          h={h}
          id={hitId(i)}
          selected={i === active}
          onHover={() => setActive(i)}
          onOpen={() => void onOpen(h.session_id)}
        />
      ))}
    </ul>
  );
}

function HitRow({
  h,
  id,
  selected,
  onHover,
  onOpen,
}: {
  h: HistorySearchHit;
  id: string;
  selected: boolean;
  onHover: () => void;
  onOpen: () => void;
}) {
  return (
    <li
      id={id}
      role="option"
      aria-selected={selected}
      onMouseMove={selected ? undefined : onHover}
      onClick={onOpen}
      className={cx("cursor-pointer px-4 py-3", selected ? "bg-neutral-800/70" : "hover:bg-neutral-900/50")}
    >
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 truncate text-sm text-neutral-200" title={h.session_title}>
          {h.session_title}
        </p>
        <Badge tone={h.scope === MAIN_SCOPE ? "info" : "neutral"}>
          {h.scope === MAIN_SCOPE ? "Chính" : `Phòng ban: ${h.scope}`}
        </Badge>
        {h.kind === "section" ? (
          <span className="font-mono text-[11px] text-neutral-500">v{h.version_no}</span>
        ) : (
          <Badge tone="neutral">Đề bài</Badge>
        )}
      </div>
      {h.kind === "section" && h.heading && (
        <p className="mt-0.5 truncate text-xs font-medium text-violet-300/90">§ {h.heading}</p>
      )}
      <p className="mt-1 line-clamp-2 text-xs text-neutral-400">
        <Snippet text={h.snippet} />
      </p>
      <p className="mt-0.5 text-[11px] text-neutral-600">{formatDate(h.created_at)}</p>
    </li>
  );
}

/** `[[khớp]]` → <mark>; mọi phần còn lại là text node (không bao giờ dùng HTML thô). */
function Snippet({ text }: { text: string }) {
  return (
    <>
      {splitSnippet(text).map((p, i) =>
        p.hit ? (
          <mark key={i} className="rounded-sm bg-amber-400/25 px-0.5 text-amber-100">
            {p.text}
          </mark>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}

function Row({ s }: { s: SessionSummary }) {
  const remove = useHistory((st) => st.remove);
  const liveId = useCouncil((st) => (st.live.status === "running" ? st.live.id : null));
  const openSession = useOpenSession();
  const [opening, setOpening] = useState(false);
  const [confirm, setConfirm] = useState(false);

  const open = async () => {
    setOpening(true);
    try {
      await openSession(s.id);
    } finally {
      setOpening(false);
    }
  };

  return (
    <li className="flex items-center gap-4 px-4 py-3 hover:bg-neutral-900/50">
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm text-neutral-200" title={s.prompt_preview}>
          {s.prompt_preview}
        </p>
        <p className="mt-0.5 text-xs text-neutral-500">
          {formatDate(s.created_at)} · {s.versions} version · {s.rounds} round
          {s.departments > 0 && ` · ${s.departments} phòng ban`}
          {s.total_tokens > 0 && ` · ${formatTokens(s.total_tokens)} token`}
        </p>
      </div>
      <StatusBadge status={s.status} />
      <div className="flex gap-1.5">
        <Button size="sm" icon={<Eye className="size-3.5" />} loading={opening} onClick={() => void open()}>
          Mở
        </Button>
        {confirm ? (
          <>
            <Button size="sm" variant="danger" onClick={() => void remove(s.id)}>
              Xác nhận xoá
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirm(false)}>
              Huỷ
            </Button>
          </>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            icon={<Trash className="size-3.5" />}
            disabled={s.id === liveId}
            title={s.id === liveId ? "Không xoá được phiên đang chạy" : "Xoá phiên"}
            onClick={() => setConfirm(true)}
            aria-label="Xoá phiên"
          />
        )}
      </div>
    </li>
  );
}
