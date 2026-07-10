import type { Dispatch, RefObject, SetStateAction } from "react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { TimelineDragState, TimelineFrameView } from "../types/editor";
import type { MediaInspection } from "../types/workflow";
import {
  lastSelectedFrameView,
  selectedPlaybackFrames,
} from "../utils/frameSelection";
import {
  microsecondsToSeconds,
  timelineDurationSeconds,
} from "../utils/timelineFrames";

const PLAYBACK_TICK_EPSILON_US = 10_000;
const PLAYBACK_FRAME_EPSILON_US = 1_000;

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export type PlaybackTimelineLookup = {
  frames: TimelineFrameView[];
  startTimesUs: number[];
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

export function findPlaybackFrameIndexAtOrAfter(
  lookup: PlaybackTimelineLookup,
  currentTime: number,
) {
  return findFirstFrameIndexAtOrAfter(
    lookup.startTimesUs,
    timelineMicroseconds(currentTime) - PLAYBACK_TICK_EPSILON_US,
  );
}

export function resolvePlaybackTickIndex(
  lookup: PlaybackTimelineLookup,
  currentTime: number,
) {
  if (lookup.frames.length === 0) {
    return 0;
  }

  const nextFrameIndex = findPlaybackFrameIndexAtOrAfter(lookup, currentTime);
  return nextFrameIndex === -1 ? 0 : nextFrameIndex;
}

export function resolvePlaybackFrameAtTime(
  lookup: PlaybackTimelineLookup,
  currentTime: number,
) {
  if (lookup.frames.length === 0) {
    return null;
  }

  const frameIndex = findLastFrameIndexAtOrBefore(
    lookup.startTimesUs,
    timelineMicroseconds(currentTime) + PLAYBACK_FRAME_EPSILON_US,
  );

  return frameIndex === -1 ? lookup.frames[0] ?? null : lookup.frames[frameIndex] ?? null;
}

export function resolveNearestFrameInstanceIdAtTime(
  lookup: PlaybackTimelineLookup,
  currentTime: number,
) {
  if (lookup.frames.length === 0) {
    return null;
  }

  const currentTimeUs = timelineMicroseconds(currentTime);
  const nextFrameIndex = findFirstFrameIndexAtOrAfter(
    lookup.startTimesUs,
    currentTimeUs,
  );
  if (nextFrameIndex === -1) {
    return lookup.frames[lookup.frames.length - 1]?.instanceId ?? null;
  }

  if (nextFrameIndex === 0) {
    return lookup.frames[0]?.instanceId ?? null;
  }

  const previousFrame = lookup.frames[nextFrameIndex - 1];
  const nextFrame = lookup.frames[nextFrameIndex];
  if (!previousFrame) {
    return nextFrame?.instanceId ?? null;
  }

  if (!nextFrame) {
    return previousFrame.instanceId;
  }

  const previousDistance = Math.abs(previousFrame.startTimeUs - currentTimeUs);
  const nextDistance = Math.abs(nextFrame.startTimeUs - currentTimeUs);
  return nextDistance < previousDistance ? nextFrame.instanceId : previousFrame.instanceId;
}

export function resolvePlaybackStartTime(
  lookup: PlaybackTimelineLookup,
  currentTime: number,
) {
  if (lookup.frames.length === 0) {
    return currentTime;
  }

  const currentFrame = resolvePlaybackFrameAtTime(lookup, currentTime);
  const currentTimeUs = timelineMicroseconds(currentTime);
  if (
    currentFrame &&
    currentTimeUs >= currentFrame.startTimeUs &&
    currentTimeUs < currentFrame.startTimeUs + currentFrame.durationUs
  ) {
    return currentTime;
  }

  const nextFrameIndex = findPlaybackFrameIndexAtOrAfter(lookup, currentTime);
  return nextFrameIndex === -1
    ? microsecondsToSeconds(lookup.frames[0]?.startTimeUs ?? timelineMicroseconds(currentTime))
    : microsecondsToSeconds(
        lookup.frames[nextFrameIndex]?.startTimeUs ?? timelineMicroseconds(currentTime),
      );
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
  const [currentTime, setCurrentTime] = useState(0);
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
    setCurrentTime(0);
    setTimelineDrag(null);
  }, [editorSessionKey, inspection?.durationSeconds, setPreviewDuration]);

  const timelineDuration = useMemo(
    () => timelineDurationSeconds(timelineFrameViews),
    [timelineFrameViews],
  );
  const totalDuration = inspection?.isStaticImage
    ? sourceDuration
    : timelineDuration || sourceDuration;

  useEffect(() => {
    if (currentTime > totalDuration) {
      setCurrentTime(totalDuration);
    }
  }, [currentTime, totalDuration]);

  function handlePreviewDurationChange(value: number) {
    setPreviewDuration(value);
    if (currentTime > value) {
      setCurrentTime(value);
    }
  }

  function scrubTo(value: number) {
    if (totalDuration <= 0) {
      return;
    }

    setCurrentTime(clamp(value, 0, totalDuration));
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

    setCurrentTime(microsecondsToSeconds(focusedSelectedFrame.startTimeUs));
  }, [focusedSelectedFrame?.instanceId, focusedSelectedFrame?.startTimeUs, isPlaying]);

  function togglePlayback() {
    if (!inspection?.ok || inspection.isStaticImage || totalDuration <= 0 || playbackTimelineFrames.length === 0) {
      return;
    }

    if (isPlaying) {
      setIsPlaying(false);
      return;
    }

    const normalizedStartTime = resolvePlaybackStartTime(playbackTimelineLookup, currentTime);
    if (normalizedStartTime !== currentTime) {
      setCurrentTime(normalizedStartTime);
    }

    setIsPlaying(true);
  }

  useEffect(() => {
    if (!isPlaying || playbackTimelineFrames.length === 0) {
      if (isPlaying && playbackTimelineFrames.length === 0) {
        setIsPlaying(false);
      }
      return;
    }

    let timeoutId: number;

    const normalizedCurrentIndex = resolvePlaybackTickIndex(
      playbackTimelineLookup,
      currentTime,
    );
    const currentFrame = playbackTimelineFrames[normalizedCurrentIndex];

    timeoutId = window.setTimeout(() => {
      const nextIndex = (normalizedCurrentIndex + 1) % playbackTimelineFrames.length;
      setCurrentTime(microsecondsToSeconds(playbackTimelineFrames[nextIndex].startTimeUs));
    }, currentFrame.durationUs / 1_000);

    return () => window.clearTimeout(timeoutId);
  }, [currentTime, isPlaying, playbackTimelineFrames, playbackTimelineLookup]);

  const timelineProgress = totalDuration > 0 ? clamp(currentTime / totalDuration, 0, 1) : 0;
  const timelineRailStyle = {
    "--timeline-progress": String(timelineProgress),
  } as React.CSSProperties;
  const currentPlaybackFrame = resolvePlaybackFrameAtTime(
    playbackTimelineLookup,
    currentTime,
  );
  const previewCurrentTime = currentPlaybackFrame?.sourceStartTimeSeconds ?? 0;
  const currentFrameInstanceId = resolveNearestFrameInstanceIdAtTime(
    playbackTimelineLookup,
    currentTime,
  );

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
