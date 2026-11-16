import { describe, expect, it } from "vitest";
import { canStartHotkey, canStopHotkey, matchesCombo, parseCombo } from "./useHotkeys";

const ev = (key: string, mods: Partial<Record<"ctrlKey" | "metaKey" | "shiftKey" | "altKey", boolean>> = {}) => ({
  key,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

describe("hotkey", () => {
  it("parseCombo", () => {
    expect(parseCombo("Mod+Shift+K")).toEqual({ key: "k", mod: true, shift: true, alt: false });
  });

  it("mod+k khớp cả Ctrl lẫn Cmd, không khớp khi thiếu / thừa phím bổ trợ", () => {
    expect(matchesCombo(ev("k", { ctrlKey: true }), "mod+k")).toBe(true);
    expect(matchesCombo(ev("K", { metaKey: true }), "mod+k")).toBe(true);
    expect(matchesCombo(ev("k"), "mod+k")).toBe(false);
    expect(matchesCombo(ev("k", { ctrlKey: true, shiftKey: true }), "mod+k")).toBe(false);
    expect(matchesCombo(ev("j", { ctrlKey: true }), "mod+k")).toBe(false);
  });
});

describe("phím tắt Phòng họp", () => {
  const ctx = { tab: "council", viewing: false, status: "idle", stopping: false };

  it("mod+enter khớp Ctrl và Cmd, không khớp khi thêm Shift", () => {
    expect(matchesCombo(ev("Enter", { ctrlKey: true }), "mod+enter")).toBe(true);
    expect(matchesCombo(ev("Enter", { metaKey: true }), "mod+enter")).toBe(true);
    expect(matchesCombo(ev("Enter"), "mod+enter")).toBe(false);
    expect(matchesCombo(ev("Enter", { ctrlKey: true, shiftKey: true }), "mod+enter")).toBe(false);
  });

  it("escape khớp khi không có phím bổ trợ", () => {
    expect(matchesCombo(ev("Escape"), "escape")).toBe(true);
    expect(matchesCombo(ev("Escape", { ctrlKey: true }), "escape")).toBe(false);
  });

  it("chỉ bắt đầu họp ở tab họp, không xem phiên cũ, không đang chạy", () => {
    expect(canStartHotkey(ctx)).toBe(true);
    expect(canStartHotkey({ ...ctx, tab: "history" })).toBe(false);
    expect(canStartHotkey({ ...ctx, viewing: true })).toBe(false);
    expect(canStartHotkey({ ...ctx, status: "running" })).toBe(false);
  });

  it("Esc chỉ mở xác nhận dừng khi đang chạy, chưa dừng, không trong popup", () => {
    const running = { ...ctx, status: "running" };
    expect(canStopHotkey(running, false)).toBe(true);
    expect(canStopHotkey(running, true)).toBe(false);
    expect(canStopHotkey({ ...running, stopping: true }, false)).toBe(false);
    expect(canStopHotkey({ ...running, viewing: true }, false)).toBe(false);
    expect(canStopHotkey({ ...running, tab: "settings" }, false)).toBe(false);
    expect(canStopHotkey(ctx, false)).toBe(false);
  });
});
