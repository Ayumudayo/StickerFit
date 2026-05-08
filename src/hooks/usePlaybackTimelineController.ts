import type { Dispatch, RefObject, SetStateAction } from "react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { TimelineDragState, TimelineFrameView } from "../types/editor";
import type { MediaInspection } from "../types/workflow";
import {
  lastSelectedFrameView,
  selectedPlaybackFrames,
} from "../utils/frameSelection";

const PLAYBACK_TICK_EPSILON_SECONDS = 0.01;
const PLAYBACK_FRAME_EPSILON_SECONDS = 0.001;

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export type PlaybackTimelineLookup = {
  frames: TimelineFrameView[];
  startTimes: number[];
};

export function createPlaybackTimelineLookup(
  frames: TimelineFrameView[],
): PlaybackTimelineLookup {
  return {
    frames,
    startTimes: frames.map((frame) => frame.startTimeSeconds),
  };
}

function findFirstFrameIndexAtOrAfter(startTimes: number[], timeSeconds: number) {
  let low = 0;
  let high = startTimes.length;

  while (low < high) {
    const midpoint = Math.floor((low + high) / 2);
    if (startTimes[midpoint] < timeSeconds) {
      low = midpoint + 1;
    } else {
      high = midpoint;
    }
  }

  return low < startTimes.length ? low : -1;
}

function findLastFrameIndexAtOrBefore(startTimes: number[], timeSeconds: number) {
  let low = 0;
  let high = startTimes.length;

  while (low < high) {
    const midpoint = Math.floor((low + high) / 2);
    if (startTimes[midpoint] <= timeSeconds) {
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
    lookup.startTimes,
    currentTime - PLAYBACK_TICK_EPSILON_SECONDS,
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
    lookup.startTimes,
    currentTime + PLAYBACK_FRAME_EPSILON_SECONDS,
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

  const nextFrameIndex = findFirstFrameIndexAtOrAfter(lookup.startTimes, currentTime);
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

  const previousDistance = Math.abs(previousFrame.startTimeSeconds - currentTime);
  const nextDistance = Math.abs(nextFrame.startTimeSeconds - currentTime);
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
  if (
    currentFrame &&
    currentTime >= currentFrame.startTimeSeconds &&
    currentTime < currentFrame.startTimeSeconds + currentFrame.durationSeconds
  ) {
    return currentTime;
  }

  const nextFrameIndex = findPlaybackFrameIndexAtOrAfter(lookup, currentTime);
  return nextFrameIndex === -1
    ? lookup.frames[0]?.startTimeSeconds ?? currentTime
    : lookup.frames[nextFrameIndex]?.startTimeSeconds ?? currentTime;
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

  const timelineDurationSeconds = useMemo(
    () => timelineFrameViews.reduce((sum, frame) => sum + frame.durationSeconds, 0),
    [timelineFrameViews],
  );
  const totalDuration = inspection?.isStaticImage
    ? sourceDuration
    : timelineDurationSeconds || sourceDuration;

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

    setCurrentTime(focusedSelectedFrame.startTimeSeconds);
  }, [focusedSelectedFrame?.instanceId, focusedSelectedFrame?.startTimeSeconds, isPlaying]);

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
      setCurrentTime(playbackTimelineFrames[nextIndex].startTimeSeconds);
    }, currentFrame.durationSeconds * 1000);

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
