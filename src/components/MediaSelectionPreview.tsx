import {
  type CSSProperties,
  type DragEvent,
  type PointerEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { MessagesForLocale } from "../locales/messages";
import {
  MIN_CROP_REGION_RATIO,
  clampUnit,
  cropRegionFromAnchor,
  cropRegionFromStartPoint,
  isFullSelection,
  nextCropRegionFromDrag,
  selectionSummary,
  type CropRegion,
  type PreviewKind,
  type PreviewZoomMode,
} from "./mediaSelectionMath";

export {
  CROP_ASPECT_RATIO_PRESETS,
  FULL_CROP_REGION,
  clampPreviewZoomScale,
  constrainCropRegionToAspectRatio,
  cropAspectRatioValue,
  cropRegionsMatch,
  previewKindForPath,
  selectionSummary,
} from "./mediaSelectionMath";
export type {
  CropAspectRatioPreset,
  CropRegion,
  PreviewKind,
  PreviewZoomMode,
} from "./mediaSelectionMath";

type PreviewDragState = {
  kind: "create" | "move" | "resize";
  pointerId: number;
  startX: number;
  startY: number;
  initialRegion: CropRegion;
  resizeHandle?: "nw" | "ne" | "sw" | "se";
};

type PreviewFrameStyle = CSSProperties & {
  width?: string;
  height?: string;
};

type MediaSelectionPreviewProps = {
  previewSrc: string;
  framePreviewSrc?: string | null;
  requiresFramePreview?: boolean;
  previewKind: PreviewKind;
  sourceWidth: number | null;
  sourceHeight: number | null;
  cropRegion: CropRegion;
  lockedAspectRatio?: number | null;
  onCropRegionChange: (nextRegion: CropRegion) => void;
  onResetSelection: () => void;
  copy: MessagesForLocale;
  isPlaying?: boolean;
  currentTime?: number;
  onCurrentTimeChange?: (value: number) => void;
  onDurationChange?: (value: number) => void;
  syncVideoTimeToParent?: boolean;
  showDetails?: boolean;
  previewZoomMode: PreviewZoomMode;
  manualZoomScale: number;
  onResolvedZoomChange?: (nextZoom: { effectiveScale: number; fitScale: number }) => void;
};

export function MediaSelectionPreview({
  previewSrc,
  framePreviewSrc = null,
  requiresFramePreview = false,
  previewKind,
  sourceWidth,
  sourceHeight,
  cropRegion,
  lockedAspectRatio = null,
  onCropRegionChange,
  onResetSelection,
  copy,
  isPlaying,
  currentTime,
  onCurrentTimeChange,
  onDurationChange,
  syncVideoTimeToParent = true,
  showDetails = true,
  previewZoomMode,
  manualZoomScale,
  onResolvedZoomChange,
}: MediaSelectionPreviewProps) {
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [dragState, setDragState] = useState<PreviewDragState | null>(null);
  const [previewFailed, setPreviewFailed] = useState(false);
  const [stageViewportWidth, setStageViewportWidth] = useState<number | null>(null);
  const [stageViewportHeight, setStageViewportHeight] = useState<number | null>(null);
  const [scrollState, setScrollState] = useState<{ scrollX: number, scrollY: number, viewWidth: number, viewHeight: number, fullWidth: number, fullHeight: number } | null>(null);
  const [minimapDragState, setMinimapDragState] = useState<{ startX: number; startY: number; startScrollX: number; startScrollY: number } | null>(null);

  const handleNativeDragStart = useCallback((event: DragEvent<HTMLElement>) => {
    event.preventDefault();
  }, []);

  const updateScrollState = useCallback(() => {
    if (stageRef.current) {
      setScrollState({
        scrollX: stageRef.current.scrollLeft,
        scrollY: stageRef.current.scrollTop,
        viewWidth: stageRef.current.clientWidth,
        viewHeight: stageRef.current.clientHeight,
        fullWidth: stageRef.current.scrollWidth,
        fullHeight: stageRef.current.scrollHeight,
      });
    }
  }, []);

  const selectionEnabled = sourceWidth !== null && sourceHeight !== null;
  const isControlledVideo = previewKind === "video" && typeof isPlaying === "boolean";
  const shouldRenderVideoFrameCanvas = isControlledVideo && !framePreviewSrc;
  const shouldHideVideoElement = isControlledVideo;

  const drawVideoFrameToCanvas = useCallback(() => {
    if (!shouldRenderVideoFrameCanvas) {
      return;
    }

    const video = videoRef.current;
    const canvas = canvasRef.current;
    if (!video || !canvas || video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA) {
      return;
    }

    const frameWidth = video.videoWidth || sourceWidth || 1;
    const frameHeight = video.videoHeight || sourceHeight || 1;
    if (canvas.width !== frameWidth) {
      canvas.width = frameWidth;
    }
    if (canvas.height !== frameHeight) {
      canvas.height = frameHeight;
    }

    const context = canvas.getContext("2d");
    context?.drawImage(video, 0, 0, frameWidth, frameHeight);
  }, [shouldRenderVideoFrameCanvas, sourceHeight, sourceWidth]);

  const handleMinimapPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!stageRef.current || !scrollState || mediaW === 0 || mediaH === 0) return;
    e.preventDefault();

    const mediaX = Math.max(0, (scrollState.fullWidth - mediaW) / 2);
    const mediaY = Math.max(0, (scrollState.fullHeight - mediaH) / 2);

    const rect = e.currentTarget.getBoundingClientRect();
    const fx = (e.clientX - rect.left) / rect.width;
    const fy = (e.clientY - rect.top) / rect.height;

    const newScrollX = (mediaX + fx * mediaW) - scrollState.viewWidth / 2;
    const newScrollY = (mediaY + fy * mediaH) - scrollState.viewHeight / 2;

    stageRef.current.scrollLeft = newScrollX;
    stageRef.current.scrollTop = newScrollY;

    setMinimapDragState({
      startX: e.clientX,
      startY: e.clientY,
      startScrollX: stageRef.current.scrollLeft,
      startScrollY: stageRef.current.scrollTop,
    });

    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const handleMinimapPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!minimapDragState || !stageRef.current || !scrollState || mediaW === 0 || mediaH === 0) return;
    e.preventDefault();

    const dx = e.clientX - minimapDragState.startX;
    const dy = e.clientY - minimapDragState.startY;

    const rect = e.currentTarget.getBoundingClientRect();
    const scaleX = mediaW / rect.width;
    const scaleY = mediaH / rect.height;

    stageRef.current.scrollLeft = minimapDragState.startScrollX + (dx * scaleX);
    stageRef.current.scrollTop = minimapDragState.startScrollY + (dy * scaleY);
  };

  const handleMinimapPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    setMinimapDragState(null);
    e.currentTarget.releasePointerCapture(e.pointerId);
  };

  useEffect(() => {
    const node = stageRef.current;
    if (!node) {
      return;
    }

    const updateWidthAndScroll = () => {
      setStageViewportWidth(node.clientWidth);
      setStageViewportHeight(node.clientHeight);
      updateScrollState();
    };

    updateWidthAndScroll();

    const observer = new ResizeObserver(() => updateWidthAndScroll());
    observer.observe(node);

    node.addEventListener("scroll", updateScrollState);
    window.addEventListener("resize", updateScrollState);
    return () => {
      observer.disconnect();
      node.removeEventListener("scroll", updateScrollState);
      window.removeEventListener("resize", updateScrollState);
    };
  }, [updateScrollState]);



  const fitZoomScale = useMemo(() => {
    if (!sourceWidth || !sourceHeight) {
      return 1;
    }

    const availableWidth = stageViewportWidth ?? sourceWidth;
    const availableHeight = stageViewportHeight ?? sourceHeight;
    return Math.min(availableWidth / sourceWidth, availableHeight / sourceHeight, 1);
  }, [sourceHeight, sourceWidth, stageViewportHeight, stageViewportWidth]);

  const effectiveZoomScale = previewZoomMode === "fit" ? fitZoomScale : manualZoomScale;
  const mediaW = effectiveZoomScale * (sourceWidth ?? 0);
  const mediaH = effectiveZoomScale * (sourceHeight ?? 0);

  useEffect(() => {
    if (!Number.isFinite(effectiveZoomScale)) {
      return;
    }

    requestAnimationFrame(updateScrollState);
  }, [effectiveZoomScale, updateScrollState]);

  useEffect(() => {
    onResolvedZoomChange?.({
      effectiveScale: effectiveZoomScale,
      fitScale: fitZoomScale,
    });
  }, [effectiveZoomScale, fitZoomScale, onResolvedZoomChange]);

  const mediaFrameStyle = useMemo<PreviewFrameStyle | undefined>(() => {
    if (!sourceWidth || !sourceHeight) {
      return undefined;
    }

    const width = Math.max(1, sourceWidth * effectiveZoomScale);
    const height = Math.max(1, sourceHeight * effectiveZoomScale);

    return {
      width: `${width.toFixed(3)}px`,
      height: `${height.toFixed(3)}px`,
    };
  }, [effectiveZoomScale, sourceHeight, sourceWidth]);

  useEffect(() => {
    if (previewKind !== "video" || currentTime === undefined) {
      return;
    }

    const video = videoRef.current;
    if (!video || Number.isNaN(currentTime)) {
      return;
    }

    if (Math.abs(video.currentTime - currentTime) > 0.005) {
      video.currentTime = currentTime;
    } else {
      requestAnimationFrame(drawVideoFrameToCanvas);
    }

    if (isControlledVideo) {
      video.pause();
    }
  }, [currentTime, drawVideoFrameToCanvas, isControlledVideo, previewKind]);

  useEffect(() => {
    if (previewKind !== "video" || isPlaying === undefined) {
      return;
    }

    const video = videoRef.current;
    if (!video) {
      return;
    }

    if (isControlledVideo) {
      video.pause();
      return;
    }

    if (isPlaying) {
      void video.play().catch(() => {
        // Ignore autoplay interruptions; the transport controls will retry.
      });
      return;
    }

    video.pause();
  }, [isControlledVideo, isPlaying, previewKind]);

  function restartPlayback(video: HTMLVideoElement) {
    video.currentTime = 0;
    onCurrentTimeChange?.(0);
    void video.play().catch(() => {
      // Ignore autoplay interruptions; the transport controls will retry.
    });
  }

  function pointerToRegionPosition(event: PointerEvent<HTMLDivElement>) {
    const bounds = overlayRef.current?.getBoundingClientRect();

    if (!bounds || bounds.width === 0 || bounds.height === 0) {
      return null;
    }

    return {
      x: clampUnit((event.clientX - bounds.left) / bounds.width),
      y: clampUnit((event.clientY - bounds.top) / bounds.height),
    };
  }

  function handlePointerDown(event: PointerEvent<HTMLDivElement>) {
    if (!selectionEnabled || previewFailed || event.button !== 0) {
      return;
    }

    const point = pointerToRegionPosition(event);
    if (!point) {
      return;
    }

    overlayRef.current?.setPointerCapture(event.pointerId);
    const target = event.target as HTMLElement;

    const handle = target.dataset.handle as
      | PreviewDragState["resizeHandle"]
      | undefined;

    if (handle) {
      setDragState({
        kind: "resize",
        pointerId: event.pointerId,
        startX: point.x,
        startY: point.y,
        initialRegion: cropRegion,
        resizeHandle: handle,
      });
      return;
    }

    if (target.closest("[data-selection-box='true']")) {
      setDragState({
        kind: "move",
        pointerId: event.pointerId,
        startX: point.x,
        startY: point.y,
        initialRegion: cropRegion,
      });
      return;
    }

    const nextRegion = cropRegionFromStartPoint(point.x, point.y);

    const initialRegion =
      lockedAspectRatio && sourceWidth && sourceHeight
        ? cropRegionFromAnchor(
          point.x,
          point.y,
          point.x + MIN_CROP_REGION_RATIO,
          point.y + MIN_CROP_REGION_RATIO,
          sourceWidth,
          sourceHeight,
          lockedAspectRatio,
        )
        : nextRegion;

    onCropRegionChange(initialRegion);
    setDragState({
      kind: "create",
      pointerId: event.pointerId,
      startX: point.x,
      startY: point.y,
      initialRegion,
    });
  }

  function handlePointerMove(event: PointerEvent<HTMLDivElement>) {
    if (!dragState || dragState.pointerId !== event.pointerId) {
      return;
    }

    const point = pointerToRegionPosition(event);
    if (!point) {
      return;
    }

    if (dragState.kind === "create") {
      onCropRegionChange(
        nextCropRegionFromDrag({
          dragKind: "create",
          startX: dragState.startX,
          startY: dragState.startY,
          initialRegion: dragState.initialRegion,
          pointX: point.x,
          pointY: point.y,
          lockedAspectRatio,
          sourceWidth,
          sourceHeight,
        }),
      );
      return;
    }

    if (dragState.kind === "move") {
      onCropRegionChange(
        nextCropRegionFromDrag({
          dragKind: "move",
          startX: dragState.startX,
          startY: dragState.startY,
          initialRegion: dragState.initialRegion,
          pointX: point.x,
          pointY: point.y,
          lockedAspectRatio,
          sourceWidth,
          sourceHeight,
        }),
      );
      return;
    }

    if (dragState.resizeHandle) {
      onCropRegionChange(
        nextCropRegionFromDrag({
          dragKind: "resize",
          startX: dragState.startX,
          startY: dragState.startY,
          initialRegion: dragState.initialRegion,
          pointX: point.x,
          pointY: point.y,
          lockedAspectRatio,
          sourceWidth,
          sourceHeight,
          resizeHandle: dragState.resizeHandle,
        }),
      );
    }
  }

  function handlePointerFinish(event: PointerEvent<HTMLDivElement>) {
    if (overlayRef.current?.hasPointerCapture(event.pointerId)) {
      overlayRef.current.releasePointerCapture(event.pointerId);
    }

    setDragState(null);
  }

  const previewFooterText = previewFailed
    ? copy.previewUnavailable
    : [copy.previewHint, copy.previewKeyboardHint]
      .filter((value) => value.trim().length > 0)
      .join(" ");

  return (
    <section className={showDetails ? "previewCard" : "previewCard previewCardStageOnly"}>
      {showDetails ? (
        <div className="previewHeader">
          <div className="previewCopy">
            <p className="panelLabel">{copy.previewSelection}</p>
            <p className="summaryText">{copy.previewSelectionBody}</p>
          </div>

          <div className="previewMetaBlock">
            <div className="previewMetaCopy">
              <span className="metaLabel">{copy.selection}</span>
              <strong className="previewSelectionValue">
                {selectionSummary(cropRegion, copy, sourceWidth, sourceHeight)}
              </strong>
            </div>
            <button
              className="subtleAction compactAction"
              type="button"
              onClick={onResetSelection}
              disabled={isFullSelection(cropRegion)}
            >
              {copy.resetSelection}
            </button>
          </div>
        </div>
      ) : null}

      <div className="previewStageWrapper" style={{ position: "relative", minHeight: 0, height: "100%", width: "100%", overflow: "hidden" }}>
        <div ref={stageRef} className="previewStage">
          <div className="previewStageViewport">
            <div className="previewMediaFrame" style={mediaFrameStyle}>
              {framePreviewSrc ? (
                <img
                  className="previewMedia previewFrameImage"
                  data-preview-frame-image="true"
                  src={framePreviewSrc}
                  alt={copy.previewSelection}
                  decoding="async"
                  draggable={false}
                  onDragStart={handleNativeDragStart}
                  onError={() => setPreviewFailed(true)}
                />
              ) : null}

              {previewKind === "video" ? (
                <>
                  {shouldRenderVideoFrameCanvas ? (
                    <canvas
                      ref={canvasRef}
                      className="previewMedia previewFrameCanvas"
                      data-preview-frame-canvas="true"
                      aria-label={copy.previewSelection}
                    />
                  ) : null}
                  <video
                    ref={videoRef}
                    className={shouldHideVideoElement ? "previewVideoSource" : "previewMedia"}
                    data-preview-video-source={shouldHideVideoElement ? "true" : undefined}
                    src={previewSrc}
                    draggable={false}
                    autoPlay={!isControlledVideo}
                    loop={!isControlledVideo}
                    muted
                    playsInline
                    onDragStart={handleNativeDragStart}
                    onLoadedMetadata={(event) => {
                      onDurationChange?.(event.currentTarget.duration);
                      if (currentTime !== undefined && isControlledVideo) {
                        event.currentTarget.currentTime = currentTime;
                        event.currentTarget.pause();
                      }
                    }}
                    onLoadedData={drawVideoFrameToCanvas}
                    onSeeked={drawVideoFrameToCanvas}
                    onTimeUpdate={(event) => {
                      const nextTime = event.currentTarget.currentTime;
                      const loopEnd = event.currentTarget.duration;

                      if (
                        !isControlledVideo &&
                        isPlaying &&
                        loopEnd > 0.02 &&
                        nextTime >= loopEnd - 0.02
                      ) {
                        restartPlayback(event.currentTarget);
                        return;
                      }

                      if (syncVideoTimeToParent) {
                        onCurrentTimeChange?.(nextTime);
                      }
                    }}
                    onEnded={(event) => {
                      if (!isControlledVideo && isPlaying) {
                        restartPlayback(event.currentTarget);
                        return;
                      }

                      if (syncVideoTimeToParent) {
                        onCurrentTimeChange?.(event.currentTarget.duration);
                      }
                    }}
                    onError={() => setPreviewFailed(true)}
                  />
                </>
              ) : framePreviewSrc ? null : requiresFramePreview ? (
                <div
                  className="previewMedia previewFramePending"
                  data-preview-frame-pending="true"
                  aria-hidden="true"
                />
              ) : (
                <img
                  className="previewMedia"
                  src={previewSrc}
                  alt={copy.previewSelection}
                  decoding="async"
                  draggable={false}
                  onDragStart={handleNativeDragStart}
                  onError={() => setPreviewFailed(true)}
                />
              )}

              {!previewFailed ? (
                <div
                  ref={overlayRef}
                  className={
                    selectionEnabled
                      ? "previewOverlay"
                      : "previewOverlay previewOverlayDisabled"
                  }
                  onPointerDown={handlePointerDown}
                  onPointerMove={handlePointerMove}
                  onPointerUp={handlePointerFinish}
                  onPointerCancel={handlePointerFinish}
                >
                  {selectionEnabled ? (
                    <fieldset
                      className="selectionBox"
                      data-selection-box="true"
                      aria-label={copy.selectionRegionLabel}
                      style={{
                        left: `${cropRegion.x * 100}%`,
                        top: `${cropRegion.y * 100}%`,
                        width: `${cropRegion.width * 100}%`,
                        height: `${cropRegion.height * 100}%`,
                      }}
                    >
                      <div className="selectionGrid" aria-hidden="true" />
                      <div className="selectionCrosshair" aria-hidden="true" />
                      <button
                        className="selectionCorner selectionCornerTopLeft selectionHandle"
                        data-handle="nw"
                        type="button"
                        tabIndex={-1}
                        aria-label={copy.selectionHandleTopLeft}
                      />
                      <button
                        className="selectionCorner selectionCornerTopRight selectionHandle"
                        data-handle="ne"
                        type="button"
                        tabIndex={-1}
                        aria-label={copy.selectionHandleTopRight}
                      />
                      <button
                        className="selectionCorner selectionCornerBottomLeft selectionHandle"
                        data-handle="sw"
                        type="button"
                        tabIndex={-1}
                        aria-label={copy.selectionHandleBottomLeft}
                      />
                      <button
                        className="selectionCorner selectionCornerBottomRight selectionHandle"
                        data-handle="se"
                        type="button"
                        tabIndex={-1}
                        aria-label={copy.selectionHandleBottomRight}
                      />
                    </fieldset>
                  ) : null}
                </div>
              ) : null}
            </div>
          </div>
        </div>

        {scrollState && mediaW > 0 && mediaH > 0 && (scrollState.fullWidth > scrollState.viewWidth + 2 || scrollState.fullHeight > scrollState.viewHeight + 2) ? (() => {
          const mediaX = Math.max(0, (scrollState.fullWidth - mediaW) / 2);
          const mediaY = Math.max(0, (scrollState.fullHeight - mediaH) / 2);
          return (
            <div className="previewMinimap">
              <div
                className="previewMinimapInner"
                style={{ aspectRatio: `${mediaW} / ${mediaH}` }}
                onDragStart={handleNativeDragStart}
                onPointerDown={handleMinimapPointerDown}
                onPointerMove={handleMinimapPointerMove}
                onPointerUp={handleMinimapPointerUp}
                onPointerCancel={handleMinimapPointerUp}
              >
                <div className="previewMinimapMediaWrapper">
                  {previewKind === "video" ? (
                    <video
                      className="previewMedia"
                      src={`${previewSrc}#t=0.01`}
                      draggable={false}
                      preload="metadata"
                      muted
                      playsInline
                      onDragStart={handleNativeDragStart}
                      style={{ objectFit: 'contain' }}
                    />
                  ) : (
                    <img
                      className="previewMedia"
                      src={previewSrc}
                      alt={copy.previewSelection}
                      draggable={false}
                      onDragStart={handleNativeDragStart}
                      style={{ objectFit: 'contain' }}
                      decoding="async"
                    />
                  )}
                </div>
                <div className="previewMinimapViewport" style={{
                  left: `${((scrollState.scrollX - mediaX) / mediaW) * 100}%`,
                  top: `${((scrollState.scrollY - mediaY) / mediaH) * 100}%`,
                  width: `${(scrollState.viewWidth / mediaW) * 100}%`,
                  height: `${(scrollState.viewHeight / mediaH) * 100}%`,
                }} />
              </div>
            </div>
          );
        })() : null}
      </div>

      {previewFooterText ? (
        <div className="previewFooter">
          <p className="detailText">{previewFooterText}</p>
        </div>
      ) : null}
    </section>
  );
}




