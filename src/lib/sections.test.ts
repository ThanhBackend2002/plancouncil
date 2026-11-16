import { describe, expect, it } from "vitest";
import vectors from "../../src-tauri/tests-data/sections_vectors.json";
import { closesFence, fenceOpen, pairSections, sectionKey, splitSections } from "./sections";

const md = (...lines: string[]) => lines.join("\n");

describe("sectionKey", () => {
  it("bỏ phần trong ngoặc, gộp ký tự đặc biệt, chữ thường", () => {
    expect(sectionKey("Changelog (v1 → v2)")).toBe("changelog");
    expect(sectionKey("1. Overview")).toBe("1 overview");
    expect(sectionKey("Tech Stack & Schema")).toBe("tech stack schema");
  });

  it("giữ chữ Unicode (tiếng Việt)", () => {
    expect(sectionKey("Tổng Quan")).toBe("tổng quan");
  });
});

describe("splitSections", () => {
  it("rỗng → không có section", () => {
    expect(splitSections("")).toEqual([]);
    expect(splitSections("\n\n  \n")).toEqual([]);
  });

  it("tách theo ## và gom phần mở đầu vào __intro", () => {
    const s = splitSections(md("# Plan", "intro text", "", "## A", "body a", "## B", "body b"));
    expect(s).toEqual([
      { key: "__intro", title: "(Mở đầu)", body: "# Plan\nintro text" },
      { key: "a", title: "A", body: "body a" },
      { key: "b", title: "B", body: "body b" },
    ]);
  });

  it("không tạo intro khi plan bắt đầu bằng heading", () => {
    const s = splitSections(md("## A", "x"));
    expect(s.map((x) => x.key)).toEqual(["a"]);
  });

  it("chỉ ## mới là ranh giới; ### và ##không-cách thì không", () => {
    const s = splitSections(md("## A", "### Sub", "##NoSpace", "text"));
    expect(s).toHaveLength(1);
    expect(s[0]!.body).toBe("### Sub\n##NoSpace\ntext");
  });

  it("bỏ dấu # đóng và khoảng trắng cuối heading", () => {
    const s = splitSections("## Title ##   \nbody");
    expect(s[0]).toMatchObject({ key: "title", title: "Title" });
  });

  it("cắt dòng trống đầu và khoảng trắng cuối body, giữ heading rỗng body", () => {
    const s = splitSections(md("## A", "", "", "  line  ", "", "## B"));
    expect(s[0]!.body).toBe("  line");
    expect(s[1]).toEqual({ key: "b", title: "B", body: "" });
  });

  it("bỏ qua heading nằm trong code fence ```", () => {
    const s = splitSections(md("## A", "```markdown", "## Not a heading", "```", "after", "## B", "b"));
    expect(s.map((x) => x.title)).toEqual(["A", "B"]);
    expect(s[0]!.body).toBe("```markdown\n## Not a heading\n```\nafter");
  });

  it("bỏ qua heading nằm trong fence ~~~ và fence thụt ≤ 3 dấu cách", () => {
    const s = splitSections(md("## A", "~~~", "## X", "~~~", "   ```", "## Y", "   ```", "## B"));
    expect(s.map((x) => x.title)).toEqual(["A", "B"]);
  });

  it("fence chưa đóng nuốt hết heading phía sau", () => {
    const s = splitSections(md("## A", "```", "## B", "## C"));
    expect(s.map((x) => x.title)).toEqual(["A"]);
  });

  it("heading trùng được thêm hậu tố #n theo thứ tự xuất hiện", () => {
    const s = splitSections(md("## Notes", "1", "## Other", "## Notes", "2", "## notes!", "3"));
    expect(s.map((x) => x.key)).toEqual(["notes", "other", "notes#2", "notes#3"]);
    expect(s.map((x) => x.body)).toEqual(["1", "", "2", "3"]);
  });
});

