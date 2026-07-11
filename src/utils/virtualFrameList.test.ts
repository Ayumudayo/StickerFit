import { describe, expect, it } from "vitest";

import {
  computeScrollTopToRevealIndex,
  computeVirtualWindow,
} from "./virtualFrameList";

describe("computeVirtualWindow", () => {
  it("returns an empty end-exclusive window for an empty list", () => {
    expect(
      computeVirtualWindow({
        scrollTop: 0,
        viewportHeight: 200,
        rowHeight: 40,
        itemCount: 0,
        overscan: 2,
      }),
    ).toEqual({ start: 0, end: 0, offsetTop: 0 });
  });

  it("clamps overscan at the top", () => {
    expect(
      computeVirtualWindow({
        scrollTop: 0,
        viewportHeight: 100,
        rowHeight: 40,
        itemCount: 20,
        overscan: 2,
      }),
    ).toEqual({ start: 0, end: 5, offsetTop: 0 });
  });

  it("computes a middle end-exclusive range and matching offset", () => {
    expect(
      computeVirtualWindow({
        scrollTop: 100,
        viewportHeight: 100,
        rowHeight: 40,
        itemCount: 20,
        overscan: 1,
      }),
    ).toEqual({ start: 1, end: 6, offsetTop: 40 });
  });

  it("clamps overscan and blank tail at the bottom", () => {
    expect(
      computeVirtualWindow({
        scrollTop: 700,
        viewportHeight: 100,
        rowHeight: 40,
        itemCount: 20,
        overscan: 1,
      }),
    ).toEqual({ start: 16, end: 20, offsetTop: 640 });
  });

  it("keeps exact row boundaries end-exclusive", () => {
    expect(
      computeVirtualWindow({
        scrollTop: 80,
        viewportHeight: 80,
        rowHeight: 40,
        itemCount: 10,
        overscan: 0,
      }),
    ).toEqual({ start: 2, end: 4, offsetTop: 80 });
  });

  it("renders all items when the viewport is taller than the content", () => {
    expect(
      computeVirtualWindow({
        scrollTop: 500,
        viewportHeight: 400,
        rowHeight: 40,
        itemCount: 3,
        overscan: 4,
      }),
    ).toEqual({ start: 0, end: 3, offsetTop: 0 });
  });

  it("returns a deterministic empty window for invalid numeric inputs", () => {
    for (const params of [
      { scrollTop: Number.NaN, viewportHeight: 100, rowHeight: 40, itemCount: 3, overscan: 1 },
      { scrollTop: 0, viewportHeight: Number.POSITIVE_INFINITY, rowHeight: 40, itemCount: 3, overscan: 1 },
      { scrollTop: 0, viewportHeight: 100, rowHeight: 0, itemCount: 3, overscan: 1 },
      { scrollTop: 0, viewportHeight: 100, rowHeight: 40, itemCount: -1, overscan: 1 },
    ]) {
      expect(computeVirtualWindow(params)).toEqual({ start: 0, end: 0, offsetTop: 0 });
    }
  });
});

describe("computeScrollTopToRevealIndex", () => {
  const base = {
    viewportHeight: 120,
    rowHeight: 40,
    itemCount: 20,
  };

  it("scrolls upward to reveal an offscreen row", () => {
    expect(computeScrollTopToRevealIndex({ ...base, scrollTop: 120, index: 1 })).toBe(40);
  });

  it("scrolls downward just enough to reveal an offscreen row", () => {
    expect(computeScrollTopToRevealIndex({ ...base, scrollTop: 0, index: 5 })).toBe(120);
  });

  it("keeps scrollTop when the row is already fully visible", () => {
    expect(computeScrollTopToRevealIndex({ ...base, scrollTop: 120, index: 4 })).toBe(120);
  });

  it("clamps an out-of-range index and final scroll offset", () => {
    expect(computeScrollTopToRevealIndex({ ...base, scrollTop: 0, index: 999 })).toBe(680);
    expect(computeScrollTopToRevealIndex({ ...base, scrollTop: 999, index: -4 })).toBe(0);
  });

  it("returns zero for empty or invalid reveal geometry", () => {
    expect(computeScrollTopToRevealIndex({ ...base, itemCount: 0, scrollTop: 80, index: 1 })).toBe(0);
    expect(computeScrollTopToRevealIndex({ ...base, rowHeight: 0, scrollTop: 80, index: 1 })).toBe(0);
    expect(computeScrollTopToRevealIndex({ ...base, scrollTop: Number.NaN, index: 1 })).toBe(0);
  });

  it("clamps stale scrollTop after the list shrinks", () => {
    expect(
      computeScrollTopToRevealIndex({
        viewportHeight: 120,
        rowHeight: 40,
        itemCount: 4,
        scrollTop: 680,
        index: 3,
      }),
    ).toBe(40);
  });
});
