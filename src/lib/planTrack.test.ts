import { describe, expect, it } from "vitest";
import type { Review } from "./contracts";
import { trackVersions } from "./planTrack";
import type { RoundView, SlotView } from "../store/session";

const review = (agent: string, verdict: Review["verdict"], issues: string[] = []): SlotView => ({
  review: { agent, role: agent.toUpperCase(), verdict, summary_bubble: "", critical_issues: issues, suggestions: [] },
  error: null,
  pending: false,
});
const round = (n: number, v: number, slots: Record<string, SlotView>, done = true, converged = false): RoundView => ({
  round: n,
  planVersion: v,
  slots,
  approvals: Object.values(slots).filter((s) => s.review?.verdict === "APPROVE").length,
  total: Object.keys(slots).length,
  converged,
  completed: done,
});
const plan = (version: number, complete = true) => ({ version, markdown: "#", complete });

describe("trackVersions", () => {
  it("phân loại từng phiên bản theo vòng review gần nhất của nó", () => {
    const rounds = [
      round(1, 1, { a: review("a", "REJECT", ["thiếu index", "thiếu auth"]), b: review("b", "APPROVE") }),
      round(2, 2, { a: review("a", "APPROVE"), b: review("b", "APPROVE") }, true, true),
    ];
    const t = trackVersions([plan(1), plan(2)], rounds, { version: 2, markdown: "#", status: "converged" }, false);
    expect(t.map((x) => x.state)).toEqual(["rejected", "approved"]);
    expect(t[0]).toMatchObject({ rejects: 1, issues: 2, approvals: 1, round: 1, isFinal: false });
    expect(t[0]?.topIssues).toEqual(["A: thiếu index", "A: thiếu auth"]);
    expect(t[1]).toMatchObject({ isFinal: true, approvals: 2 });
  });

  it("đang viết / viết dở / chờ review / đang review", () => {
    const pending: SlotView = { review: null, error: null, pending: true };
    expect(trackVersions([plan(1, false)], [], null, true)[0]?.state).toBe("writing");
    expect(trackVersions([plan(1, false)], [], null, false)[0]?.state).toBe("incomplete");
    expect(trackVersions([plan(1)], [], null, true)[0]?.state).toBe("waiting");
    const t = trackVersions([plan(1)], [round(1, 1, { a: review("a", "APPROVE"), b: pending }, false)], null, true)[0];
    expect(t?.state).toBe("reviewing");
    expect(t?.votes.map((v) => v.vote)).toEqual(["approve", "pending"]);
  });

  it("không ai REJECT nhưng có reviewer lỗi → duyệt một phần", () => {
    const err: SlotView = { review: null, error: "timeout", pending: false };
    const t = trackVersions([plan(1)], [round(1, 1, { a: review("a", "APPROVE"), b: err })], null, false)[0];
    expect(t?.state).toBe("partial");
    expect(t?.votes.map((v) => v.vote)).toEqual(["approve", "error"]);
  });

  it("REJECT không có critical issue vẫn tính 1 vấn đề", () => {
    const t = trackVersions([plan(1)], [round(1, 1, { a: review("a", "REJECT") })], null, false)[0];
    expect(t?.issues).toBe(1);
  });
});