describe("pairSections", () => {
  const v1 = md("## 1. Overview", "Plan v1", "## 2. Architecture", "A-->B", "## 3. Data Flow", "flow");

  it("giống hệt → tất cả unchanged", () => {
    const p = pairSections(v1, v1);
    expect(p.map((x) => x.status)).toEqual(["unchanged", "unchanged", "unchanged"]);
    expect(p[0]).toEqual({
      key: "1 overview",
      titleA: "1. Overview",
      titleB: "1. Overview",
      a: "Plan v1",
      b: "Plan v1",
      status: "unchanged",
    });
  });

  it("body khác → modified; title khác nhưng cùng key → modified", () => {
    const v2 = md("## 1. Overview", "Plan v2", "## 2. Architecture (revised)", "A-->B", "## 3. Data Flow", "flow");
    const p = pairSections(v1, v2);
    expect(p.map((x) => [x.key, x.status])).toEqual([
      ["1 overview", "modified"],
      ["2 architecture", "modified"],
      ["3 data flow", "unchanged"],
    ]);
    expect(p[1]).toMatchObject({ titleA: "2. Architecture", titleB: "2. Architecture (revised)" });
  });

  it("section mới ở B → added; section mất ở A → removed, chèn ngay sau section đứng trước nó ở A", () => {
    const v2 = md("## 1. Overview", "Plan v1", "## 3. Data Flow", "flow", "## 4. Risks", "r");
    const p = pairSections(v1, v2);
    expect(p.map((x) => [x.key, x.status])).toEqual([
      ["1 overview", "unchanged"],
      ["2 architecture", "removed"],
      ["3 data flow", "unchanged"],
      ["4 risks", "added"],
    ]);
    const removed = p[1]!;
    expect(removed).toMatchObject({ titleA: "2. Architecture", titleB: null, a: "A-->B", b: "" });
    const added = p[3]!;
    expect(added).toMatchObject({ titleA: null, titleB: "4. Risks", a: "", b: "r" });
  });

  it("section đầu tiên bị xoá → nằm ở đầu danh sách", () => {
    const p = pairSections(md("## Gone", "g", "## Keep", "k"), md("## Keep", "k"));
    expect(p.map((x) => [x.key, x.status])).toEqual([
      ["gone", "removed"],
      ["keep", "unchanged"],
    ]);
  });

  it("giữ thứ tự của B khi section bị đảo chỗ", () => {
    const p = pairSections(md("## A", "a", "## B", "b"), md("## B", "b", "## A", "a"));
    expect(p.map((x) => x.key)).toEqual(["b", "a"]);
    expect(p.every((x) => x.status === "unchanged")).toBe(true);
  });

  it("heading trùng được ghép theo thứ tự xuất hiện", () => {
    const a = md("## Notes", "first", "## Notes", "second");
    const b = md("## Notes", "first", "## Notes", "second changed", "## Notes", "third");
    const p = pairSections(a, b);
    expect(p.map((x) => [x.key, x.status])).toEqual([
      ["notes", "unchanged"],
      ["notes#2", "modified"],
      ["notes#3", "added"],
    ]);
  });

  it("heading trong code fence không sinh section giả khi so sánh", () => {
    const a = md("## A", "```", "## Fake", "```");
    const b = md("## A", "```", "## Fake", "## Still fake", "```");
    const p = pairSections(a, b);
    expect(p).toHaveLength(1);
    expect(p[0]!.status).toBe("modified");
  });

  it("phần mở đầu được ghép như một section riêng", () => {
    const p = pairSections("intro\n## A\nx", "## A\nx");
    expect(p.map((x) => [x.key, x.status])).toEqual([
      ["__intro", "removed"],
      ["a", "unchanged"],
    ]);
  });
});

describe("fence theo CommonMark", () => {
  it("``` bên trong ~~~ không đóng fence", () => {
    const s = splitSections(md("## A", "~~~", "```", "## X", "~~~", "## B"));
    expect(s.map((x) => x.title)).toEqual(["A", "B"]);
  });

  it("fence đóng phải cùng ký tự, đủ dài, không có info, thụt ≤ 3", () => {
    expect(closesFence("````", { ch: "`", len: 3 })).toBe(true);
    expect(closesFence("``", { ch: "`", len: 3 })).toBe(false);
    expect(closesFence("~~~", { ch: "`", len: 3 })).toBe(false);
    expect(closesFence("```rust", { ch: "`", len: 3 })).toBe(false);
    expect(closesFence("   ```  ", { ch: "`", len: 3 })).toBe(true);
    expect(closesFence("    ```", { ch: "`", len: 3 })).toBe(false);
  });

  it("fence mở: thụt ≤ 3; info của fence ` không chứa `", () => {
    expect(fenceOpen("   ~~~~ md")).toEqual({ ch: "~", len: 4 });
    expect(fenceOpen("    ```")).toBeNull();
    expect(fenceOpen("``` a`b")).toBeNull();
    expect(fenceOpen("~~~ a`b")).toEqual({ ch: "~", len: 3 });
  });
});

interface Vector {
  name: string;
  md: string;
  expected: { level: number; number: string | null; heading: string }[];
}

// Vector dùng chung với Rust (`src-tauri/src/sections.rs`). Splitter TS chỉ tách ở `##` nên chỉ
// so heading cấp 2 (`###` của Rust là section con, TS để nguyên trong body). Bỏ qua vector có
// heading `##` rỗng: TS dùng title rỗng làm dấu hiệu phần mở đầu (`__intro`), nên `##` trống
// không thành section riêng — khác biệt có chủ ý, không phải lỗi.
describe("vector dùng chung với Rust", () => {
  const all = vectors as Vector[];
  const comparable = all.filter((v) => !v.expected.some((e) => e.level === 2 && e.heading === ""));

  it("chỉ loại đúng vector có heading `##` rỗng", () => {
    expect(all.filter((v) => !comparable.includes(v)).map((v) => v.name)).toEqual(["number_variants"]);
  });

  it.each(comparable.map((v) => [v.name, v] as const))("%s", (_, v) => {
    const got = splitSections(v.md)
      .filter((s) => s.key !== "__intro")
      .map((s) => s.title);
    const want = v.expected.filter((e) => e.level === 2).map((e) => e.heading);
    expect(got).toEqual(want);
  });
});
