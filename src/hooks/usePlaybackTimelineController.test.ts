import { describe, expect, it } from "vitest";

import type { TimelineFrameView } from "../types/editor";

const timelineFrameViews: TimelineFrameView[] = [
  {
    instanceId: "frame-1",
    sourceFrameId: 1,
    displayNumber: 1,
    durationUs: 1_000_000,
    durationSeconds: 1,
    startTimeSeconds: 0,
    sourceStartTimeSeconds: 0,
  },
  {
    instanceId: "frame-2",
    sourceFrameId: 2,
    displayNumber: 2,
    durationUs: 1_000_000,
    durationSeconds: 1,
    startTimeSeconds: 1,
    sourceStartTimeSeconds: 10,
  },
  {
    instanceId: "frame-3",
    sourceFrameId: 3,
    displayNumber: 3,
    durationUs: 1_500_000,
    durationSeconds: 1.5,
    startTimeSeconds: 2,
    sourceStartTimeSeconds: 20,
  },
  {
    instanceId: "frame-4",
    sourceFrameId: 4,
    displayNumber: 4,
    durationUs: 500_000,
    durationSeconds: 0.5,
    startTimeSeconds: 3.5,
    sourceStartTimeSeconds: 35,
  },
];

describe("usePlaybackTimelineController timeline lookup helpers", () => {
  it("resolves the playback step index from the next matching timeline frame", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackTickIndex = (
      playbackModule as Record<string, unknown>
    ).resolvePlaybackTickIndex;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackTickIndex).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackTickIndex !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup(timelineFrameViews);

    expect(resolvePlaybackTickIndex(lookup, 0)).toBe(0);
    expect(resolvePlaybackTickIndex(lookup, 0.995)).toBe(1);
    expect(resolvePlaybackTickIndex(lookup, 2.004)).toBe(2);
    expect(resolvePlaybackTickIndex(lookup, 9)).toBe(0);
  });

  it("resolves the preview frame from the latest frame at or before the current time", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackFrameAtTime = (
      playbackModule as Record<string, unknown>
    ).resolvePlaybackFrameAtTime;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackFrameAtTime).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackFrameAtTime !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup(timelineFrameViews);

    expect(resolvePlaybackFrameAtTime(lookup, -0.5)?.instanceId).toBe("frame-1");
    expect(resolvePlaybackFrameAtTime(lookup, 1.2)?.instanceId).toBe("frame-2");
    expect(resolvePlaybackFrameAtTime(lookup, 3.9)?.instanceId).toBe("frame-4");
  });

  it("resolves the nearest frame instance id without scanning the full list", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolveNearestFrameInstanceIdAtTime = (
      playbackModule as Record<string, unknown>
    ).resolveNearestFrameInstanceIdAtTime;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolveNearestFrameInstanceIdAtTime).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolveNearestFrameInstanceIdAtTime !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup(timelineFrameViews);
    const selectedLookup = createPlaybackTimelineLookup([
      timelineFrameViews[0],
      timelineFrameViews[2],
      timelineFrameViews[3],
    ]);

    expect(resolveNearestFrameInstanceIdAtTime(lookup, 0.45)).toBe("frame-1");
    expect(resolveNearestFrameInstanceIdAtTime(lookup, 2.6)).toBe("frame-3");
    expect(resolveNearestFrameInstanceIdAtTime(selectedLookup, 1.4)).toBe("frame-3");
    expect(resolveNearestFrameInstanceIdAtTime(createPlaybackTimelineLookup([]), 1.4)).toBeNull();
  });

  it("normalizes play start to the next selected frame when current time is in a gap", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackStartTime = (
      playbackModule as Record<string, unknown>
    ).resolvePlaybackStartTime;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackStartTime).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackStartTime !== "function"
    ) {
      return;
    }

    const selectedLookup = createPlaybackTimelineLookup([
      timelineFrameViews[0],
      timelineFrameViews[2],
      timelineFrameViews[3],
    ]);

    expect(resolvePlaybackStartTime(selectedLookup, 1.4)).toBe(2);
    expect(resolvePlaybackStartTime(selectedLookup, 2.2)).toBe(2.2);
    expect(resolvePlaybackStartTime(selectedLookup, 9)).toBe(0);
  });
});
