import { describe, expect, it } from "vitest";

import type { TimelineFrameView } from "../types/editor";
import {
  buildTimelineSegmentBuckets,
  MAX_TIMELINE_SEGMENT_BUCKETS,
} from "./timelineSegments";

function buildFrameViews(durationsUs: readonly number[]): TimelineFrameView[] {
  let startTimeUs = 0;

  return durationsUs.map((durationUs, index) => {
    const frame = {
      instanceId: `frame-${index}`,
      sourceFrameId: index + 1,
      displayNumber: index + 1,
      durationUs,
      durationSeconds: durationUs / 1_000_000,
      startTimeUs,
      startTimeSeconds: startTimeUs / 1_000_000,
      sourceStartTimeSeconds: startTimeUs / 1_000_000,
    } satisfies TimelineFrameView;

    startTimeUs += durationUs;
    return frame;
  });
}

describe("buildTimelineSegmentBuckets", () => {
  it("returns no buckets for an empty timeline", () => {
    expect(
      buildTimelineSegmentBuckets({
        frames: [],
        currentTimeUs: 0,
        selectedInstanceIds: new Set(),
      }),
    ).toEqual([]);
  });

  it("keeps one exact bucket per frame at or below the limit", () => {
    const frames = buildFrameViews([100, 200, 300]);

    expect(
      buildTimelineSegmentBuckets({
        frames,
        currentTimeUs: 100,
        selectedInstanceIds: new Set(["frame-2"]),
      }),
    ).toEqual([
      {
        startUs: 0,
        durationUs: 100,
        firstFrameIndex: 0,
        lastFrameIndex: 0,
        containsCurrent: false,
        containsSelected: false,
      },
      {
        startUs: 100,
        durationUs: 200,
        firstFrameIndex: 1,
        lastFrameIndex: 1,
        containsCurrent: true,
        containsSelected: false,
      },
      {
        startUs: 300,
        durationUs: 300,
        firstFrameIndex: 2,
        lastFrameIndex: 2,
        containsCurrent: false,
        containsSelected: true,
      },
    ]);
  });

  it("bounds a 1,000-frame timeline while preserving exact ranges and state", () => {
    const frames = buildFrameViews(Array.from({ length: 1_000 }, () => 1_000));
    const buckets = buildTimelineSegmentBuckets({
      frames,
      currentTimeUs: 500_500,
      selectedInstanceIds: new Set(["frame-0", "frame-999"]),
    });

    expect(buckets).toHaveLength(MAX_TIMELINE_SEGMENT_BUCKETS);
    expect(buckets[0]).toMatchObject({
      startUs: 0,
      durationUs: 5_000,
      firstFrameIndex: 0,
      lastFrameIndex: 4,
      containsSelected: true,
    });
    expect(buckets[buckets.length - 1]).toMatchObject({
      startUs: 995_000,
      durationUs: 5_000,
      firstFrameIndex: 995,
      lastFrameIndex: 999,
      containsSelected: true,
    });
    expect(buckets.filter((bucket) => bucket.containsCurrent)).toEqual([
      expect.objectContaining({
        firstFrameIndex: 500,
        lastFrameIndex: 504,
      }),
    ]);
    expect(
      buckets.reduce((sum, bucket) => sum + bucket.durationUs, 0),
    ).toBe(1_000_000);
    expect(
      buckets.every((bucket, index) =>
        index === 0 ||
        bucket.firstFrameIndex === buckets[index - 1].lastFrameIndex + 1
      ),
    ).toBe(true);
  });

  it("chooses adjacent boundaries by duration rather than frame count", () => {
    const frames = buildFrameViews([1_000, ...Array.from({ length: 200 }, () => 1)]);
    const buckets = buildTimelineSegmentBuckets({
      frames,
      currentTimeUs: 1_050,
      selectedInstanceIds: new Set(["frame-100"]),
      maxBuckets: 2,
    });

    expect(buckets).toEqual([
      {
        startUs: 0,
        durationUs: 1_000,
        firstFrameIndex: 0,
        lastFrameIndex: 0,
        containsCurrent: false,
        containsSelected: false,
      },
      {
        startUs: 1_000,
        durationUs: 200,
        firstFrameIndex: 1,
        lastFrameIndex: 200,
        containsCurrent: true,
        containsSelected: true,
      },
    ]);
  });

  it("caps an oversized custom limit and keeps a non-empty timeline non-empty", () => {
    const frames = buildFrameViews(Array.from({ length: 201 }, () => 10));

    expect(
      buildTimelineSegmentBuckets({
        frames,
        currentTimeUs: -1,
        selectedInstanceIds: new Set(),
        maxBuckets: 999,
      }),
    ).toHaveLength(MAX_TIMELINE_SEGMENT_BUCKETS);
    expect(
      buildTimelineSegmentBuckets({
        frames,
        currentTimeUs: -1,
        selectedInstanceIds: new Set(),
        maxBuckets: 0,
      }),
    ).toEqual([
      expect.objectContaining({
        durationUs: 2_010,
        firstFrameIndex: 0,
        lastFrameIndex: 200,
      }),
    ]);
  });
});
