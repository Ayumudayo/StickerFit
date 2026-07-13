import type {
  CSSProperties,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEventHandler,
  RefObject,
} from "react";
import { useMemo } from "react";

import type { MessagesForLocale, Locale } from "../../locales/messages";
import type { EditorText } from "../../locales/editorText";
import type { PreviewZoomMode } from "../../components/MediaSelectionPreview";
import type { FrameSelectionModel, TimelineFrameView } from "../../types/editor";
import { buildTimelineSegmentBuckets } from "../../utils/timelineSegments";
import {
  formatTimelineTime,
  microsecondsToSeconds,
} from "../../utils/timelineFrames";
import { PauseIcon, PlayIcon } from "../AppIcons";

const TIMELINE_PAGE_STEP_US = 1_000_000;

function clampMicroseconds(value: number, maximum: number) {
  const finiteValue = Number.isFinite(value) ? Math.round(value) : 0;
  return Math.min(maximum, Math.max(0, finiteValue));
}

type PreviewControlBarProps = {
  copy: MessagesForLocale;
  ui: EditorText;
  locale: Locale;
  isStaticImage: boolean;
  currentTime: number;
  totalDuration: number;
  currentTimeUs: number;
  totalDurationUs: number;
  timelineFrameViews: TimelineFrameView[];
  selection: FrameSelectionModel;
  isPlaying: boolean;
  timelineRailStyle: CSSProperties;
  timelineRailRef: RefObject<HTMLDivElement | null>;
  previewZoomMode: PreviewZoomMode;
  previewZoomPercent: number;
  previewZoomSliderValue: number;
  previewZoomSliderMin: number;
  onTogglePlayback: () => void;
  onPointerDown: PointerEventHandler<HTMLDivElement>;
  onPointerMove: PointerEventHandler<HTMLDivElement>;
  onPointerUp: PointerEventHandler<HTMLDivElement>;
  onPointerCancel: PointerEventHandler<HTMLDivElement>;
  onTimelineTimeChangeUs: (nextTimeUs: number) => void;
  onPreviewZoomFit: () => void;
  onPreviewZoomStep: (delta: number) => void;
  onPreviewZoomChange: (nextScale: number) => void;
};

