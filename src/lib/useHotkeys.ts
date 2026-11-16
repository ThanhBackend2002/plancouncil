// Phím tắt toàn cục. Thêm phím mới: bổ sung 1 phần tử vào `useAppHotkeys` (hoặc gọi `useHotkeys`
// ở component cần phím tắt riêng).

import { useEffect, useRef } from "react";
import { useCouncil } from "../store/council";
import { useHistory } from "../store/history";
import { useUi } from "../store/ui";

export interface Hotkey {
  /** Vd "mod+k", "shift+?", "escape". `mod` = Cmd trên macOS, Ctrl ở nơi khác (nhận cả hai). */
  combo: string;
  /** Trả `false` = không xử lý (không preventDefault, phím đi tiếp cho phần tử khác). */
  handler: (e: KeyboardEvent) => void | false;
}

interface Combo {
  key: string;
  mod: boolean;
  shift: boolean;
  alt: boolean;
}

export function parseCombo(combo: string): Combo {
  const parts = combo.toLowerCase().split("+").map((p) => p.trim());
  const key = parts.pop() ?? "";
  return { key, mod: parts.includes("mod"), shift: parts.includes("shift"), alt: parts.includes("alt") };
}

export function matchesCombo(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">, combo: string): boolean {
  const c = parseCombo(combo);
  return (
    e.key.toLowerCase() === c.key &&
    (e.ctrlKey || e.metaKey) === c.mod &&
    e.shiftKey === c.shift &&
    e.altKey === c.alt
  );
}

/** Lắng nghe keydown trên window. Phím khớp đầu tiên được gọi và `preventDefault`. */
export function useHotkeys(hotkeys: Hotkey[]) {
  const ref = useRef(hotkeys);
  useEffect(() => {
    ref.current = hotkeys;
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing || e.defaultPrevented) return;
      const hk = ref.current.find((h) => matchesCombo(e, h.combo));
      if (!hk) return;
      if (hk.handler(e) !== false) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

// ── Phím tắt tab Phòng họp ──
// Ctrl/⌘+Enter = bắt đầu họp từ bất kỳ đâu · Esc = dừng phiên đang chạy (có hộp thoại xác nhận).

export interface CouncilKeyContext {
  tab: string;
  /** Đang xem phiên cũ (chỉ xem). */
  viewing: boolean;
  /** Trạng thái phiên live. */
  status: string;
  stopping: boolean;
}

export const canStartHotkey = (c: CouncilKeyContext): boolean => c.tab === "council" && !c.viewing && c.status !== "running";

/** `inPopup`: phím phát ra từ hộp thoại / dropdown → để chúng tự xử lý Esc (đóng), không dừng họp. */
export const canStopHotkey = (c: CouncilKeyContext, inPopup: boolean): boolean =>
  c.tab === "council" && !c.viewing && c.status === "running" && !c.stopping && !inPopup;

const POPUP = '[role="dialog"],[role="listbox"],[role="combobox"],[aria-expanded="true"]';

let startHandler: (() => void) | null = null;
/** PromptPanel đăng ký hàm bắt đầu (phụ thuộc state nhập liệu). Trả hàm huỷ đăng ký. */
export function registerStartHandler(fn: () => void): () => void {
  startHandler = fn;
  return () => {
    if (startHandler === fn) startHandler = null;
  };
}

function councilContext(): CouncilKeyContext {
  const st = useCouncil.getState();
  return { tab: useUi.getState().tab, viewing: st.viewing !== null, status: st.live.status, stopping: st.stopping };
}

/** Phím tắt cấp app — gọi 1 lần trong App. */
export function useAppHotkeys() {
  useHotkeys([
    {
      combo: "mod+k",
      handler: () => {
        useUi.getState().setTab("history");
        useHistory.getState().focusSearch();
      },
    },
    {
      combo: "mod+enter",
      handler: () => {
        if (!canStartHotkey(councilContext()) || !startHandler) return false;
        startHandler();
      },
    },
    {
      combo: "escape",
      handler: (e) => {
        const t = e.target instanceof Element ? e.target : null;
        if (!canStopHotkey(councilContext(), !!t?.closest(POPUP))) return false;
        useUi.getState().setStopConfirmOpen(true);
      },
    },
  ]);
}
