import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useFrameDialogState } from "./frameEditor/useFrameDialogState";
import { useFrameSelectionInteractions } from "./frameEditor/useFrameSelectionInteractions";
import type { SourceFrame, TimelineFrame } from "../types/editor";
import {
  clearTimelineSelection,
  copySelectedFrames,
  copySelectedFramesToBoundary,
  cutSelectedFrames,
  deleteUnselectedTimelineFrames,
  pasteClipboardFrames,
  reverseSelectedFramesInPlace,
  scaleSelectedFrameDurations,
  splitSelectedFrame,
} from "../utils/frameEditing";
import { buildInitialTimelineFrames, buildTimelineFrameViews } from "../utils/timelineFrames";

type UseFrameEditorControllerParams = {
  editorSessionKey?: number;
  sourceFrames: SourceFrame[];
  minDurationUs: number;
};

export function useFrameEditorController({
  editorSessionKey,
  sourceFrames,
  minDurationUs,
}: UseFrameEditorControllerParams) {
  const [timelineFrames, setTimelineFrames] = useState<TimelineFrame[]>([]);
  const [clipboardFrames, setClipboardFrames] = useState<TimelineFrame[]>([]);

  const frameInstanceCounterRef = useRef(0);
  const hydratedSessionKeyRef = useRef<number | null>(null);

  const timelineFrameViews = useMemo(
    () => buildTimelineFrameViews(timelineFrames, sourceFrames),
    [sourceFrames, timelineFrames],
  );

  const selection = useFrameSelectionInteractions({
    timelineFrames,
    timelineFrameViews,
    setTimelineFrames,
  });
  const {
    selectedInstanceIds,
    setSelectedInstanceIds,
    selectionModel,
    hasSingleFrameSelection,
    canDeleteUnselectedFrames,
    frameContextMenu,
    setFrameContextMenu,
    frameDropTarget,
    frameReorderState,
    frameTableBodyRef,
    frameContextMenuRef,
    resetSelectionState,
    handleFramePointerDown,
    handleFrameKeyDown,
    handleFrameContextMenu,
    selectSingleFrame,
    selectAdjacentFrame,
    selectAllFrames,
    clearAllFrames,
    selectOddFrames,
    selectEvenFrames,
    invertFrameSelection,
    moveSelectedFrames,
    moveSelectedFramesTo,
  } = selection;

  const dialog = useFrameDialogState({
    timelineFrames,
    selectedInstanceIds,
    frameContextMenu,
    minDurationUs,
    setTimelineFrames,
    setSelectedInstanceIds,
    closeFrameContextMenu: () => setFrameContextMenu(null),
  });
  const {
    frameDurationDialog,
    frameDurationMode,
    setFrameDurationMode,
    frameDurationSecondsValue,
    frameDurationFpsValue,
    hasMixedSelectedDurations,
    frameDurationDialogRef,
    nthSelectionDialogRef,
    nthSelectionStep,
    setNthSelectionStep,
    showNthSelectionDialog,
    resetDialogState,
    openFrameDurationDialog,
    applyDurationChange,
    updateFrameDurationFromSeconds,
    updateFrameDurationFromFps,
    openNthSelectionDialog,
    applyNthFrameSelection,
    closeFrameDurationDialog,
    closeNthSelectionDialog,
  } = dialog;

  const hasClipboardFrames = clipboardFrames.length > 0;
  const anchorSourceFrameId =
    timelineFrames.find((frame) => frame.instanceId === selectedInstanceIds[0])
      ?.sourceFrameId ?? timelineFrames[0]?.sourceFrameId ?? 0;

  const createTimelineInstanceId = useCallback((sourceFrameId: number) => {
    frameInstanceCounterRef.current += 1;
    return `frame-${sourceFrameId}-${frameInstanceCounterRef.current}`;
  }, []);

  const resetFrameEditorState = useCallback(() => {
    setTimelineFrames([]);
    setClipboardFrames([]);
    resetSelectionState();
    resetDialogState();
    frameInstanceCounterRef.current = 0;
  }, [resetDialogState, resetSelectionState]);

  useLayoutEffect(() => {
    if (editorSessionKey === undefined) {
      frameInstanceCounterRef.current = 0;
      setTimelineFrames(buildInitialTimelineFrames(sourceFrames));
      resetSelectionState();
      resetDialogState();
      hydratedSessionKeyRef.current = null;
      return;
    }

    if (hydratedSessionKeyRef.current === editorSessionKey) {
      return;
    }

    resetFrameEditorState();
    if (sourceFrames.length === 0) {
      return;
    }

    frameInstanceCounterRef.current = 0;
    setTimelineFrames(buildInitialTimelineFrames(sourceFrames));
    hydratedSessionKeyRef.current = editorSessionKey;
  }, [editorSessionKey, resetDialogState, resetFrameEditorState, resetSelectionState, sourceFrames]);

  const deleteUnselectedFrames = useCallback(() => {
    if (selectedInstanceIds.length === 0) {
      return;
    }

    setTimelineFrames((current) => deleteUnselectedTimelineFrames(current, selectedInstanceIds));
    setFrameContextMenu(null);
  }, [selectedInstanceIds, setFrameContextMenu]);

  const speedAdjustSelectedFrames = useCallback(
    (factor: number) => {
      setTimelineFrames((current) =>
        scaleSelectedFrameDurations(current, selectedInstanceIds, factor, minDurationUs),
      );
      setFrameContextMenu(null);
    },
    [minDurationUs, selectedInstanceIds, setFrameContextMenu],
  );

  const splitCurrentFrame = useCallback(() => {
    const result = splitSelectedFrame(
      timelineFrames,
      selectedInstanceIds,
      () => createTimelineInstanceId(anchorSourceFrameId),
      minDurationUs,
    );
    if (!result) {
      return;
    }

    setTimelineFrames(result.timelineFrames);
    setSelectedInstanceIds(result.selectedInstanceIds);
    setFrameContextMenu(null);
  }, [
    anchorSourceFrameId,
    createTimelineInstanceId,
    minDurationUs,
    selectedInstanceIds,
    setFrameContextMenu,
    setSelectedInstanceIds,
    timelineFrames,
  ]);

  const copySelectedFramesTo = useCallback(
    (boundary: "start" | "end") => {
      const nextFrames = copySelectedFramesToBoundary(
        timelineFrames,
        selectedInstanceIds,
        boundary,
        () => createTimelineInstanceId(anchorSourceFrameId),
      );
      setTimelineFrames(nextFrames);
      setSelectedInstanceIds(
        boundary === "start"
          ? nextFrames.slice(0, selectedInstanceIds.length).map((frame) => frame.instanceId)
          : nextFrames.slice(-selectedInstanceIds.length).map((frame) => frame.instanceId),
      );
      setFrameContextMenu(null);
    },
    [
      anchorSourceFrameId,
      createTimelineInstanceId,
      selectedInstanceIds,
      setFrameContextMenu,
      setSelectedInstanceIds,
      timelineFrames,
    ],
  );

  const reverseSelectedFrames = useCallback(() => {
    setTimelineFrames((current) => reverseSelectedFramesInPlace(current, selectedInstanceIds));
    setFrameContextMenu(null);
  }, [selectedInstanceIds, setFrameContextMenu]);

  const copyFramesToClipboard = useCallback(() => {
    setClipboardFrames(copySelectedFrames(timelineFrames, selectedInstanceIds));
    setFrameContextMenu(null);
  }, [selectedInstanceIds, setFrameContextMenu, timelineFrames]);

  const cutFramesToClipboard = useCallback(() => {
    const result = cutSelectedFrames(timelineFrames, selectedInstanceIds);
    setClipboardFrames(result.clipboardFrames);
    setTimelineFrames(result.timelineFrames);
    setSelectedInstanceIds(clearTimelineSelection());
    setFrameContextMenu(null);
  }, [selectedInstanceIds, setFrameContextMenu, setSelectedInstanceIds, timelineFrames]);

  const pasteClipboard = useCallback(
    (position: "above" | "below") => {
      if (!frameContextMenu) {
        return;
      }

      const result = pasteClipboardFrames(
        timelineFrames,
        clipboardFrames,
        frameContextMenu.anchorInstanceId,
        position,
        () => createTimelineInstanceId(0),
      );
      if (!result) {
        return;
      }

      setTimelineFrames(result.timelineFrames);
      setSelectedInstanceIds(result.selectedInstanceIds);
      setFrameContextMenu(null);
    },
    [
      clipboardFrames,
      createTimelineInstanceId,
      frameContextMenu,
      setFrameContextMenu,
      setSelectedInstanceIds,
      timelineFrames,
    ],
  );

  const pasteClipboardAtSelection = useCallback(
    (position: "above" | "below") => {
      if (timelineFrames.length === 0) {
        if (clipboardFrames.length === 0) {
          return;
        }

        const restoredFrames = clipboardFrames.map((frame, index) => ({
          ...frame,
          instanceId: createTimelineInstanceId(frame.sourceFrameId),
          displayNumber: index + 1,
        }));

        setTimelineFrames(restoredFrames);
        setSelectedInstanceIds(restoredFrames.map((frame) => frame.instanceId));
        setFrameContextMenu(null);
        return;
      }

      const anchorInstanceId =
        selectedInstanceIds.length > 0
          ? selectedInstanceIds[selectedInstanceIds.length - 1]
          : timelineFrames[position === "below" ? timelineFrames.length - 1 : 0]?.instanceId ?? null;
      if (!anchorInstanceId) {
        return;
      }

      const result = pasteClipboardFrames(
        timelineFrames,
        clipboardFrames,
        anchorInstanceId,
        position,
        () => createTimelineInstanceId(0),
      );
      if (!result) {
        return;
      }

      setTimelineFrames(result.timelineFrames);
      setSelectedInstanceIds(result.selectedInstanceIds);
      setFrameContextMenu(null);
    },
    [
      clipboardFrames,
      createTimelineInstanceId,
      selectedInstanceIds,
      setFrameContextMenu,
      setSelectedInstanceIds,
      timelineFrames,
    ],
  );

  const renumberTimelineFrames = useCallback(() => {
    setTimelineFrames((current) =>
      current.map((frame, index) => ({
        ...frame,
        displayNumber: index + 1,
      })),
    );
    setFrameContextMenu(null);
  }, [setFrameContextMenu]);

  useEffect(() => {
    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (
        frameContextMenuRef.current &&
        target &&
        !frameContextMenuRef.current.contains(target)
      ) {
        setFrameContextMenu(null);
      }
      if (
        frameDurationDialogRef.current &&
        target &&
        !frameDurationDialogRef.current.contains(target)
      ) {
        closeFrameDurationDialog();
      }
      if (
        nthSelectionDialogRef.current &&
        target &&
        !nthSelectionDialogRef.current.contains(target)
      ) {
        closeNthSelectionDialog();
      }
    };

    window.addEventListener("mousedown", handlePointerDown);
    return () => window.removeEventListener("mousedown", handlePointerDown);
  }, [
    closeFrameDurationDialog,
    closeNthSelectionDialog,
    frameContextMenuRef,
    frameDurationDialogRef,
    nthSelectionDialogRef,
    setFrameContextMenu,
  ]);

  return {
    timelineFrames,
    timelineFrameViews,
    selectedInstanceIds,
    selectionModel,
    hasClipboardFrames,
    hasSingleFrameSelection,
    hasMixedSelectedDurations,
    canDeleteUnselectedFrames,
    frameContextMenu,
    frameDropTarget,
    frameReorderState,
    frameDurationDialog,
    frameDurationMode,
    frameDurationSecondsValue,
    frameDurationFpsValue,
    nthSelectionStep,
    showNthSelectionDialog,
    frameTableBodyRef,
    frameContextMenuRef,
    frameDurationDialogRef,
    nthSelectionDialogRef,
    resetFrameEditorState,
    setFrameDurationMode,
    setNthSelectionStep,
    handleFramePointerDown,
    handleFrameKeyDown,
    handleFrameContextMenu,
    selectSingleFrame,
    selectAdjacentFrame,
    selectAllFrames,
    clearAllFrames,
    openFrameDurationDialog,
    splitCurrentFrame,
    speedAdjustSelectedFrames,
    copySelectedFramesTo,
    moveSelectedFramesTo,
    moveSelectedFrames,
    reverseSelectedFrames,
    deleteUnselectedFrames,
    selectOddFrames,
    selectEvenFrames,
    openNthSelectionDialog,
    invertFrameSelection,
    renumberTimelineFrames,
    copyFramesToClipboard,
    cutFramesToClipboard,
    pasteClipboard,
    pasteClipboardAtSelection,
    applyDurationChange,
    updateFrameDurationFromSeconds,
    updateFrameDurationFromFps,
    applyNthFrameSelection,
    closeFrameDurationDialog,
    closeNthSelectionDialog,
  };
}
