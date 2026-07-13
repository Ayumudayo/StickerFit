import { describe, expect, it } from "vitest";

import type { TimelineFrameView } from "../types/editor";

const timelineFrameViews: TimelineFrameView[] = [
  {
    instanceId: "frame-1",
    sourceFrameId: 1,
    displayNumber: 1,
    durationUs: 1_000_000,
    durationSeconds: 1,
    startTimeUs: 0,
    startTimeSeconds: 0,
    sourceStartTimeSeconds: 0,
  },
  {
    instanceId: "frame-2",
    sourceFrameId: 2,
    displayNumber: 2,
    durationUs: 1_000_000,
    durationSeconds: 1,
    startTimeUs: 1_000_000,
    startTimeSeconds: 1,
    sourceStartTimeSeconds: 10,
  },
  {
    instanceId: "frame-3",
    sourceFrameId: 3,
    displayNumber: 3,
    durationUs: 1_500_000,
    durationSeconds: 1.5,
    startTimeUs: 2_000_000,
    startTimeSeconds: 2,
    sourceStartTimeSeconds: 20,
  },
  {
    instanceId: "frame-4",
    sourceFrameId: 4,
    displayNumber: 4,
    durationUs: 500_000,
    durationSeconds: 0.5,
    startTimeUs: 3_500_000,
    startTimeSeconds: 3.5,
    sourceStartTimeSeconds: 35,
  },
];

describe("usePlaybackTimelineController timeline lookup helpers", () => {
  it("resolves the containing frame and its remaining delay in microseconds", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackTick = (playbackModule as Record<string, unknown>)
      .resolvePlaybackTick;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackTick).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackTick !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup(timelineFrameViews);

    expect(resolvePlaybackTick(lookup, 2_000_000)).toEqual({
      frameIndex: 2,
      delayUs: 1_500_000,
    });
    expect(resolvePlaybackTick(lookup, 2_200_000)).toEqual({
      frameIndex: 2,
      delayUs: 1_300_000,
    });
    expect(resolvePlaybackTick(lookup, 4_000_000)).toEqual({
      frameIndex: 0,
      delayUs: 1_000_000,
    });
  });

  it("uses exact half-open microsecond frame boundaries", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackTick = (playbackModule as Record<string, unknown>)
      .resolvePlaybackTick;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackTick).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackTick !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup(timelineFrameViews);

    expect(resolvePlaybackTick(lookup, 1_999_999)).toEqual({
      frameIndex: 1,
      delayUs: 1,
    });
    expect(resolvePlaybackTick(lookup, 2_000_000)).toEqual({
      frameIndex: 2,
      delayUs: 1_500_000,
    });
    expect(resolvePlaybackTick(lookup, 3_500_000)).toEqual({
      frameIndex: 3,
      delayUs: 500_000,
    });
  });

  it("selects the next frame across a gap", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackTick = (playbackModule as Record<string, unknown>)
      .resolvePlaybackTick;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackTick).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackTick !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup([
      timelineFrameViews[0],
      timelineFrameViews[2],
      timelineFrameViews[3],
    ]);

    expect(resolvePlaybackTick(lookup, 1_400_000)).toEqual({
      frameIndex: 1,
      delayUs: 1_500_000,
    });
  });

  it("returns null for an empty playback lookup", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackTick = (playbackModule as Record<string, unknown>)
      .resolvePlaybackTick;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackTick).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackTick !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup([]);

    expect(resolvePlaybackTick(lookup, 0)).toBeNull();
  });

  it("uses integer microsecond offsets as the lookup authority", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackTick = (playbackModule as Record<string, unknown>)
      .resolvePlaybackTick;

    expect(createPlaybackTimelineLookup).toBeTypeOf("function");
    expect(resolvePlaybackTick).toBeTypeOf("function");
    if (
      typeof createPlaybackTimelineLookup !== "function" ||
      typeof resolvePlaybackTick !== "function"
    ) {
      return;
    }

    const lookup = createPlaybackTimelineLookup([
      {
        ...timelineFrameViews[0],
        durationUs: 333_334,
        durationSeconds: 0.333334,
      },
      {
        ...timelineFrameViews[1],
        startTimeUs: 333_334,
        startTimeSeconds: 0.333,
        durationUs: 333_333,
        durationSeconds: 0.333333,
      },
    ]);

    expect(resolvePlaybackTick(lookup, 333_333)).toEqual({
      frameIndex: 0,
      delayUs: 1,
    });
    expect(resolvePlaybackTick(lookup, 333_334)).toEqual({
      frameIndex: 1,
      delayUs: 333_333,
    });
  });

  it("normalizes play start to the next selected frame when current time is in a gap", async () => {
    const playbackModule = await import("./usePlaybackTimelineController");
    const createPlaybackTimelineLookup = (
      playbackModule as Record<string, unknown>
    ).createPlaybackTimelineLookup;
    const resolvePlaybackStartTime = (playbackModule as Record<string, unknown>)
      .resolvePlaybackStartTime;

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

    expect(resolvePlaybackStartTime(selectedLookup, 1_400_000)).toBe(2_000_000);
    expect(resolvePlaybackStartTime(selectedLookup, 2_200_000)).toBe(2_200_000);
    expect(resolvePlaybackStartTime(selectedLookup, 9_000_000)).toBe(0);
  });
});
