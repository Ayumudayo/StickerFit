import type {
  CSSProperties,
  KeyboardEventHandler,
  MouseEvent,
  PointerEvent,
  RefObject,
} from "react";

import type { EditorText } from "../../locales/editorText";
import type { Locale } from "../../locales/messages";
import type {
  FrameDropTargetState,
  FrameReorderState,
  FrameSelectionModel,
  TimelineFrameView,
} from "../../types/editor";
import { formatTimelineTime } from "../../utils/timelineFrames";
import { GridIcon } from "../AppIcons";

type FrameRailProps = {
  ui: EditorText;
  locale: Locale;
  timelineFrameViews: TimelineFrameView[];
  selection: FrameSelectionModel;
  currentFrameInstanceId: string | null;
  hasClipboardFrames: boolean;
  frameDropTarget: FrameDropTargetState | null;
  frameReorderState: FrameReorderState | null;
  frameTableBodyRef: RefObject<HTMLDivElement | null>;
  onFramePointerDown: (instanceId: string, event: PointerEvent<HTMLButtonElement>) => void;
  onFramePointerEnter: (instanceId: string) => void;
  onFrameContextMenu: (instanceId: string, event: MouseEvent<HTMLButtonElement>) => void;
  onFrameKeyDown: KeyboardEventHandler<HTMLButtonElement>;
  onFrameFocus: (time: number) => void;
  onPasteFramesBelow: () => void;
};

export function FrameRail({
  ui,
  locale,
  timelineFrameViews,
  selection,
  currentFrameInstanceId,
  hasClipboardFrames,
  frameDropTarget,
  frameReorderState,
  frameTableBodyRef,
  onFramePointerDown,
  onFramePointerEnter,
  onFrameContextMenu,
  onFrameKeyDown,
  onFrameFocus,
  onPasteFramesBelow,
}: FrameRailProps) {
  return (
    <aside className="frameRail">
      <section className="appCard frameCard frameRailCard">
        <div className="cardHeading">
          <h3>
            <GridIcon size={16} className="cardHeadingIcon" />
            {ui.frameTitle}
          </h3>
        </div>

        {timelineFrameViews.length > 0 ? (
          <>
            <div className="frameTable frameTableHeader">
              <div>{ui.frameNumber}</div>
              <div>{ui.frameTime}</div>
            </div>

            <div
              ref={frameTableBodyRef}
              className="frameTableBody"
              role="listbox"
              aria-multiselectable="true"
              aria-label={ui.frameTitle}
            >
              {timelineFrameViews.map((frame) => {
                const isSelected = selection.selectedInstanceIdSet.has(frame.instanceId);
                const isCurrent = frame.instanceId === currentFrameInstanceId;
                const isDropTarget = frameDropTarget?.anchorInstanceId === frame.instanceId;
                const isDragged =
                  frameReorderState?.active === true &&
                  frameReorderState.draggedInstanceIds.includes(frame.instanceId);
                const dragOffsetY = isDragged && frameReorderState
                  ? frameReorderState.currentY - frameReorderState.startY
                  : 0;

                return (
                  <button
                    key={frame.instanceId}
                    className={`frameRow ${isSelected ? "is-selected" : ""} ${isCurrent ? "is-current" : ""} ${isDropTarget ? `is-drop-${frameDropTarget?.position}` : ""} ${isDragged ? "is-dragged" : ""}`}
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    data-instance-id={frame.instanceId}
                    aria-grabbed={isDragged}
                    style={
                      isDragged
                        ? ({ "--frame-row-drag-y": `${dragOffsetY}px` } as CSSProperties)
                        : undefined
                    }
                    onPointerDown={(event) => onFramePointerDown(frame.instanceId, event)}
                    onPointerEnter={() => onFramePointerEnter(frame.instanceId)}
                    onContextMenu={(event) => onFrameContextMenu(frame.instanceId, event)}
                    onKeyDown={onFrameKeyDown}
                    onFocus={() => onFrameFocus(frame.startTimeSeconds)}
                  >
                    <div className="frameCell frameIdCell">
                      <span className="frameCellLabel">{ui.frameNumber}</span>
                      <span className="frameValueGroup">
                        <span>{frame.displayNumber}</span>
                        {isCurrent ? <span className="frameDot" /> : null}
                      </span>
                    </div>
                    <div className="frameCell frameTimeCell">
                      <span className="frameCellLabel">{ui.frameTime}</span>
                      <span className="frameMono">
                        {formatTimelineTime(frame.durationSeconds, locale)}
                      </span>
                    </div>
                  </button>
                );
              })}
            </div>
            <p className="frameQuickHint frameContextHint">{ui.frameToolbarHint}</p>
          </>
        ) : (
          <div className="frameRailEmptyState">
            <div className="emptyState compactState">
              <p>{ui.noFrames}</p>
            </div>
            {hasClipboardFrames ? (
              <div className="frameQuickActionsWrap">
                <div className="frameQuickActions frameQuickActionsSingle">
                  <button
                    className="secondaryAction frameQuickAction"
                    type="button"
                    onClick={onPasteFramesBelow}
                  >
                    {ui.frameToolbarPasteBelow}
                  </button>
                </div>
                <p className="frameQuickHint">{ui.frameEmptyPasteHint}</p>
              </div>
            ) : null}
          </div>
        )}

      </section>
    </aside>
  );
}
