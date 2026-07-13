import { describe, expect, it } from "vitest";

import {
  advanceFrameRailAutoScroll,
  clampFrameRailAutoScroll,
  computeFrameRailAutoScrollDelta,
  resolveFrameDropTargetFromGeometry,
  resolveFrameDropTargetFromVirtualGeometry,
} from "./frameDnD";

describe("resolveFrameDropTargetFromGeometry", () => {
  const rows = [
    { instanceId: "frame-1", top: 110, bottom: 130 },
    { instanceId: "frame-2", top: 130, bottom: 150 },
    { instanceId: "frame-3", top: 150, bottom: 170 },
    { instanceId: "frame-4", top: 170, bottom: 190 },
  ];

  it("resolves the anchor from viewport geometry instead of offsetTop", () => {
    expect(
      resolveFrameDropTargetFromGeometry({
        listBounds: { left: 20, right: 220, top: 100, bottom: 200 },
        rows,
        draggedInstanceIds: ["frame-2"],
        clientX: 60,
        clientY: 162,
      }),
    ).toEqual({
      anchorInstanceId: "frame-3",
      position: "below",
    });
  });

  it("ignores dragged rows and returns a lower anchor when pointer moves down", () => {
    expect(
      resolveFrameDropTargetFromGeometry({
        listBounds: { left: 20, right: 220, top: 100, bottom: 200 },
        rows,
        draggedInstanceIds: ["frame-2", "frame-3"],
        clientX: 60,
        clientY: 184,
      }),
    ).toEqual({
      anchorInstanceId: "frame-4",
      position: "below",
    });
  });
});

describe("resolveFrameDropTargetFromVirtualGeometry", () => {
  const orderedInstanceIds = Array.from({ length: 12 }, (_, index) => `frame-${index + 1}`);
  const listBounds = { left: 20, right: 220, top: 100, bottom: 200 };

  it("resolves an offscreen full-list anchor with nonzero scrollTop", () => {
    expect(
      resolveFrameDropTargetFromVirtualGeometry({
        listBounds,
        scrollTop: 80,
        rowHeight: 20,
        orderedInstanceIds,
        draggedInstanceIds: [],
        clientX: 60,
        clientY: 130,
      }),
    ).toEqual({ anchorInstanceId: "frame-6", position: "below" });
  });

  it("uses above before the midpoint and below at the midpoint tie", () => {
    const params = {
      listBounds,
      scrollTop: 0,
      rowHeight: 20,
      orderedInstanceIds,
      draggedInstanceIds: [] as string[],
      clientX: 60,
    };
    expect(resolveFrameDropTargetFromVirtualGeometry({ ...params, clientY: 109 })).toEqual({
      anchorInstanceId: "frame-1",
      position: "above",
    });
    expect(resolveFrameDropTargetFromVirtualGeometry({ ...params, clientY: 110 })).toEqual({
      anchorInstanceId: "frame-1",
      position: "below",
    });
  });

  it("clamps viewport blank-tail geometry to the final row", () => {
    expect(
      resolveFrameDropTargetFromVirtualGeometry({
        listBounds,
        scrollTop: 180,
        rowHeight: 20,
        orderedInstanceIds,
        draggedInstanceIds: [],
        clientX: 60,
        clientY: 198,
      }),
    ).toEqual({ anchorInstanceId: "frame-12", position: "below" });
  });

  it("skips a dragged block in both directions and returns null when all rows are dragged", () => {
    const params = {
      listBounds,
      scrollTop: 0,
      rowHeight: 20,
      orderedInstanceIds,
      draggedInstanceIds: ["frame-2", "frame-3"],
      clientX: 60,
    };
    expect(resolveFrameDropTargetFromVirtualGeometry({ ...params, clientY: 135 })).toEqual({
      anchorInstanceId: "frame-4",
      position: "above",
    });
    expect(resolveFrameDropTargetFromVirtualGeometry({ ...params, clientY: 121 })).toEqual({
      anchorInstanceId: "frame-1",
      position: "below",
    });
    expect(
      resolveFrameDropTargetFromVirtualGeometry({
        ...params,
        draggedInstanceIds: orderedInstanceIds,
        clientY: 125,
      }),
    ).toBeNull();
  });

  it("rejects pointers outside either list axis", () => {
    const params = {
      listBounds,
      scrollTop: 0,
      rowHeight: 20,
      orderedInstanceIds,
      draggedInstanceIds: [] as string[],
    };
    expect(resolveFrameDropTargetFromVirtualGeometry({ ...params, clientX: 10, clientY: 120 })).toBeNull();
    expect(resolveFrameDropTargetFromVirtualGeometry({ ...params, clientX: 60, clientY: 220 })).toBeNull();
  });

  it("resolves a far-offscreen anchor from the full 1,000-frame index space", () => {
    const longTimelineIds = Array.from(
      { length: 1_000 },
      (_, index) => `frame-${index + 1}`,
    );

    expect(
      resolveFrameDropTargetFromVirtualGeometry({
        listBounds: { left: 20, right: 220, top: 100, bottom: 340 },
        scrollTop: 750 * 48,
        rowHeight: 48,
        orderedInstanceIds: longTimelineIds,
        draggedInstanceIds: [],
        clientX: 60,
        clientY: 124,
      }),
    ).toEqual({
      anchorInstanceId: "frame-751",
      position: "below",
    });
  });
});

describe("frame rail edge autoscroll math", () => {
  const listBounds = { left: 20, right: 220, top: 100, bottom: 200 };

  it("returns signed bounded deltas only inside the edge zones", () => {
    expect(computeFrameRailAutoScrollDelta({ clientY: 100, listBounds, edgeSize: 24, maxStep: 16 })).toBe(-16);
    expect(computeFrameRailAutoScrollDelta({ clientY: 150, listBounds, edgeSize: 24, maxStep: 16 })).toBe(0);
    expect(computeFrameRailAutoScrollDelta({ clientY: 200, listBounds, edgeSize: 24, maxStep: 16 })).toBe(16);
    expect(
      Math.abs(
        computeFrameRailAutoScrollDelta({ clientY: 102, listBounds, edgeSize: 24, maxStep: 16 }),
      ),
    ).toBeLessThanOrEqual(16);
  });

  it("clamps repeated RAF steps at both scroll boundaries", () => {
    expect(clampFrameRailAutoScroll({ scrollTop: 2, delta: -16, scrollHeight: 500, clientHeight: 100 })).toBe(0);
    expect(clampFrameRailAutoScroll({ scrollTop: 395, delta: 16, scrollHeight: 500, clientHeight: 100 })).toBe(400);
    expect(clampFrameRailAutoScroll({ scrollTop: 120, delta: 0, scrollHeight: 500, clientHeight: 100 })).toBe(120);

    let scrollTop = 380;
    let didScroll = true;
    for (let tick = 0; tick < 4; tick += 1) {
      const advanced = advanceFrameRailAutoScroll({
        scrollTop,
        delta: 16,
        scrollHeight: 500,
        clientHeight: 100,
      });
      scrollTop = advanced.scrollTop;
      didScroll = advanced.didScroll;
    }
    expect(scrollTop).toBe(400);
    expect(didScroll).toBe(false);
  });
});
