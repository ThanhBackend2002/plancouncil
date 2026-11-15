import { create } from "zustand";

export type Tab = "council" | "history" | "settings";

interface UiStore {
  tab: Tab;
  setTab: (t: Tab) => void;
  /** Yêu cầu Plan Viewer mở 1 version (từ khung hội thoại). `nonce` để bấm lại cùng version vẫn kích hoạt. */
  planFocus: { version: number; nonce: number } | null;
  focusPlan: (version: number) => void;
}

export const useUi = create<UiStore>((set) => ({
  tab: "council",
  setTab: (tab) => set({ tab }),
  planFocus: null,
  focusPlan: (version) => set({ planFocus: { version, nonce: Date.now() } }),
}));
