import type { CSSProperties, PointerEventHandler, RefObject } from "react";

import type { MessagesForLocale, Locale } from "../../locales/messages";
import type { EditorText } from "../../locales/editorText";
import type { PreviewZoomMode } from "../../components/MediaSelectionPreview";
import type { FrameSelectionModel, TimelineFrameView } from "../../types/editor";
import { formatTimelineTime } from "../../utils/timelineFrames";
import { PauseIcon, PlayIcon } from "../AppIcons";

type PreviewControlBarProps = {
  copy: MessagesForLocale;
  ui: EditorText;
  locale: Locale;
  isStaticImage: boolean;
  currentTime: number;
  totalDuration: number;
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
  onPreviewZoomFit,
  onPreviewZoomStep,
  onPreviewZoomChange,
}: PreviewControlBarProps) {
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
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerCancel={onPointerCancel}
            >
              <div className="timelineSegments" aria-hidden="true">
                {timelineFrameViews.map((frame) => (
                  <div
                    key={frame.instanceId}
                    className={
                      selection.selectedInstanceIdSet.has(frame.instanceId)
                        ? "timelineSegment"
                        : "timelineSegment is-dimmed"
                    }
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
