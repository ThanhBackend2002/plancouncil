import { create } from "zustand";

export type Tab = "council" | "history" | "settings";
export type ThemePref = "system" | "dark" | "light";
export type MotionPref = "system" | "on" | "off";
/** Cảnh bên trái: 1 phòng họp (nhìn thẳng) hay cả toà nhà (mọi nơi họp + sảnh). */
export type RoomView = "room" | "building";
/**
 * Bố cục màn Phòng họp: "stage" = phòng họp lớn ở giữa, plan theo dõi bằng thẻ version và mở trong
 * popup khi cần; "split" = bố cục cũ (plan luôn hiện bên phải).
 */
export type LayoutPref = "stage" | "split";

interface UiStore {
  tab: Tab;
  setTab: (t: Tab) => void;
  /** Yêu cầu Plan Viewer mở 1 version (từ khung hội thoại). `nonce` để bấm lại cùng version vẫn kích hoạt. */
  planFocus: { version: number; nonce: number } | null;
  /** Chọn version để xem. Bố cục "stage": đồng thời mở popup plan. */
  focusPlan: (version: number) => void;
  /** Popup plan (bố cục "stage"). */
  planOpen: boolean;
  closePlan: () => void;
  layout: LayoutPref;
  setLayout: (l: LayoutPref) => void;
  /** Yêu cầu Cài đặt mở 1 tab cụ thể (vd. "providers" từ màn onboarding). `nonce` như planFocus. */
  settingsFocus: { tab: string; nonce: number } | null;
  openSettings: (tab?: string) => void;
  /** Lọc phản hồi trong Timeline theo số hiệu section (vd "3.3"); null = không lọc. */
  /** Hộp thoại xác nhận dừng họp (mở bằng Esc). */
  stopConfirmOpen: boolean;
  setStopConfirmOpen: (v: boolean) => void;
  sectionFilter: string | null;
  setSectionFilter: (n: string | null) => void;
  /** Lưu localStorage. */
  roomView: RoomView;
  setRoomView: (v: RoomView) => void;

  /** Theme người dùng chọn ("system" = theo hệ điều hành). Lưu localStorage. */
  theme: ThemePref;
  setTheme: (t: ThemePref) => void;
  /** Giảm chuyển động: "on" = luôn giảm, "off" = không giảm, "system" = theo hệ điều hành. */
  reducedMotion: MotionPref;
  setReducedMotion: (m: MotionPref) => void;
  /** Theme đang áp dụng thật (đã giải "system"). */
  resolvedTheme: "dark" | "light";
  /** Có đang giảm chuyển động không (đã giải "system"). */
  motionReduced: boolean;
}

const THEME_KEY = "plancouncil.theme";
const MOTION_KEY = "plancouncil.reducedMotion";
const ROOM_VIEW_KEY = "plancouncil.roomView";
const LAYOUT_KEY = "plancouncil.layout";

function readPref<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v && (allowed as readonly string[]).includes(v) ? (v as T) : fallback;
  } catch {
    return fallback;
  }
}

function writePref(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // localStorage bị chặn (private mode…) → chỉ giữ trong phiên.
  }
}

function media(query: string): MediaQueryList | null {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" ? window.matchMedia(query) : null;
}

const darkQuery = media("(prefers-color-scheme: dark)");
const motionQuery = media("(prefers-reduced-motion: reduce)");

function resolveTheme(t: ThemePref): "dark" | "light" {
  if (t !== "system") return t;
  // Không đọc được hệ điều hành → giữ dark như trước đây.
  return darkQuery && !darkQuery.matches ? "light" : "dark";
}

function resolveMotion(m: MotionPref): boolean {
  return m === "on" || (m === "system" && !!motionQuery?.matches);
}

const initialTheme = readPref<ThemePref>(THEME_KEY, ["system", "dark", "light"], "system");
const initialMotion = readPref<MotionPref>(MOTION_KEY, ["system", "on", "off"], "system");

export const useUi = create<UiStore>((set) => ({
  tab: "council",
  setTab: (tab) => set({ tab }),
  planFocus: null,
  focusPlan: (version) => set((s) => ({ planFocus: { version, nonce: Date.now() }, planOpen: s.layout === "stage" || s.planOpen })),
  planOpen: false,
  closePlan: () => set({ planOpen: false }),
  layout: readPref<LayoutPref>(LAYOUT_KEY, ["stage", "split"], "stage"),
  setLayout: (layout) => {
    writePref(LAYOUT_KEY, layout);
    set({ layout, planOpen: false });
  },
  settingsFocus: null,
  stopConfirmOpen: false,
  setStopConfirmOpen: (stopConfirmOpen) => set({ stopConfirmOpen }),
  sectionFilter: null,
  setSectionFilter: (sectionFilter) => set({ sectionFilter }),
  roomView: readPref<RoomView>(ROOM_VIEW_KEY, ["room", "building"], "room"),
  setRoomView: (roomView) => {
    writePref(ROOM_VIEW_KEY, roomView);
    set({ roomView });
  },
  openSettings: (tab) => set({ tab: "settings", settingsFocus: tab ? { tab, nonce: Date.now() } : null }),

  theme: initialTheme,
  setTheme: (theme) => {
    writePref(THEME_KEY, theme);
    set({ theme, resolvedTheme: resolveTheme(theme) });
  },
  reducedMotion: initialMotion,
  setReducedMotion: (reducedMotion) => {
    writePref(MOTION_KEY, reducedMotion);
    set({ reducedMotion, motionReduced: resolveMotion(reducedMotion) });
  },
  resolvedTheme: resolveTheme(initialTheme),
  motionReduced: resolveMotion(initialMotion),
}));

/** Gắn data-theme / data-motion lên <html> (index.css đọc 2 thuộc tính này). */
function applyToDocument(s: Pick<UiStore, "resolvedTheme" | "motionReduced">) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.dataset.theme = s.resolvedTheme;
  root.dataset.motion = s.motionReduced ? "reduce" : "full";
  // index.html gắn sẵn class "dark"; giữ đồng bộ cho thư viện nào đọc class này.
  root.classList.toggle("dark", s.resolvedTheme === "dark");
}

// Áp dụng ngay khi nạp module (trước lần render đầu) để không nháy theme.
applyToDocument(useUi.getState());
useUi.subscribe((s, prev) => {
  if (s.resolvedTheme !== prev.resolvedTheme || s.motionReduced !== prev.motionReduced) applyToDocument(s);
});
// Chế độ "system": đổi theo hệ điều hành ngay khi người dùng đổi cài đặt OS.
darkQuery?.addEventListener("change", () => {
  const { theme } = useUi.getState();
  useUi.setState({ resolvedTheme: resolveTheme(theme) });
});
motionQuery?.addEventListener("change", () => {
  const { reducedMotion } = useUi.getState();
  useUi.setState({ motionReduced: resolveMotion(reducedMotion) });
});
