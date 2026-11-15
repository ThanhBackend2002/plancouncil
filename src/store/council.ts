import { create } from "zustand";
import { MAIN_SCOPE, type EventEnvelope, type SessionOptions, type SessionRecord } from "../lib/contracts";
import { ipc } from "../lib/ipc";
import { applyEvent, fromRecord, initialSession, isTerminal, type SessionState } from "./session";
import { toast, toastError } from "./toast";

/** "auto" = tự chọn scope đáng xem (phòng ban đang họp → cuộc họp chính khi tới lượt). */
export type ScopeSelection = "auto" | string;

interface CouncilStore {
  /** Phiên live (đang chạy hoặc vừa kết thúc). */
  live: SessionState;
  /** Phiên cũ mở từ History ở chế độ chỉ xem; null = đang xem live. */
  viewing: SessionState | null;
  scope: ScopeSelection;
  /** Phòng ban sẽ tham gia phiên kế tiếp; null = theo cấu hình mặc định. */
  nextDepartments: string[] | null;
  /** Workspace / web / bối cảnh công ty cho phiên kế tiếp; null = theo mặc định cấu hình. */
  nextOptions: SessionOptions | null;
  starting: boolean;
  stopping: boolean;
  ingest: (env: EventEnvelope) => void;
  start: (prompt: string) => Promise<boolean>;
  stop: () => Promise<void>;
  restore: () => Promise<void>;
  openRecord: (rec: SessionRecord) => void;
  closeView: () => void;
  hydrateLive: (rec: SessionRecord) => void;
  setScope: (s: ScopeSelection) => void;
  setNextDepartments: (ids: string[] | null) => void;
  setNextOptions: (o: SessionOptions | null) => void;
}

export const useCouncil = create<CouncilStore>((set, get) => ({
  live: initialSession(),
  viewing: null,
  scope: "auto",
  nextDepartments: null,
  nextOptions: null,
  starting: false,
  stopping: false,

  ingest: (env) => {
    const prev = get().live;
    const next = applyEvent(prev, env);
    if (next === prev) {
      if (import.meta.env.DEV) console.debug("[council] bỏ qua event", env.type, env.seq, env.session_id);
      return;
    }
    if (next.seqGap && !prev.seqGap) console.warn(`[council] seq bị nhảy: ${prev.lastSeq} → ${env.seq}`);
    set({ live: next, stopping: isTerminal(next.status) ? false : get().stopping });
  },

  start: async (prompt) => {
    if (get().live.status === "running" || get().starting) return false;
    set({ starting: true, live: initialSession(), viewing: null, scope: "auto" });
    try {
      const id = await ipc.startCouncil(prompt, get().nextDepartments ?? undefined, get().nextOptions ?? undefined);
      const cur = get().live;
      if (cur.id === null) set({ live: { ...cur, id, status: "running", prompt } });
      return true;
    } catch (e) {
      toastError(e);
      return false;
    } finally {
      set({ starting: false });
    }
  },

  stop: async () => {
    const { live } = get();
    if (!live.id || live.status !== "running") return;
    set({ stopping: true });
    try {
      const ok = await ipc.stopCouncil(live.id);
      if (!ok) {
        set({ stopping: false });
        toast("info", "Phiên đã kết thúc trước khi dừng");
      }
    } catch (e) {
      set({ stopping: false });
      toastError(e);
    }
  },

  restore: async () => {
    try {
      const id = await ipc.activeSession();
      if (id && get().live.id === null) {
        set({ live: { ...initialSession(), id, status: "running", resumed: true } });
        toast(
          "info",
          "Đã kết nối lại phiên đang chạy",
          "Nội dung trước khi reload sẽ được nạp lại từ lịch sử khi phiên kết thúc.",
        );
      }
    } catch (e) {
      if (import.meta.env.DEV) console.debug("[council] active_session lỗi", e);
    }
  },

  openRecord: (rec) => set({ viewing: fromRecord(rec, true), scope: MAIN_SCOPE }),
  closeView: () => set({ viewing: null, scope: "auto" }),
  hydrateLive: (rec) => {
    if (get().live.id !== rec.id) return;
    set({ live: fromRecord(rec, false) });
  },
  setScope: (scope) => set({ scope }),
  setNextDepartments: (ids) => set({ nextDepartments: ids }),
  setNextOptions: (o) => set({ nextOptions: o }),
}));

export const useActiveSession = () => useCouncil((s) => s.viewing ?? s.live);

/** Scope thực sự hiển thị. "auto": cuộc họp chính nếu đã bắt đầu/kết thúc, ngược lại phòng ban đầu tiên còn đang họp. */
export function resolveScope(s: SessionState, sel: ScopeSelection): string {
  if (sel !== "auto" && s.scopes[sel]) return sel;
  const main = s.scopes[MAIN_SCOPE];
  const depts = s.scopeOrder.filter((id) => id !== MAIN_SCOPE);
  if (!main || main.plans.length > 0 || depts.length === 0 || s.status !== "running") return MAIN_SCOPE;
  return depts.find((id) => s.scopes[id]?.status === "running") ?? MAIN_SCOPE;
}
