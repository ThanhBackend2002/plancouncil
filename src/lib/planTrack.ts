// Trạng thái từng phiên bản plan cho thanh theo dõi (PlanTracker) — hàm thuần, không đọc text stream.

import type { RoleId } from "./contracts";
import type { FinalView, PlanView, RoundView } from "../store/session";

export type VersionState =
  /** Người viết đang viết phiên bản này. */
  | "writing"
  /** Viết dở rồi dừng (bị cắt / phiên kết thúc giữa chừng). */
  | "incomplete"
  /** Viết xong, chưa có vòng review nào. */
  | "waiting"
  | "reviewing"
  /** Có reviewer REJECT ở vòng review gần nhất của phiên bản này. */
  | "rejected"
  /** Mọi reviewer APPROVE (hội tụ). */
  | "approved"
  /** Không ai REJECT nhưng có reviewer lỗi / chưa đủ để hội tụ. */
  | "partial";

export type Vote = "approve" | "reject" | "abstain" | "error" | "pending" | "none";

export interface VersionTrack {
  version: number;
  state: VersionState;
  /** Vòng review gần nhất của phiên bản này (null = chưa review). */
  round: number | null;
  votes: { agent: RoleId; vote: Vote }[];
  approvals: number;
  rejects: number;
  /** Tổng số critical issue trong các review REJECT của vòng gần nhất. */
  issues: number;
  /** Vài issue đầu (để làm tooltip). */
  topIssues: string[];
  isFinal: boolean;
}

export function trackVersions(plans: readonly PlanView[], rounds: readonly RoundView[], final: FinalView | null, running: boolean): VersionTrack[] {
  const last = plans.at(-1)?.version;
  return plans.map((p) => {
    const round = rounds.filter((r) => r.planVersion === p.version).at(-1) ?? null;
    const votes: VersionTrack["votes"] = round
      ? Object.entries(round.slots).map(([agent, s]) => ({
          agent,
          vote: s.pending
            ? "pending"
            : s.review
              ? s.review.verdict === "APPROVE"
                ? "approve"
                : s.review.verdict === "REJECT"
                  ? "reject"
                  : "abstain"
              : s.error
                ? "error"
                : "none",
        }))
      : [];
    const rejectReviews = round ? Object.values(round.slots).flatMap((s) => (s.review?.verdict === "REJECT" ? [s.review] : [])) : [];
    const rejects = rejectReviews.length;
    const issues = rejectReviews.reduce((n, r) => n + Math.max(r.critical_issues.length, 1), 0);
    const topIssues = rejectReviews.flatMap((r) => r.critical_issues.slice(0, 2).map((i) => `${r.role}: ${i}`)).slice(0, 4);

    let state: VersionState;
    if (!p.complete) state = running && p.version === last ? "writing" : "incomplete";
    else if (!round) state = "waiting";
    else if (!round.completed) state = "reviewing";
    else if (rejects > 0) state = "rejected";
    else if (round.converged) state = "approved";
    else state = "partial";

    return {
      version: p.version,
      state,
      round: round?.round ?? null,
      votes,
      approvals: votes.filter((v) => v.vote === "approve").length,
      rejects,
      issues,
      topIssues,
      isFinal: !!final && final.version === p.version,
    };
  });
}
