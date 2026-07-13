import type {
  KeyboardEventHandler,
  MouseEvent,
  PointerEvent,
  ReactElement,
  RefObject,
} from "react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { EditorText } from "../../locales/editorText";
import type { Locale } from "../../locales/messages";
import type {
  FrameDropTargetState,
  FrameReorderState,
  FrameSelectionModel,
  TimelineFrameView,
} from "../../types/editor";
import type { FramePreviewLoadState } from "../../types/workflow";
import { formatTimelineTime } from "../../utils/timelineFrames";
import {
  computeRovingFrameIndex,
  computeScrollTopToRevealIndex,
  computeVirtualWindow,
  FRAME_RAIL_OVERSCAN,
  FRAME_RAIL_ROW_HEIGHT,
  normalizeRovingFrameIndex,
  type FrameRailNavigationKey,
} from "../../utils/virtualFrameList";
import { GridIcon } from "../AppIcons";

type FrameRailProps = {
  ui: EditorText;
  locale: Locale;
  timelineFrameViews: TimelineFrameView[];
  selection: FrameSelectionModel;
  hasClipboardFrames: boolean;
  frameDropTarget: FrameDropTargetState | null;
  frameReorderState: FrameReorderState | null;
  frameTableBodyRef: RefObject<HTMLDivElement | null>;
  activeInstanceId: string | null;
  showFramePreviews: boolean;
  previewEntries: ReadonlyMap<number, FramePreviewLoadState>;
  onVisibleRange: (start: number, end: number) => void;
  onRetryFramePreview: (sourceFrameId: number) => void;
  onFramePointerDown: (
    instanceId: string,
    event: PointerEvent<HTMLButtonElement>,
  ) => void;
  onFrameContextMenu: (
    instanceId: string,
    event: MouseEvent<HTMLButtonElement>,
  ) => void;
  onFrameKeyDown: KeyboardEventHandler<HTMLButtonElement>;
  onFrameFocus: (time: number) => void;
  onPasteFramesBelow: () => void;
};

type FramePreviewRetryLayerProps = {
  ui: EditorText;
  visibleFrames: TimelineFrameView[];
  previewEntries: ReadonlyMap<number, FramePreviewLoadState>;
  virtualOffsetTop: number;
  scrollTop: number;
  viewportHeight: number;
  onRetryFramePreview: (sourceFrameId: number) => void;
  onRestoreFrameFocus: (instanceId: string) => void;
};

export function FramePreviewRetryLayer({
  ui,
  visibleFrames,
  previewEntries,
  virtualOffsetTop,
  scrollTop,
  viewportHeight,
  onRetryFramePreview,
  onRestoreFrameFocus,
}: FramePreviewRetryLayerProps): ReactElement {
  return (
    <div className="framePreviewRetryLayer">
      {visibleFrames.map((frame, visibleIndex) => {
        const top =
          virtualOffsetTop +
          visibleIndex * FRAME_RAIL_ROW_HEIGHT +
          FRAME_RAIL_ROW_HEIGHT / 2 -
          scrollTop;
        if (
          previewEntries.get(frame.sourceFrameId)?.status !== "error" ||
          top < 0 ||
          top > viewportHeight
        ) {
          return null;
        }

        return (
          <button
            key={frame.instanceId}
            className="framePreviewRetry"
            type="button"
            aria-label={ui.framePreviewRetryLabel(frame.displayNumber)}
            style={{ top }}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.stopPropagation();
              onRestoreFrameFocus(frame.instanceId);
              onRetryFramePreview(frame.sourceFrameId);
            }}
          >
            {ui.framePreviewRetry}
          </button>
        );
      })}
    </div>
  );
}

function isFrameRailNavigationKey(key: string): key is FrameRailNavigationKey {
  return (
    key === "ArrowUp" || key === "ArrowDown" || key === "Home" || key === "End"
  );
}

