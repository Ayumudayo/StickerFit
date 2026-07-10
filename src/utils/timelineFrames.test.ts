import { describe, expect, it } from "vitest";

import {
  buildInitialTimelineFrames,
  buildSourceFrames,
  buildTimelineFrameViews,
} from "./timelineFrames";

describe("timeline frame durations", () => {
  it("distributes one second across three frames without losing microseconds", () => {
    const frames = buildSourceFrames(1, 3, null);

    expect(frames.map((frame) => frame.durationUs)).toEqual([
      333_334,
      333_333,
      333_333,
    ]);
    expect(frames.reduce((sum, frame) => sum + frame.durationUs, 0)).toBe(
      1_000_000,
    );
  });

  it("uses inspected frame durations even when duration metadata is unavailable", () => {
    const frames = buildSourceFrames(
      null,
      null,
      [0.016667, 0.016667, 0.016666],
    );

    expect(frames.map((frame) => frame.durationUs)).toEqual([
      16_667,
      16_667,
      16_666,
    ]);
    expect(frames.reduce((sum, frame) => sum + frame.durationUs, 0)).toBe(50_000);
  });

  it("derives timeline view start times from integer microseconds", () => {
    const sourceFrames = buildSourceFrames(1, 3, null);
    const views = buildTimelineFrameViews(
      buildInitialTimelineFrames(sourceFrames),
      sourceFrames,
    );

    expect(views.map((frame) => frame.startTimeUs)).toEqual([
      0,
      333_334,
      666_667,
    ]);
    expect(views.map((frame) => frame.startTimeSeconds)).toEqual([
      0,
      0.333334,
      0.666667,
    ]);
  });

  it("preserves exactly ten seconds across three hundred frames", () => {
    const frames = buildInitialTimelineFrames(buildSourceFrames(10, 300, null));

    expect(frames.reduce((sum, frame) => sum + frame.durationUs, 0)).toBe(
      10_000_000,
    );
  });

  it("reports timeline duration from integer microseconds", async () => {
    const timelineFramesModule = await import("./timelineFrames");
    const timelineDurationSeconds = (
      timelineFramesModule as Record<string, unknown>
    ).timelineDurationSeconds;

    expect(timelineDurationSeconds).toBeTypeOf("function");
    if (typeof timelineDurationSeconds !== "function") {
      return;
    }

    expect(
      timelineDurationSeconds([
        { durationUs: 333_334 },
        { durationUs: 333_333 },
        { durationUs: 333_333 },
      ]),
    ).toBe(1);
  });
});
