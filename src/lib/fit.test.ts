import { describe, expect, it } from "vitest";
import { fitScale, MIN_SCALE } from "./fit";

describe("fitScale", () => {
  it("leaves mail that fits alone", () => {
    expect(fitScale(380, 380)).toBe(1);
    expect(fitScale(380, 381)).toBe(1); // rounding slack
    expect(fitScale(0, 600)).toBe(1); // not laid out yet
  });

  it("shrinks a 600px newsletter to a phone's width, never past the floor", () => {
    expect(fitScale(380, 600)).toBeCloseTo(0.633, 3);
    expect(fitScale(380, 600) * 600).toBeLessThanOrEqual(380);
    expect(fitScale(380, 4000)).toBe(MIN_SCALE);
  });
});
