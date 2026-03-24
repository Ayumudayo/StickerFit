import type { Dispatch, RefObject, SetStateAction } from "react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { TimelineDragState, TimelineFrameView } from "../types/editor";
import type { MediaInspection } from "../types/workflow";

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
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

  const playbackTimelineFrames = timelineFrameViews;

  function togglePlayback() {
    if (!inspection?.ok || inspection.isStaticImage || totalDuration <= 0 || playbackTimelineFrames.length === 0) {
      return;
    }

    if (isPlaying) {
      setIsPlaying(false);
      return;
    }

    const firstSelectedFrame = playbackTimelineFrames[0];
    const nextSelectedFrame = playbackTimelineFrames.find(
      (frame) => frame.startTimeSeconds >= currentTime - 0.01,
    );

    if (!nextSelectedFrame) {
      setCurrentTime(firstSelectedFrame.startTimeSeconds);
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

    const currentFrameIndex = playbackTimelineFrames.findIndex(
      (frame) => Math.abs(frame.startTimeSeconds - currentTime) < 0.01,
    );
    const resolvedCurrentIndex =
      currentFrameIndex !== -1
        ? currentFrameIndex
        : playbackTimelineFrames.findIndex((frame) => frame.startTimeSeconds >= currentTime - 0.01);
    const normalizedCurrentIndex = resolvedCurrentIndex === -1 ? 0 : resolvedCurrentIndex;
    const currentFrame = playbackTimelineFrames[normalizedCurrentIndex];

    timeoutId = window.setTimeout(() => {
      const nextIndex = (normalizedCurrentIndex + 1) % playbackTimelineFrames.length;
      setCurrentTime(playbackTimelineFrames[nextIndex].startTimeSeconds);
    }, currentFrame.durationSeconds * 1000);

    return () => window.clearTimeout(timeoutId);
  }, [currentTime, isPlaying, playbackTimelineFrames]);

  const timelineProgress = totalDuration > 0 ? clamp(currentTime / totalDuration, 0, 1) : 0;
  const timelineRailStyle = {
    "--timeline-progress": String(timelineProgress),
  } as React.CSSProperties;
  const currentPlaybackFrame =
    [...playbackTimelineFrames]
      .reverse()
      .find((frame) => frame.startTimeSeconds <= currentTime + 0.001) ??
    playbackTimelineFrames[0] ??
    null;
  const previewCurrentTime = currentPlaybackFrame?.sourceStartTimeSeconds ?? 0;
  const selectedTimelineFrames = timelineFrameViews.filter((frame) =>
    selectedInstanceIds.includes(frame.instanceId),
  );
  const currentFrameCandidates =
    selectedTimelineFrames.length > 0 ? selectedTimelineFrames : timelineFrameViews;
  const currentFrameInstanceId =
    currentFrameCandidates.length > 0
      ? currentFrameCandidates.reduce((closest, candidate) =>
          Math.abs(candidate.startTimeSeconds - currentTime) <
          Math.abs(closest.startTimeSeconds - currentTime)
            ? candidate
            : closest,
        ).instanceId
      : null;

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
