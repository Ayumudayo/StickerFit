import { describe, expect, it } from "vitest";

import {
  buildFramePointerSelection,
  frameIdsInRange,
  lastSelectedFrameView,
  selectedPlaybackFrames,
} from "./frameSelection";
import type { TimelineFrameView } from "../types/editor";

const frameIds = ["frame-1", "frame-2", "frame-3", "frame-4", "frame-5"];

const frameViews: TimelineFrameView[] = frameIds.map((instanceId, index) => ({
  instanceId,
  sourceFrameId: index + 1,
  displayNumber: index + 1,
  durationUs: 100_000,
  durationSeconds: 0.1,
  startTimeSeconds: index / 10,
  sourceStartTimeSeconds: index / 10,
}));

describe("frame selection helpers", () => {
  it("builds Explorer-style plain, Ctrl, and Shift selections", () => {
    expect(
      buildFramePointerSelection({
        orderedFrameIds: frameIds,
        selectedInstanceIds: ["frame-1", "frame-3"],
        anchorInstanceId: "frame-1",
        targetInstanceId: "frame-4",
        ctrlKey: false,
        shiftKey: false,
      }),
    ).toEqual({
      selectedInstanceIds: ["frame-4"],
      anchorInstanceId: "frame-4",
      action: "replace",
    });

    expect(
      buildFramePointerSelection({
        orderedFrameIds: frameIds,
        selectedInstanceIds: ["frame-1", "frame-3"],
        anchorInstanceId: "frame-1",
        targetInstanceId: "frame-3",
        ctrlKey: true,
        shiftKey: false,
      }),
    ).toEqual({
      selectedInstanceIds: ["frame-1"],
      anchorInstanceId: "frame-3",
      action: "toggle",
    });

    expect(
      buildFramePointerSelection({
        orderedFrameIds: frameIds,
        selectedInstanceIds: ["frame-5"],
        anchorInstanceId: "frame-2",
        targetInstanceId: "frame-4",
        ctrlKey: false,
        shiftKey: true,
      }),
    ).toEqual({
      selectedInstanceIds: ["frame-2", "frame-3", "frame-4"],
      anchorInstanceId: "frame-2",
      action: "range",
    });
  });

  it("keeps the most recently selected frame available for preview focus", () => {
    expect(frameIdsInRange(frameIds, "frame-4", "frame-2")).toEqual([
      "frame-4",
      "frame-3",
      "frame-2",
    ]);
    expect(lastSelectedFrameView(frameViews, ["frame-1", "frame-3"])?.instanceId).toBe("frame-3");
    expect(lastSelectedFrameView(frameViews, ["frame-3", "frame-2"])?.instanceId).toBe("frame-2");
  });

  it("keeps playback based on every frame present in the frame rail", () => {
    expect(selectedPlaybackFrames(frameViews, ["frame-4", "frame-2"]).map((frame) => frame.instanceId))
      .toEqual(frameIds);
    expect(selectedPlaybackFrames(frameViews, []).map((frame) => frame.instanceId))
      .toEqual(frameIds);
  });

  it("deselects the clicked selected frame with Ctrl-click", () => {
    expect(
      buildFramePointerSelection({
        orderedFrameIds: frameIds,
        selectedInstanceIds: ["frame-1", "frame-3"],
        anchorInstanceId: "frame-1",
        targetInstanceId: "frame-3",
        ctrlKey: true,
        shiftKey: false,
      }),
    ).toEqual({
      selectedInstanceIds: ["frame-1"],
      anchorInstanceId: "frame-3",
      action: "toggle",
    });
  });

  it("preserves selection history when Ctrl-deselecting the last selected frame", () => {
    expect(
      buildFramePointerSelection({
        orderedFrameIds: frameIds,
        selectedInstanceIds: ["frame-4", "frame-2", "frame-5"],
        anchorInstanceId: "frame-5",
        targetInstanceId: "frame-5",
        ctrlKey: true,
        shiftKey: false,
      }),
    ).toEqual({
      selectedInstanceIds: ["frame-4", "frame-2"],
      anchorInstanceId: "frame-5",
      action: "toggle",
    });
  });

  it("preserves existing selection history when Ctrl-Shift adds a range", () => {
    expect(
      buildFramePointerSelection({
        orderedFrameIds: frameIds,
        selectedInstanceIds: ["frame-4", "frame-2"],
        anchorInstanceId: "frame-2",
        targetInstanceId: "frame-5",
        ctrlKey: true,
        shiftKey: true,
      }),
    ).toEqual({
      selectedInstanceIds: ["frame-4", "frame-2", "frame-3", "frame-5"],
      anchorInstanceId: "frame-2",
      action: "range",
    });
  });
});
