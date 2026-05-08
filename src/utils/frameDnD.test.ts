import { describe, expect, it } from "vitest";

import { resolveFrameDropTargetFromGeometry } from "./frameDnD";

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
