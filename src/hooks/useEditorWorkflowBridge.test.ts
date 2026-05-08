import { describe, expect, it } from "vitest";

import type { SourceFrame, TimelineFrame } from "../types/editor";
import { buildEditedTimelineFramesForRequest } from "./useEditorWorkflowBridge";

describe("buildEditedTimelineFramesForRequest", () => {
  const sourceFrames: SourceFrame[] = [
    { sourceFrameId: 1, startTimeUs: 0, durationUs: 100_000 },
    { sourceFrameId: 2, startTimeUs: 100_000, durationUs: 200_000 },
    { sourceFrameId: 3, startTimeUs: 300_000, durationUs: 300_000 },
  ];

  it("omits timeline frames when the editor still matches the source sequence", () => {
    const timelineFrames: TimelineFrame[] = sourceFrames.map((frame) => ({
      instanceId: `frame-${frame.sourceFrameId}`,
      sourceFrameId: frame.sourceFrameId,
      displayNumber: frame.sourceFrameId,
      durationUs: frame.durationUs,
    }));

    expect(buildEditedTimelineFramesForRequest(timelineFrames, sourceFrames)).toBeUndefined();
  });

  it("sends timeline frames after order or duration edits", () => {
    const editedTimelineFrames: TimelineFrame[] = [
      {
        instanceId: "frame-2",
        sourceFrameId: 2,
        displayNumber: 2,
        durationUs: 250_000,
      },
      {
        instanceId: "frame-1",
        sourceFrameId: 1,
        displayNumber: 1,
        durationUs: 100_000,
      },
    ];

    expect(buildEditedTimelineFramesForRequest(editedTimelineFrames, sourceFrames)).toEqual([
      { sourceFrameId: 2, durationUs: 250_000 },
      { sourceFrameId: 1, durationUs: 100_000 },
    ]);
  });
});
