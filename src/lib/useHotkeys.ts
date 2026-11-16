// Phím tắt toàn cục. Thêm phím mới: bổ sung 1 phần tử vào `useAppHotkeys` (hoặc gọi `useHotkeys`
// ở component cần phím tắt riêng).

import { useEffect, useRef } from "react";
import { useHistory } from "../store/history";
import { useUi } from "../store/ui";

export interface Hotkey {
  /** Vd "mod+k", "shift+?", "escape". `mod` = Cmd trên macOS, Ctrl ở nơi khác (nhận cả hai). */
  combo: string;
  handler: (e: KeyboardEvent) => void;
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
      e.preventDefault();
      hk.handler(e);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
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
  ]);
}
