import { create } from "zustand";
import type { SessionSummary } from "../lib/contracts";
import { ipc } from "../lib/ipc";
import { toastError } from "./toast";

interface HistoryStore {
  sessions: SessionSummary[];
  loading: boolean;
  loaded: boolean;
  refresh: () => Promise<void>;
  remove: (id: string) => Promise<void>;
}

export const useHistory = create<HistoryStore>((set, get) => ({
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
  },
  remove: async (id) => {
    try {
      await ipc.deleteSession(id);
      set({ sessions: get().sessions.filter((s) => s.id !== id) });
    } catch (e) {
      toastError(e, "Không xoá được phiên");
    }
  },
}));
