import { create } from "zustand";
import type { CommandError, HistorySearchHit, SessionSummary } from "../lib/contracts";
import { ipc, toCommandError } from "../lib/ipc";
import { toastError } from "./toast";

/** Chờ người dùng ngừng gõ bao lâu (ms) rồi mới gọi `search_history`. */
export const SEARCH_DEBOUNCE_MS = 200;
export const SEARCH_LIMIT = 50;

interface HistoryStore {
  sessions: SessionSummary[];
  loading: boolean;
  loaded: boolean;
  refresh: () => Promise<void>;
  remove: (id: string) => Promise<void>;

  // ── Tìm kiếm ──
  query: string;
  hits: HistorySearchHit[];
  /** Câu tìm (đã trim) mà `hits` / `searchError` đang phản ánh; null = chưa có kết quả nào. */
  hitsFor: string | null;
  searchLoading: boolean;
  searchError: CommandError | null;
  /** Đổi câu tìm; gọi backend sau `SEARCH_DEBOUNCE_MS`, bỏ qua response cũ. */
  setQuery: (q: string) => void;
  clearSearch: () => void;
  /** Tăng mỗi lần yêu cầu focus ô tìm kiếm (Ctrl+K) — HistoryView theo dõi giá trị này. */
  focusNonce: number;
  focusSearch: () => void;
}

let timer: ReturnType<typeof setTimeout> | null = null;
/** Mỗi request được đánh số; chỉ response của request mới nhất được ghi vào store. */
let seq = 0;

function cancelPending() {
  if (timer !== null) clearTimeout(timer);
  timer = null;
  seq++; // vô hiệu hoá response đang bay
}

export const useHistory = create<HistoryStore>((set, get) => {
  const runSearch = async (q: string) => {
    const my = ++seq;
    set({ searchLoading: true });
    try {
      const hits = await ipc.searchHistory(q, SEARCH_LIMIT);
      if (my !== seq) return;
      set({ hits, hitsFor: q, searchError: null, searchLoading: false });
    } catch (e) {
      if (my !== seq) return;
      set({ hits: [], hitsFor: q, searchError: toCommandError(e), searchLoading: false });
    }
  };

  const schedule = (q: string, delay: number) => {
    cancelPending();
    const trimmed = q.trim();
    if (!trimmed) {
      set({ hits: [], hitsFor: null, searchError: null, searchLoading: false });
      return;
    }
    set({ searchLoading: true });
    timer = setTimeout(() => {
      timer = null;
      void runSearch(trimmed);
    }, delay);
  };

  return {
    sessions: [],
    loading: false,
    loaded: false,
    refresh: async () => {
      set({ loading: true });
      try {
        set({ sessions: await ipc.listSessions(200), loaded: true });
      } catch (e) {
        toastError(e, "Không tải được lịch sử");
      } finally {
        set({ loading: false });
      }
      // Phiên mới lưu / xoá → kết quả tìm cũ có thể lệch; tìm lại ngay.
      if (get().query.trim()) schedule(get().query, 0);
    },
    remove: async (id) => {
      try {
        await ipc.deleteSession(id);
        set((s) => ({
          sessions: s.sessions.filter((x) => x.id !== id),
          hits: s.hits.filter((h) => h.session_id !== id),
        }));
      } catch (e) {
        toastError(e, "Không xoá được phiên");
      }
    },

    query: "",
    hits: [],
    hitsFor: null,
    searchLoading: false,
    searchError: null,
    setQuery: (q) => {
      set({ query: q });
      schedule(q, SEARCH_DEBOUNCE_MS);
    },
    clearSearch: () => {
      set({ query: "" });
      schedule("", 0);
    },
    focusNonce: 0,
    focusSearch: () => set((s) => ({ focusNonce: s.focusNonce + 1 })),
  };
});
