import { describe, expect, it } from "vitest";
import { matchesCombo, parseCombo } from "./useHotkeys";

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
