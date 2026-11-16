import { describe, expect, it } from "vitest";
import type { EventEnvelope } from "../lib/contracts";
import { applyEvent, initialSession } from "./session";

const SID = "s1";
const env = (seq: number, e: object): EventEnvelope => ({ session_id: SID, seq, ts: "2026-01-01T00:00:00Z", ...e }) as EventEnvelope;

describe("truncated / budget events", () => {
  it("lưu ảnh chụp ngân sách mới nhất và thêm dòng thông báo khi bị cắt", () => {
    let s = applyEvent(
      initialSession(),
      env(0, {
        type: "session_started",
        prompt: "p",
        max_rounds: 3,
        chair_id: "cto",
        reviewer_ids: ["dba"],
        cast: [],
        departments: [],
        options: { workspace: null, web_search: false, company_context: false },
      }),
    );
    s = applyEvent(s, env(1, { type: "agent_thinking", scope: "main", agent: "cto", round: 1, plan_version: 1 }));
    s = applyEvent(s, env(2, { type: "truncated", scope: "main", agent: "cto", call_kind: "plan", recovered: true, reason: "length" }));
    s = applyEvent(s, env(3, { type: "budget", used: 900, reserved: 0, limit: 1000, estimated: true, soft: true, state: "warn" }));

    expect(s.budget).toEqual({ used: 900, reserved: 0, limit: 1000, estimated: true, soft: true, state: "warn" });
    const notices = s.scopes.main!.notices!;
    expect(notices).toHaveLength(1);
    expect(notices[0]!.planVersion).toBe(1);
    expect(notices[0]!.text).toContain("đã viết tiếp");
  });
});

describe("revise_fallback / consistency events", () => {
  it("thêm dòng thông báo vào hội thoại của người viết plan", () => {
    let s = applyEvent(
      initialSession(),
      env(0, {
        type: "session_started",
        prompt: "p",
        max_rounds: 3,
        chair_id: "cto",
        reviewer_ids: ["dba"],
        cast: [],
        departments: [],
        options: { workspace: null, web_search: false, company_context: false },
      }),
    );
    s = applyEvent(s, env(1, { type: "plan_ready", scope: "main", version: 2, markdown: "## 1. A\n" }));
    s = applyEvent(s, env(2, { type: "revise_fallback", scope: "main", round: 2, reason: "missing_ids", consecutive: 2 }));
    s = applyEvent(
      s,
      env(3, { type: "consistency", scope: "main", result: "conflicts", conflicts: [{ sections: ["s2", "s5"], text: "TTL lệch" }] }),
    );
    const notices = s.scopes.main!.notices!;
    expect(notices).toHaveLength(2);
    expect(notices[0]!.agentId).toBe("cto");
    expect(notices[0]!.text).toContain("thiếu section bắt buộc");
    expect(notices[0]!.text).toContain("2 lần liên tiếp");
    expect(notices[1]!.text).toContain("[s2, s5] TTL lệch");
  });
});
