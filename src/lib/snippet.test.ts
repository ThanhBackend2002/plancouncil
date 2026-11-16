import { describe, expect, it } from "vitest";
import { splitSnippet } from "./snippet";

describe("splitSnippet", () => {
  it("tách text và chỗ khớp", () => {
    expect(splitSnippet("…Đây là [[kết]] [[quả]] cuối…")).toEqual([
      { text: "…Đây là ", hit: false },
      { text: "kết", hit: true },
      { text: " ", hit: false },
      { text: "quả", hit: true },
      { text: " cuối…", hit: false },
    ]);
  });

  it("chỗ khớp ở đầu / cuối, không có phần rỗng", () => {
    expect(splitSnippet("[[Redis]]")).toEqual([{ text: "Redis", hit: true }]);
    expect(splitSnippet("[[a]][[b]]")).toEqual([
      { text: "a", hit: true },
      { text: "b", hit: true },
    ]);
  });

  it("không có chỗ khớp / chuỗi rỗng", () => {
    expect(splitSnippet("abc")).toEqual([{ text: "abc", hit: false }]);
    expect(splitSnippet("")).toEqual([]);
  });

  it("`[[` thiếu `]]` giữ nguyên là text; HTML không bị diễn giải", () => {
    expect(splitSnippet("x [[y")).toEqual([{ text: "x [[y", hit: false }]);
    expect(splitSnippet("a]] [[<b>]] c")).toEqual([
      { text: "a]] ", hit: false },
      { text: "<b>", hit: true },
      { text: " c", hit: false },
    ]);
  });
});