export function PreviewControlBar({
  copy,
  ui,
  locale,
  isStaticImage,
  currentTime,
  totalDuration,
  currentTimeUs,
  totalDurationUs,
  timelineFrameViews,
  selection,
  isPlaying,
  timelineRailStyle,
  timelineRailRef,
  previewZoomMode,
  previewZoomPercent,
  previewZoomSliderValue,
  previewZoomSliderMin,
  onTogglePlayback,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onPointerCancel,
  onTimelineTimeChangeUs,
  onPreviewZoomFit,
  onPreviewZoomStep,
  onPreviewZoomChange,
}: PreviewControlBarProps) {
  const normalizedTotalDurationUs = Math.max(
    0,
    Number.isFinite(totalDurationUs) ? Math.round(totalDurationUs) : 0,
  );
  const normalizedCurrentTimeUs = clampMicroseconds(
    currentTimeUs,
    normalizedTotalDurationUs,
  );
  const timelineSegmentBuckets = useMemo(
    () =>
      buildTimelineSegmentBuckets({
        frames: timelineFrameViews,
        currentTimeUs: normalizedCurrentTimeUs,
        selectedInstanceIds: selection.selectedInstanceIdSet,
      }),
    [
      normalizedCurrentTimeUs,
      selection.selectedInstanceIdSet,
      timelineFrameViews,
    ],
  );
  const timelineBoundariesUs = useMemo(() => {
    const boundaries = timelineFrameViews.map((frame) =>
      clampMicroseconds(frame.startTimeUs, normalizedTotalDurationUs)
    );
    boundaries.push(0, normalizedTotalDurationUs);
    return [...new Set(boundaries)].sort((left, right) => left - right);
  }, [normalizedTotalDurationUs, timelineFrameViews]);
  const bucketDurationTotalUs = timelineSegmentBuckets.reduce(
    (sum, bucket) => sum + bucket.durationUs,
    0,
  );

  function handleTimelineKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    let nextTimeUs: number | null = null;

    switch (event.key) {
      case "ArrowLeft":
      case "ArrowDown":
        nextTimeUs = [...timelineBoundariesUs]
          .reverse()
          .find((boundaryUs) => boundaryUs < normalizedCurrentTimeUs) ?? 0;
        break;
      case "ArrowRight":
      case "ArrowUp":
        nextTimeUs = timelineBoundariesUs.find(
          (boundaryUs) => boundaryUs > normalizedCurrentTimeUs,
        ) ?? normalizedTotalDurationUs;
        break;
      case "Home":
        nextTimeUs = 0;
        break;
      case "End":
        nextTimeUs = normalizedTotalDurationUs;
        break;
      case "PageUp":
        nextTimeUs = clampMicroseconds(
          normalizedCurrentTimeUs + TIMELINE_PAGE_STEP_US,
          normalizedTotalDurationUs,
        );
        break;
      case "PageDown":
        nextTimeUs = clampMicroseconds(
          normalizedCurrentTimeUs - TIMELINE_PAGE_STEP_US,
          normalizedTotalDurationUs,
        );
        break;
      default:
        return;
    }

    if (nextTimeUs === null) {
      return;
    }
    event.preventDefault();
    onTimelineTimeChangeUs(nextTimeUs);
  }

  return (
    <section className="previewControlBar">
      {!isStaticImage ? (
        <div className="previewTransportBar">
          <button
            className="previewTransportPlayButton"
            type="button"
            onClick={onTogglePlayback}
            aria-label={isPlaying ? ui.pause : ui.play}
          >
            {isPlaying ? <PauseIcon size={18} /> : <PlayIcon size={18} />}
          </button>

          <section className="previewTransportRailBlock" aria-label={ui.timelineTitle}>
            <div
              ref={timelineRailRef}
              className="timelineRail previewTimelineRail"
              style={timelineRailStyle}
              role="slider"
              tabIndex={0}
              aria-label={ui.timelineTitle}
              aria-orientation="horizontal"
              aria-valuemin={0}
              aria-valuemax={normalizedTotalDurationUs}
              aria-valuenow={normalizedCurrentTimeUs}
              aria-valuetext={formatTimelineTime(
                microsecondsToSeconds(normalizedCurrentTimeUs),
                locale,
              )}
              data-editor-interactive="true"
              onKeyDown={handleTimelineKeyDown}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerCancel={onPointerCancel}
            >
              <div
                className="timelineSegments"
                aria-hidden="true"
              >
                {timelineSegmentBuckets.map((bucket) => (
                  <div
                    key={`${bucket.firstFrameIndex}-${bucket.lastFrameIndex}`}
                    className={`timelineSegment${bucket.containsSelected ? "" : " is-dimmed"}${bucket.containsCurrent ? " is-current" : ""}`}
                    style={{
                      flex: `0 0 ${bucketDurationTotalUs > 0
                        ? (bucket.durationUs / bucketDurationTotalUs) * 100
                        : 100 / timelineSegmentBuckets.length}%`,
                    }}
                  />
                ))}
              </div>

              <div className="timelinePlayhead" aria-hidden="true" />
            </div>

            <div className="previewTransportMeta">
              <span>{formatTimelineTime(0, locale)}</span>
              <span>{formatTimelineTime(currentTime, locale)} / {formatTimelineTime(totalDuration, locale)}</span>
              <span>{formatTimelineTime(totalDuration, locale)}</span>
            </div>
          </section>
        </div>
      ) : null}

      <div className="previewZoomBar">
        <span className="previewZoomDockReadout" aria-live="polite">{previewZoomPercent}%</span>
        <button
          className="secondaryAction previewZoomDockStepButton"
          type="button"
          aria-label={copy.previewZoomOut}
          onClick={() => onPreviewZoomStep(-0.1)}
        >
          -
        </button>
        <input
          className="previewZoomSlider previewZoomDockSlider"
          type="range"
          aria-label={copy.previewZoom}
          aria-valuetext={`${previewZoomPercent}%`}
          min={previewZoomSliderMin}
          max={400}
          step={0.5}
          value={previewZoomSliderValue}
          onChange={(event) => onPreviewZoomChange(Number(event.target.value) / 100)}
        />
        <button
          className="secondaryAction previewZoomDockStepButton"
          type="button"
          aria-label={copy.previewZoomIn}
          onClick={() => onPreviewZoomStep(0.1)}
        >
          +
        </button>
        <button
          className={previewZoomMode === "fit" ? "secondaryAction previewZoomDockButton is-active" : "secondaryAction previewZoomDockButton"}
          type="button"
          aria-pressed={previewZoomMode === "fit"}
          onClick={onPreviewZoomFit}
        >
          {copy.previewZoomFit}
        </button>
      </div>
    </section>
  );
}
