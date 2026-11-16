import { describe, expect, it } from "vitest";
import { dayPhaseAt } from "./scene";

describe("dayPhaseAt", () => {
  it("chia ngày / chạng vạng / đêm theo giờ máy", () => {
    expect([5, 6, 12, 16].map(dayPhaseAt)).toEqual(["night", "day", "day", "day"]);
    expect([17, 18].map(dayPhaseAt)).toEqual(["dusk", "dusk"]);
    expect([19, 23, 0].map(dayPhaseAt)).toEqual(["night", "night", "night"]);
  });
});
