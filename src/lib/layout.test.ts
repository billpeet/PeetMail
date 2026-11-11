import { describe, expect, it } from "vitest";
import { isCompact } from "./layout.svelte";

describe("layout", () => {
  it("is compact on a phone, and on a narrow desktop window", () => {
    expect(isCompact("android", false)).toBe(true);
    expect(isCompact("windows", true)).toBe(true);
    expect(isCompact("windows", false)).toBe(false);
    expect(isCompact("linux", false)).toBe(false);
  });
});
