import { useMemo } from "react";

import { previewKindForPath } from "../components/MediaSelectionPreview";
import type { SourceFrame, TimelineFrame } from "../types/editor";
import type { MediaInspection } from "../types/workflow";
import { buildSourceFrames } from "../utils/timelineFrames";

type UseEditorWorkflowBridgeParams = {
  inspection: MediaInspection | null;
  previewDuration: number | null;
};

export function buildEditedTimelineFramesForRequest(
  timelineFrames: TimelineFrame[],
  sourceFrames: SourceFrame[],
) {
  const matchesSource =
    timelineFrames.length === sourceFrames.length &&
    timelineFrames.every((frame, index) => {
      const sourceFrame = sourceFrames[index];
      return (
        sourceFrame !== undefined &&
        frame.sourceFrameId === sourceFrame.sourceFrameId &&
        frame.durationUs === sourceFrame.durationUs
      );
    });

  if (matchesSource) {
    return undefined;
  }

  return timelineFrames.map((frame) => ({
    sourceFrameId: frame.sourceFrameId,
    durationUs: frame.durationUs,
  }));
}

export function useEditorWorkflowBridge({
  inspection,
  previewDuration,
}: UseEditorWorkflowBridgeParams) {
  const previewKind = useMemo(
    () => previewKindForPath(inspection?.inputPath ?? null),
    [inspection?.inputPath],
  );

  const sourceDuration = inspection?.durationSeconds ?? previewDuration ?? 0;

  const sourceFrames = useMemo(
    () =>
      buildSourceFrames(
        inspection?.isStaticImage ? null : sourceDuration,
        inspection?.estimatedFrames ?? null,
        inspection?.frameDurationsSeconds ?? null,
      ),
    [
      inspection?.estimatedFrames,
      inspection?.frameDurationsSeconds,
      inspection?.isStaticImage,
      sourceDuration,
    ],
  );

  const quickResolution =
    inspection?.ok && inspection.width && inspection.height
      ? `${inspection.width} x ${inspection.height}`
      : "-";
  const quickFps = inspection?.ok && !inspection.isStaticImage
    ? inspection.avgFps?.toFixed(2) ?? inspection.frameRateLabel ?? "-"
    : null;

  return {
    previewKind,
    sourceDuration,
    sourceFrames,
    quickResolution,
    quickFps,
  };
}
