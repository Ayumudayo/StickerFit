import type { Dispatch, RefObject, SetStateAction } from "react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { TimelineDragState, TimelineFrameView } from "../types/editor";
import type { MediaInspection } from "../types/workflow";
import {
  lastSelectedFrameView,
  selectedPlaybackFrames,
} from "../utils/frameSelection";
import { microsecondsToSeconds } from "../utils/timelineFrames";

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export type PlaybackTimelineLookup = {
  frames: TimelineFrameView[];
  startTimesUs: number[];
};

export type PlaybackTick = {
  frameIndex: number;
  delayUs: number;
};

export function createPlaybackTimelineLookup(
  frames: TimelineFrameView[],
): PlaybackTimelineLookup {
  return {
    frames,
    startTimesUs: frames.map((frame) => frame.startTimeUs),
  };
}

function timelineMicroseconds(value: number) {
  return Math.round(value * 1_000_000);
}

function findFirstFrameIndexAtOrAfter(startTimesUs: number[], timeUs: number) {
  let low = 0;
  let high = startTimesUs.length;

  while (low < high) {
    const midpoint = Math.floor((low + high) / 2);
    if (startTimesUs[midpoint] < timeUs) {
      low = midpoint + 1;
    } else {
      high = midpoint;
    }
  }

  return low < startTimesUs.length ? low : -1;
}

function findLastFrameIndexAtOrBefore(startTimesUs: number[], timeUs: number) {
  let low = 0;
  let high = startTimesUs.length;

  while (low < high) {
    const midpoint = Math.floor((low + high) / 2);
    if (startTimesUs[midpoint] <= timeUs) {
      low = midpoint + 1;
    } else {
      high = midpoint;
    }
  }

  return low - 1;
}

export function resolvePlaybackTick(
  lookup: PlaybackTimelineLookup,
  currentTimeUs: number,
): PlaybackTick | null {
  if (lookup.frames.length === 0) {
    return null;
  }

  const frameIndex = findLastFrameIndexAtOrBefore(
    lookup.startTimesUs,
    currentTimeUs,
  );
  const frame = lookup.frames[frameIndex];
  if (frame) {
    const frameEndUs = frame.startTimeUs + frame.durationUs;
    if (currentTimeUs >= frame.startTimeUs && currentTimeUs < frameEndUs) {
      return {
        frameIndex,
        delayUs: Math.max(0, frameEndUs - currentTimeUs),
      };
    }
  }

  const nextFrameIndex = findFirstFrameIndexAtOrAfter(
    lookup.startTimesUs,
    currentTimeUs,
  );
  const wrappedFrameIndex = nextFrameIndex === -1 ? 0 : nextFrameIndex;
  const nextFrame = lookup.frames[wrappedFrameIndex];

  return nextFrame
    ? {
        frameIndex: wrappedFrameIndex,
        delayUs: nextFrame.durationUs,
      }
    : null;
}

export function resolvePlaybackStartTime(
  lookup: PlaybackTimelineLookup,
  currentTimeUs: number,
) {
  const tick = resolvePlaybackTick(lookup, currentTimeUs);
  if (!tick) {
    return currentTimeUs;
  }

  const currentFrame = lookup.frames[tick.frameIndex];
  if (
    currentFrame &&
    currentTimeUs >= currentFrame.startTimeUs &&
    currentTimeUs < currentFrame.startTimeUs + currentFrame.durationUs
  ) {
    return currentTimeUs;
  }

  return currentFrame?.startTimeUs ?? currentTimeUs;
}

type UsePlaybackTimelineControllerParams = {
  editorSessionKey?: number;
  inspection: MediaInspection | null;
  setPreviewDuration: Dispatch<SetStateAction<number | null>>;
  sourceDuration: number;
  timelineFrameViews: TimelineFrameView[];
  selectedInstanceIds: string[];
  timelineRailRef: RefObject<HTMLDivElement | null>;
};