export function FrameRail({
  ui,
  locale,
  timelineFrameViews,
  selection,
  hasClipboardFrames,
  frameDropTarget,
  frameReorderState,
  frameTableBodyRef,
  activeInstanceId,
  showFramePreviews,
  previewEntries,
  onVisibleRange,
  onRetryFramePreview,
  onFramePointerDown,
  onFrameContextMenu,
  onFrameKeyDown,
  onFrameFocus,
  onPasteFramesBelow,
}: FrameRailProps) {
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(
    FRAME_RAIL_ROW_HEIGHT * 8,
  );
  const [railViewportTop, setRailViewportTop] = useState(0);
  const pendingKeyboardFocusInstanceIdRef = useRef<string | null>(null);
  const draggedInstanceIdSet = useMemo(
    () => new Set(frameReorderState?.draggedInstanceIds ?? []),
    [frameReorderState?.draggedInstanceIds],
  );
  const virtualWindow = useMemo(
    () =>
      computeVirtualWindow({
        scrollTop,
        viewportHeight,
        rowHeight: FRAME_RAIL_ROW_HEIGHT,
        itemCount: timelineFrameViews.length,
        overscan: FRAME_RAIL_OVERSCAN,
      }),
    [scrollTop, timelineFrameViews.length, viewportHeight],
  );
  const visibleFrames = timelineFrameViews.slice(
    virtualWindow.start,
    virtualWindow.end,
  );
  const activeIndex = activeInstanceId
    ? timelineFrameViews.findIndex(
        (frame) => frame.instanceId === activeInstanceId,
      )
    : -1;
  const rovingActiveIndex = normalizeRovingFrameIndex(
    activeIndex,
    timelineFrameViews.length,
  );
  const renderedTabStopIndex =
    rovingActiveIndex >= virtualWindow.start &&
    rovingActiveIndex < virtualWindow.end
      ? rovingActiveIndex
      : virtualWindow.start;
  const dragPreviewFrame = frameReorderState?.active
    ? (timelineFrameViews.find(
        (frame) => frame.instanceId === frameReorderState.draggedInstanceIds[0],
      ) ?? null)
    : null;
  const dragOverlayTop = frameReorderState?.active
    ? scrollTop +
      Math.max(
        0,
        Math.min(
          Math.max(0, viewportHeight - FRAME_RAIL_ROW_HEIGHT),
          frameReorderState.currentY -
            railViewportTop -
            FRAME_RAIL_ROW_HEIGHT / 2,
        ),
      )
    : 0;
  useLayoutEffect(() => {
    const element = frameTableBodyRef.current;
    if (!element) return;

    const updateMetrics = () => {
      setScrollTop(element.scrollTop);
      setViewportHeight(Math.max(1, element.clientHeight));
      setRailViewportTop(element.getBoundingClientRect().top);
    };
    updateMetrics();
    element.addEventListener("scroll", updateMetrics, { passive: true });
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(updateMetrics);
    observer?.observe(element);
    return () => {
      element.removeEventListener("scroll", updateMetrics);
      observer?.disconnect();
    };
  }, [frameTableBodyRef, timelineFrameViews.length]);

  useEffect(() => {
    onVisibleRange(virtualWindow.start, virtualWindow.end);
  }, [onVisibleRange, virtualWindow.end, virtualWindow.start]);

  useLayoutEffect(() => {
    const element = frameTableBodyRef.current;
    if (!element || activeIndex < 0) return;
    const nextScrollTop = computeScrollTopToRevealIndex({
      scrollTop: element.scrollTop,
      viewportHeight: element.clientHeight,
      rowHeight: FRAME_RAIL_ROW_HEIGHT,
      itemCount: timelineFrameViews.length,
      index: activeIndex,
    });
    if (nextScrollTop !== element.scrollTop) {
      element.scrollTop = nextScrollTop;
      setScrollTop(nextScrollTop);
    }
  }, [activeIndex, frameTableBodyRef, timelineFrameViews.length]);

  useLayoutEffect(() => {
    const element = frameTableBodyRef.current;
    const pendingInstanceId = pendingKeyboardFocusInstanceIdRef.current;
    const pendingIndex = pendingInstanceId
      ? timelineFrameViews.findIndex(
          (frame) => frame.instanceId === pendingInstanceId,
        )
      : -1;
    if (pendingInstanceId && pendingIndex < 0) {
      pendingKeyboardFocusInstanceIdRef.current = null;
      return;
    }
    if (
      !element ||
      !pendingInstanceId ||
      pendingIndex < virtualWindow.start ||
      pendingIndex >= virtualWindow.end
    ) {
      return;
    }
    const target = Array.from(
      element.querySelectorAll<HTMLButtonElement>("[data-instance-id]"),
    ).find((row) => row.dataset.instanceId === pendingInstanceId);
    if (target && document.activeElement !== target) {
      target.focus({ preventScroll: true });
    }
    if (target) {
      pendingKeyboardFocusInstanceIdRef.current = null;
    }
  }, [
    activeInstanceId,
    frameTableBodyRef,
    timelineFrameViews,
    virtualWindow.end,
    virtualWindow.start,
  ]);

  const handleFrameRowKeyDown: KeyboardEventHandler<HTMLButtonElement> = (
    event,
  ) => {
    if (
      !event.defaultPrevented &&
      !event.nativeEvent.isComposing &&
      !event.ctrlKey &&
      !event.altKey &&
      !event.metaKey &&
      isFrameRailNavigationKey(event.key)
    ) {
      const currentIndex = Number(event.currentTarget.dataset.frameIndex);
      if (Number.isInteger(currentIndex)) {
        const targetIndex = computeRovingFrameIndex({
          activeIndex: currentIndex,
          itemCount: timelineFrameViews.length,
          key: event.key,
        });
        const targetInstanceId =
          timelineFrameViews[targetIndex]?.instanceId ?? null;
        if (
          targetInstanceId !== null &&
          targetInstanceId !== event.currentTarget.dataset.instanceId
        ) {
          pendingKeyboardFocusInstanceIdRef.current = targetInstanceId;
          const element = frameTableBodyRef.current;
          if (element) {
            const nextScrollTop = computeScrollTopToRevealIndex({
              scrollTop: element.scrollTop,
              viewportHeight: element.clientHeight,
              rowHeight: FRAME_RAIL_ROW_HEIGHT,
              itemCount: timelineFrameViews.length,
              index: targetIndex,
            });
            if (nextScrollTop !== element.scrollTop) {
              element.scrollTop = nextScrollTop;
              setScrollTop(nextScrollTop);
            }
          }
        }
      }
    }

    onFrameKeyDown(event);
  };

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
            <div
              className={`frameTable frameTableHeader ${showFramePreviews ? "with-preview" : ""}`}
            >
              {showFramePreviews ? <div aria-hidden="true" /> : null}
              <div>{ui.frameNumber}</div>
              <div>{ui.frameTime}</div>
            </div>

            <div className="frameTableBodyShell">
              <div
                ref={frameTableBodyRef}
                className="frameTableBody"
                role="listbox"
                aria-multiselectable="true"
                aria-label={ui.frameTitle}
              >
                <div
                  className="frameVirtualSpacer"
                  style={{
                    height: timelineFrameViews.length * FRAME_RAIL_ROW_HEIGHT,
                  }}
                >
                  <div
                    className="frameVirtualRows"
                    style={{
                      transform: `translateY(${virtualWindow.offsetTop}px)`,
                    }}
                  >
                    {visibleFrames.map((frame, visibleIndex) => {
                      const frameIndex = virtualWindow.start + visibleIndex;
                      const isSelected = selection.selectedInstanceIdSet.has(
                        frame.instanceId,
                      );
                      const isDropTarget =
                        frameDropTarget?.anchorInstanceId === frame.instanceId;
                      const isDragged =
                        frameReorderState?.active === true &&
                        draggedInstanceIdSet.has(frame.instanceId);
                      const previewEntry: FramePreviewLoadState =
                        previewEntries.get(frame.sourceFrameId) ?? {
                          status: "idle",
                        };
                      const hasPreviewError =
                        showFramePreviews && previewEntry.status === "error";

                      return (
                        <div
                          key={frame.instanceId}
                          className={`frameVirtualItem ${hasPreviewError ? "has-preview-error" : ""}`}
                          style={{ height: FRAME_RAIL_ROW_HEIGHT }}
                        >
                          <button
                            className={`frameRow ${showFramePreviews ? "with-preview" : ""} ${isSelected ? "is-selected" : ""} ${isDropTarget ? `is-drop-${frameDropTarget?.position}` : ""} ${isDragged ? "is-dragged" : ""}`}
                            type="button"
                            role="option"
                            aria-selected={isSelected}
                            aria-setsize={timelineFrameViews.length}
                            aria-posinset={frameIndex + 1}
                            tabIndex={
                              frameIndex === renderedTabStopIndex ? 0 : -1
                            }
                            data-instance-id={frame.instanceId}
                            data-frame-index={frameIndex}
                            aria-grabbed={isDragged}
                            onPointerDown={(event) =>
                              onFramePointerDown(frame.instanceId, event)
                            }
                            onContextMenu={(event) =>
                              onFrameContextMenu(frame.instanceId, event)
                            }
                            onKeyDown={handleFrameRowKeyDown}
                            onFocus={() => onFrameFocus(frame.startTimeSeconds)}
                          >
                            {showFramePreviews ? (
                              <span
                                className={`framePreviewCell is-${previewEntry.status}`}
                                title={
                                  previewEntry.status === "error"
                                    ? previewEntry.message
                                    : undefined
                                }
                              >
                                {previewEntry.status === "ready" ? (
                                  <img
                                    src={previewEntry.dataUrl}
                                    alt=""
                                    aria-hidden="true"
                                  />
                                ) : previewEntry.status === "loading" ? (
                                  <span className="framePreviewStatus">
                                    {ui.framePreviewLoading}
                                  </span>
                                ) : previewEntry.status === "error" ? (
                                  <span className="framePreviewStatus">
                                    {ui.framePreviewUnavailable}
                                  </span>
                                ) : (
                                  <span
                                    className="framePreviewPlaceholder"
                                    aria-hidden="true"
                                  />
                                )}
                              </span>
                            ) : null}
                            <div className="frameCell frameIdCell">
                              <span className="frameCellLabel">
                                {ui.frameNumber}
                              </span>
                              <span className="frameValueGroup">
                                <span>{frame.displayNumber}</span>
                              </span>
                            </div>
                            <div className="frameCell frameTimeCell">
                              <span className="frameCellLabel">
                                {ui.frameTime}
                              </span>
                              <span className="frameMono">
                                {formatTimelineTime(
                                  frame.durationSeconds,
                                  locale,
                                )}
                              </span>
                            </div>
                          </button>
                        </div>
                      );
                    })}
                  </div>
                  {dragPreviewFrame && frameReorderState?.active ? (
                    <div
                      className="frameDragOverlay"
                      style={{
                        top: dragOverlayTop,
                        height: FRAME_RAIL_ROW_HEIGHT,
                      }}
                      aria-hidden="true"
                    >
                      <span>
                        {ui.frameNumber} {dragPreviewFrame.displayNumber}
                      </span>
                      {frameReorderState.draggedInstanceIds.length > 1 ? (
                        <span>
                          +{frameReorderState.draggedInstanceIds.length - 1}
                        </span>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              </div>
              {showFramePreviews ? (
                <FramePreviewRetryLayer
                  ui={ui}
                  visibleFrames={visibleFrames}
                  previewEntries={previewEntries}
                  virtualOffsetTop={virtualWindow.offsetTop}
                  scrollTop={scrollTop}
                  viewportHeight={viewportHeight}
                  onRetryFramePreview={onRetryFramePreview}
                  onRestoreFrameFocus={(instanceId) => {
                    const target = Array.from(
                      frameTableBodyRef.current?.querySelectorAll<HTMLButtonElement>(
                        "[data-instance-id]",
                      ) ?? [],
                    ).find((row) => row.dataset.instanceId === instanceId);
                    target?.focus({ preventScroll: true });
                  }}
                />
              ) : null}
            </div>
            <p className="frameQuickHint frameContextHint">
              {ui.frameToolbarHint}
            </p>
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
