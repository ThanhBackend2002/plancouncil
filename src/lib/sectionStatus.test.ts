import { describe, expect, it } from "vitest";
import { changedNumbers, deriveSectionStatus, headingNumber, headingNumbers, numbersMatch, reviewMentions, type RoundLike } from "./sectionStatus";

const rv = (verdict: "APPROVE" | "REJECT", sections?: string[]) => ({ review: { verdict, sections } });
const round = (planVersion: number, converged: boolean, slots: RoundLike["slots"]): RoundLike => ({
  planVersion,
  converged,
  completed: true,
  slots,
});

describe("headingNumber", () => {
  it("lấy số đầu heading", () => {
    expect(headingNumber("3.3 API")).toBe("3.3");
    expect(headingNumber("2. Kiến trúc")).toBe("2");
    expect(headingNumber("§4 Rủi ro")).toBe("4");
    expect(headingNumber("Tổng quan")).toBeNull();
  });
});

describe("numbersMatch", () => {
  it("khớp bằng nhau hoặc tiền tố theo dấu chấm", () => {
    expect(numbersMatch("3.3", "3.3")).toBe(true);
    expect(numbersMatch("3", "3.3")).toBe(true);
    expect(numbersMatch("3.3", "3")).toBe(true);
    expect(numbersMatch("3", "30")).toBe(false);
    expect(numbersMatch("3.1", "3.3")).toBe(false);
    expect(numbersMatch("§3.3.", "3.3")).toBe(true);
    expect(numbersMatch("", "3")).toBe(false);
  });
  it("reviewMentions", () => {
    expect(reviewMentions({ sections: ["3.3"] }, "3")).toBe(true);
    expect(reviewMentions({ sections: [] }, "3")).toBe(false);
    expect(reviewMentions(null, "3")).toBe(false);
  });
});

describe("deriveSectionStatus", () => {
  it("REJECT nêu section → changes_requested với count; ## phủ ###", () => {
    const rounds = [round(1, false, { a: rv("REJECT", ["3.3"]), b: rv("REJECT", ["3"]), c: rv("APPROVE") })];
    const r = deriveSectionStatus(rounds, 1, ["3", "3.3", "3.1", "4"]);
    expect(r["3"]).toMatchObject({ status: "changes_requested", count: 2 });
    expect(r["3.3"]).toMatchObject({ status: "changes_requested", count: 2 });
    expect(r["3.1"]).toMatchObject({ status: "changes_requested", count: 1 });
    expect(r["4"]).toMatchObject({ status: "pending", count: 0 });
  });
  it("kết quả một phần không bao giờ thành approved", () => {
    const rounds = [round(1, false, { a: rv("APPROVE"), b: rv("REJECT", ["2"]) })];
    expect(deriveSectionStatus(rounds, 1, ["1"])["1"]!.status).toBe("pending");
  });
  it("hội tụ → approved cho mọi section", () => {
    const rounds = [round(2, true, { a: rv("APPROVE"), b: rv("APPROVE") })];
    const r = deriveSectionStatus(rounds, 2, ["1", "2.1"]);
    expect(r["1"]!.status).toBe("approved");
    expect(r["2.1"]!.status).toBe("approved");
  });
  it("REJECT không nêu section → pending (chưa xác nhận)", () => {
    const rounds = [round(1, false, { a: rv("REJECT") })];
    expect(deriveSectionStatus(rounds, 1, ["1"])["1"]).toMatchObject({ status: "pending", count: 0 });
  });
  it("version chưa có round → pending; chỉ xét round của đúng version", () => {
    const rounds = [round(1, true, { a: rv("APPROVE") })];
    expect(deriveSectionStatus(rounds, 2, ["1"])["1"]!.status).toBe("pending");
    expect(deriveSectionStatus([], 1, ["1"])["1"]!.status).toBe("pending");
  });
  it("round chưa hoàn tất không được coi là approved", () => {
    const r: RoundLike = { planVersion: 1, converged: true, completed: false, slots: {} };
    expect(deriveSectionStatus([r], 1, ["1"])["1"]!.status).toBe("pending");
  });
});

describe("changedNumbers / headingNumbers", () => {
  it("chỉ lấy section sửa/thêm", () => {
    const set = changedNumbers([
      { key: "a", titleA: "1 A", titleB: "1 A", a: "x", b: "y", status: "modified" },
      { key: "b", titleA: "2 B", titleB: "2 B", a: "x", b: "x", status: "unchanged" },
      { key: "c", titleA: null, titleB: "3 C", a: "", b: "z", status: "added" },
      { key: "d", titleA: "4 D", titleB: null, a: "x", b: "", status: "removed" },
    ]);
    expect([...set].sort()).toEqual(["1", "3"]);
  });
  it("headingNumbers bỏ qua code fence và heading không số", () => {
    const md = "## 1. A\n### 1.1 B\n```\n## 9 no\n```\n## Không số\n#### 1.1.1 deep";
    expect(headingNumbers(md)).toEqual(["1", "1.1"]);
  });
});