export function usePlaybackTimelineController({
  editorSessionKey,
  inspection,
  setPreviewDuration,
  sourceDuration,
  timelineFrameViews,
  selectedInstanceIds,
  timelineRailRef,
}: UsePlaybackTimelineControllerParams) {
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTimeUs, setCurrentTimeUs] = useState(0);
  const [timelineDrag, setTimelineDrag] = useState<TimelineDragState | null>(null);
  const lastSessionKeyRef = useRef<number | undefined>(editorSessionKey);

  useEffect(() => {
    if (editorSessionKey !== undefined) {
      if (lastSessionKeyRef.current === editorSessionKey) {
        return;
      }

      lastSessionKeyRef.current = editorSessionKey;
    }

    setPreviewDuration(inspection?.durationSeconds ?? null);
    setIsPlaying(false);
    setCurrentTimeUs(0);
    setTimelineDrag(null);
  }, [editorSessionKey, inspection?.durationSeconds, setPreviewDuration]);

  const timelineDurationUs = useMemo(
    () => timelineFrameViews.reduce((sum, frame) => sum + frame.durationUs, 0),
    [timelineFrameViews],
  );
  const sourceDurationUs = timelineMicroseconds(sourceDuration);
  const totalDurationUs = inspection?.isStaticImage
    ? sourceDurationUs
    : timelineDurationUs || sourceDurationUs;
  const currentTime = microsecondsToSeconds(currentTimeUs);
  const totalDuration = microsecondsToSeconds(totalDurationUs);

  useEffect(() => {
    if (currentTimeUs > totalDurationUs) {
      setCurrentTimeUs(totalDurationUs);
    }
  }, [currentTimeUs, totalDurationUs]);

  function handlePreviewDurationChange(value: number) {
    setPreviewDuration(value);
    const valueUs = timelineMicroseconds(value);
    if (currentTimeUs > valueUs) {
      setCurrentTimeUs(valueUs);
    }
  }

  function scrubTo(value: number) {
    if (totalDurationUs <= 0) {
      return;
    }

    setCurrentTimeUs(clamp(timelineMicroseconds(value), 0, totalDurationUs));
  }

  function timelineTimeFromPointer(clientX: number) {
    const bounds = timelineRailRef.current?.getBoundingClientRect();
    if (!bounds || bounds.width === 0 || totalDuration <= 0) {
      return null;
    }

    const ratio = clamp((clientX - bounds.left) / bounds.width, 0, 1);
    return ratio * totalDuration;
  }

  function applyTimelineDrag(clientX: number) {
    const nextTime = timelineTimeFromPointer(clientX);
    if (nextTime === null) {
      return;
    }

    scrubTo(nextTime);
  }

  function handleTimelinePointerDown(event: React.PointerEvent<HTMLDivElement>) {
    if (totalDuration <= 0) {
      return;
    }

    timelineRailRef.current?.setPointerCapture(event.pointerId);
    setTimelineDrag({ pointerId: event.pointerId });
    applyTimelineDrag(event.clientX);
  }

  function handleTimelinePointerMove(event: React.PointerEvent<HTMLDivElement>) {
    if (!timelineDrag || timelineDrag.pointerId !== event.pointerId) {
      return;
    }

    applyTimelineDrag(event.clientX);
  }

  function handleTimelinePointerEnd(event: React.PointerEvent<HTMLDivElement>) {
    if (timelineRailRef.current?.hasPointerCapture(event.pointerId)) {
      timelineRailRef.current.releasePointerCapture(event.pointerId);
    }

    setTimelineDrag(null);
  }

  const playbackTimelineFrames = useMemo(
    () => selectedPlaybackFrames(timelineFrameViews, selectedInstanceIds),
    [selectedInstanceIds, timelineFrameViews],
  );
  const playbackTimelineLookup = useMemo(
    () => createPlaybackTimelineLookup(playbackTimelineFrames),
    [playbackTimelineFrames],
  );
  const playbackTick = useMemo(
    () => resolvePlaybackTick(playbackTimelineLookup, currentTimeUs),
    [currentTimeUs, playbackTimelineLookup],
  );
  const focusedSelectedFrame = useMemo(
    () => lastSelectedFrameView(timelineFrameViews, selectedInstanceIds),
    [selectedInstanceIds, timelineFrameViews],
  );

  useEffect(() => {
    if (!focusedSelectedFrame) {
      return;
    }

    if (isPlaying) {
      return;
    }

    setCurrentTimeUs(focusedSelectedFrame.startTimeUs);
  }, [focusedSelectedFrame?.instanceId, focusedSelectedFrame?.startTimeUs, isPlaying]);

  function togglePlayback() {
    if (!inspection?.ok || inspection.isStaticImage || totalDuration <= 0 || playbackTimelineFrames.length === 0) {
      return;
    }

    if (isPlaying) {
      setIsPlaying(false);
      return;
    }

    const normalizedStartTimeUs = resolvePlaybackStartTime(
      playbackTimelineLookup,
      currentTimeUs,
    );
    if (normalizedStartTimeUs !== currentTimeUs) {
      setCurrentTimeUs(normalizedStartTimeUs);
    }

    setIsPlaying(true);
  }

  useEffect(() => {
    if (!isPlaying || !playbackTick) {
      if (isPlaying && !playbackTick) {
        setIsPlaying(false);
      }
      return;
    }

    let timeoutId: number;

    timeoutId = window.setTimeout(() => {
      const nextIndex = (playbackTick.frameIndex + 1) % playbackTimelineFrames.length;
      setCurrentTimeUs(playbackTimelineFrames[nextIndex].startTimeUs);
    }, Math.ceil(playbackTick.delayUs / 1_000));

    return () => window.clearTimeout(timeoutId);
  }, [isPlaying, playbackTick, playbackTimelineFrames]);

  const timelineProgress = totalDurationUs > 0
    ? clamp(currentTimeUs / totalDurationUs, 0, 1)
    : 0;
  const timelineRailStyle = {
    "--timeline-progress": String(timelineProgress),
  } as React.CSSProperties;
  const currentPlaybackFrame = playbackTick
    ? playbackTimelineFrames[playbackTick.frameIndex] ?? null
    : null;
  const previewCurrentTime = currentPlaybackFrame?.sourceStartTimeSeconds ?? 0;
  const currentFrameInstanceId = currentPlaybackFrame?.instanceId ?? null;

  return {
    currentTime,
    isPlaying,
    totalDuration,
    timelineRailStyle,
    previewCurrentTime,
    currentFrameInstanceId,
    scrubTo,
    handlePreviewDurationChange,
    togglePlayback,
    handleTimelinePointerDown,
    handleTimelinePointerMove,
    handleTimelinePointerEnd,
  };
}
