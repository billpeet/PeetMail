import { describe, expect, it } from "vitest";
import { dragAxis, pageSwipe, pullDistance, PULL_TRIGGER_PX, rowSwipe, rowThreshold } from "./gestures";

describe("gestures", () => {
  it("decides a drag's axis only once it has clearly moved", () => {
    expect(dragAxis(4, 3)).toBeNull();
    expect(dragAxis(30, 5)).toBe("x");
    expect(dragAxis(5, 30)).toBe("y");
    // Diagonal leans to scrolling: a list mustn't archive when you meant to scroll.
    expect(dragAxis(20, 18)).toBe("y");
  });

  it("acts on a row dragged far enough: right archives, left snoozes", () => {
    expect(rowThreshold(412)).toBe(120);
    expect(rowThreshold(300)).toBe(90);
    expect(rowSwipe(130, 412)).toBe("archive");
    expect(rowSwipe(-130, 412)).toBe("snooze");
    expect(rowSwipe(60, 412)).toBeNull();
  });

  it("flips conversations on a quick, mostly sideways flick", () => {
    expect(pageSwipe(-150, 10, 250)).toBe("next");
    expect(pageSwipe(150, 10, 250)).toBe("prev");
    expect(pageSwipe(-150, 120, 250)).toBeNull(); // scrolling, a bit sideways
    expect(pageSwipe(-50, 0, 200)).toBeNull(); // too short
    expect(pageSwipe(-150, 0, 1500)).toBeNull(); // a slow drag, maybe selecting text
  });

  it("damps the pull, so syncing takes a deliberate pull", () => {
    expect(pullDistance(-20)).toBe(0);
    expect(pullDistance(100)).toBe(50);
    expect(pullDistance(PULL_TRIGGER_PX * 2)).toBeGreaterThanOrEqual(PULL_TRIGGER_PX);
    expect(pullDistance(1000)).toBe(110);
  });
});
