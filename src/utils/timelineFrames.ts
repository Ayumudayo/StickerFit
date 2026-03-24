import type { Locale } from "../locales/messages";
import type { SourceFrame, TimelineFrame, TimelineFrameView } from "../types/editor";

export function formatTimelineTime(value: number, locale: Locale) {
  return locale === "ko" ? `${value.toFixed(2)}초` : `${value.toFixed(2)}s`;
}

export function secondsToMicroseconds(value: number) {
  return Math.max(1, Math.round(value * 1_000_000));
}

export function microsecondsToSeconds(value: number) {
  return value / 1_000_000;
}

export function buildSourceFrames(
  durationSeconds: number | null,
  estimatedFrames: number | null,
  frameDurationsSeconds: number[] | null,
) {
  if (
    frameDurationsSeconds &&
    frameDurationsSeconds.length > 0 &&
    durationSeconds &&
    durationSeconds > 0
  ) {
    let currentStartUs = 0;

    return frameDurationsSeconds.map((frameDurationSeconds, index) => {
      const normalizedDuration = Number(frameDurationSeconds.toFixed(3));
      const frame = {
        sourceFrameId: index + 1,
        startTimeUs: currentStartUs,
        durationUs: secondsToMicroseconds(normalizedDuration),
      } satisfies SourceFrame;

      currentStartUs += frame.durationUs;
      return frame;
    });
  }

  if (!durationSeconds || durationSeconds <= 0 || !estimatedFrames || estimatedFrames <= 0) {
    return [] as SourceFrame[];
  }

  const frameCount = Math.max(1, Math.round(estimatedFrames));
  const uniformFrameDuration = Number((durationSeconds / frameCount).toFixed(3));
  let currentStartUs = 0;

  return Array.from({ length: frameCount }, (_, index) => {
    const frame = {
      sourceFrameId: index + 1,
      startTimeUs: currentStartUs,
      durationUs: secondsToMicroseconds(uniformFrameDuration),
    } satisfies SourceFrame;

    currentStartUs += frame.durationUs;
    return frame;
  });
}

export function buildInitialTimelineFrames(sourceFrames: SourceFrame[]) {
  return sourceFrames.map((sourceFrame) => ({
    instanceId: `frame-${sourceFrame.sourceFrameId}`,
    sourceFrameId: sourceFrame.sourceFrameId,
    displayNumber: sourceFrame.sourceFrameId,
    durationUs: sourceFrame.durationUs,
  })) satisfies TimelineFrame[];
}

export function buildTimelineFrameViews(
  timelineFrames: TimelineFrame[],
  sourceFrames: SourceFrame[],
) {
  let currentTimeSeconds = 0;
  const sourceFrameMap = new Map(sourceFrames.map((frame) => [frame.sourceFrameId, frame]));

  return timelineFrames.map((frame) => {
    const sourceFrame = sourceFrameMap.get(frame.sourceFrameId);
    const view = {
      instanceId: frame.instanceId,
      sourceFrameId: frame.sourceFrameId,
      displayNumber: frame.displayNumber,
      durationUs: frame.durationUs,
      durationSeconds: microsecondsToSeconds(frame.durationUs),
      startTimeSeconds: Number(currentTimeSeconds.toFixed(3)),
      sourceStartTimeSeconds: microsecondsToSeconds(sourceFrame?.startTimeUs ?? 0),
    } satisfies TimelineFrameView;

    currentTimeSeconds += microsecondsToSeconds(frame.durationUs);
    return view;
  });
}
