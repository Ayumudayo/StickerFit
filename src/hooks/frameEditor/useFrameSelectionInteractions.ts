import {
  type Dispatch,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type SetStateAction,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { releasePointerCaptureIfHeld, resolveFrameDropTargetFromList } from "../../utils/frameDnD";
import {
  moveSelectedFramesAroundAnchor,
} from "../../utils/frameEditing";
import { useFrameContextMenuState } from "./useFrameContextMenuState";
import { useFrameSelectionCommands } from "./useFrameSelectionCommands";
import type {
  DragSelectionState,
  FrameDropTargetState,
  FrameReorderState,
  FrameSelectionModel,
  TimelineFrame,
  TimelineFrameView,
} from "../../types/editor";

type UseFrameSelectionInteractionsParams = {
  timelineFrames: TimelineFrame[];
  timelineFrameViews: TimelineFrameView[];
  setTimelineFrames: Dispatch<SetStateAction<TimelineFrame[]>>;
};

export function useFrameSelectionInteractions({
  timelineFrames,
  timelineFrameViews,
  setTimelineFrames,
}: UseFrameSelectionInteractionsParams) {
  const [selectedInstanceIds, setSelectedInstanceIds] = useState<string[]>([]);
  const [frameDropTarget, setFrameDropTarget] = useState<FrameDropTargetState | null>(null);
  const [frameReorderState, setFrameReorderState] = useState<FrameReorderState | null>(null);

  const dragSelectionActionRef = useRef<DragSelectionState | null>(null);
  const frameTableBodyRef = useRef<HTMLDivElement | null>(null);
  const selectionAnchorInstanceIdRef = useRef<string | null>(null);
  const didDragSelectRef = useRef(false);
  const frameReorderStateRef = useRef<FrameReorderState | null>(null);
  const frameDropTargetRef = useRef<FrameDropTargetState | null>(null);
  const selectedInstanceIdSet = useMemo(
    () => new Set(selectedInstanceIds),
    [selectedInstanceIds],
  );

  const selectedTimelineFrames = useMemo(
    () =>
      timelineFrameViews.filter((frame) => selectedInstanceIdSet.has(frame.instanceId)),
    [selectedInstanceIdSet, timelineFrameViews],
  );
  const selectedVisibleCount = selectedTimelineFrames.length;
  const hasSelectedFrames = selectedVisibleCount > 0;
  const selectionModel = useMemo<FrameSelectionModel>(
    () => ({
      selectedInstanceIdSet,
      selectedVisibleCount,
      hasSelectedFrames,
    }),
    [hasSelectedFrames, selectedInstanceIdSet, selectedVisibleCount],
  );
  const hasSingleFrameSelection = selectedInstanceIds.length === 1;
  const canDeleteUnselectedFrames =
    selectedInstanceIds.length > 0 && selectedInstanceIds.length < timelineFrames.length;

  const {
    frameContextMenu,
    frameContextMenuRef,
    setFrameContextMenu,
    closeFrameContextMenu,
    handleFrameContextMenu,
  } = useFrameContextMenuState({
    selectedInstanceIdSet,
    setSelectedInstanceIds,
  });

  const resetSelectionState = useCallback(() => {
    setSelectedInstanceIds([]);
    setFrameContextMenu(null);
    frameDropTargetRef.current = null;
    frameReorderStateRef.current = null;
    setFrameDropTarget(null);
    setFrameReorderState(null);
    dragSelectionActionRef.current = null;
    selectionAnchorInstanceIdRef.current = null;
    didDragSelectRef.current = false;
  }, [setFrameContextMenu]);

  const updateFrameDropTarget = useCallback((nextTarget: FrameDropTargetState | null) => {
    const currentTarget = frameDropTargetRef.current;
    if (
      currentTarget?.anchorInstanceId === nextTarget?.anchorInstanceId &&
      currentTarget?.position === nextTarget?.position
    ) {
      return;
    }

    frameDropTargetRef.current = nextTarget;
    setFrameDropTarget(nextTarget);
  }, []);

  const updateFrameReorderState = useCallback((nextState: FrameReorderState | null) => {
    frameReorderStateRef.current = nextState;
    setFrameReorderState(nextState);
  }, []);

  const frameIdsInRange = useCallback(
    (startInstanceId: string, endInstanceId: string) => {
      const startIndex = timelineFrameViews.findIndex(
        (frame) => frame.instanceId === startInstanceId,
      );
      const endIndex = timelineFrameViews.findIndex(
        (frame) => frame.instanceId === endInstanceId,
      );
      if (startIndex === -1 || endIndex === -1) {
        return [] as string[];
      }

      const minIndex = Math.min(startIndex, endIndex);
      const maxIndex = Math.max(startIndex, endIndex);
      return timelineFrameViews
        .slice(minIndex, maxIndex + 1)
        .map((frame) => frame.instanceId);
    },
    [timelineFrameViews],
  );

  const applyFrameSelectionRange = useCallback(
    (
      startInstanceId: string,
      endInstanceId: string,
      preDragIds: string[],
      willSelect: boolean,
    ) => {
      const nextSelectedIds = new Set<string>(preDragIds);

      for (const instanceId of frameIdsInRange(startInstanceId, endInstanceId)) {
        if (willSelect) {
          nextSelectedIds.add(instanceId);
        } else {
          nextSelectedIds.delete(instanceId);
        }
      }

      return Array.from(nextSelectedIds);
    },
    [frameIdsInRange],
  );

  const commitPendingFrameSelection = useCallback(() => {
    const pendingSelection = dragSelectionActionRef.current;
    if (!pendingSelection) {
      didDragSelectRef.current = false;
      return;
    }

    if (pendingSelection.applyOnPointerUp && !didDragSelectRef.current) {
      setSelectedInstanceIds(
        applyFrameSelectionRange(
          pendingSelection.startInstanceId,
          pendingSelection.startInstanceId,
          pendingSelection.preDragIds,
          pendingSelection.willSelect,
        ),
      );
    }

    dragSelectionActionRef.current = null;
    didDragSelectRef.current = false;
  }, [applyFrameSelectionRange]);

  const handleFramePointerDown = useCallback(
    (instanceId: string, event: ReactPointerEvent<HTMLButtonElement>) => {
      if (event.pointerType === "mouse" && event.button !== 0) return;

      didDragSelectRef.current = false;

      if (selectedInstanceIdSet.has(instanceId)) {
        const draggedInstanceIds =
          selectedInstanceIds.length === timelineFrameViews.length
            ? [instanceId]
            : selectedInstanceIds;

        if (draggedInstanceIds.length === 1 && selectedInstanceIds.length !== 1) {
          setSelectedInstanceIds(draggedInstanceIds);
        }

        updateFrameReorderState({
          pointerId: event.pointerId,
          draggedInstanceIds,
          startY: event.clientY,
          currentY: event.clientY,
          active: false,
        });
        selectionAnchorInstanceIdRef.current = instanceId;
        releasePointerCaptureIfHeld(event.currentTarget, event.pointerId);
        return;
      }

      const startInstanceId = instanceId;
      const preDragIds: string[] = [];
      const willSelect = true;

      dragSelectionActionRef.current = {
        startInstanceId,
        willSelect,
        preDragIds,
        applyOnPointerUp: true,
      };
      selectionAnchorInstanceIdRef.current = instanceId;

      releasePointerCaptureIfHeld(event.currentTarget, event.pointerId);
      return;
    },
    [
      selectedInstanceIdSet,
      selectedInstanceIds,
      timelineFrameViews.length,
      updateFrameReorderState,
    ],
  );

  const handleFramePointerEnter = useCallback(
    (instanceId: string) => {
      if (!dragSelectionActionRef.current) return;

      const { startInstanceId, willSelect, preDragIds } = dragSelectionActionRef.current;
      if (startInstanceId !== instanceId) {
        didDragSelectRef.current = true;
      }

      setSelectedInstanceIds(
        applyFrameSelectionRange(startInstanceId, instanceId, preDragIds, willSelect),
      );
    },
    [applyFrameSelectionRange],
  );

  const handleFrameKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLButtonElement>) => {
      if (event.key === "ArrowUp") {
        event.preventDefault();
        const prevElement = event.currentTarget.previousElementSibling as HTMLButtonElement | null;
        prevElement?.focus();
      } else if (event.key === "ArrowDown") {
        event.preventDefault();
        const nextElement = event.currentTarget.nextElementSibling as HTMLButtonElement | null;
        nextElement?.focus();
      }
    },
    [],
  );

  useEffect(() => {
    const handlePointerMove = (event: PointerEvent) => {
      const reorderState = frameReorderStateRef.current;
      if (!reorderState || event.pointerId !== reorderState.pointerId) {
        return;
      }

      const nextActive =
        reorderState.active || Math.abs(event.clientY - reorderState.startY) >= 4;
      if (!nextActive) {
        return;
      }

      if (!reorderState.active || reorderState.currentY !== event.clientY) {
        updateFrameReorderState({
          ...reorderState,
          active: nextActive,
          currentY: event.clientY,
        });
      }

      const nextTarget = resolveFrameDropTargetFromList(
        frameTableBodyRef.current,
        reorderState.draggedInstanceIds,
        event.clientX,
        event.clientY,
      );
      updateFrameDropTarget(nextTarget);
    };

    const handlePointerEnd = (event: PointerEvent) => {
      const reorderState = frameReorderStateRef.current;
      const dropTarget = frameDropTargetRef.current;
      if (!reorderState || event.pointerId !== reorderState.pointerId) {
        commitPendingFrameSelection();
        return;
      }

      if (reorderState.active && dropTarget) {
        setTimelineFrames((current) =>
          moveSelectedFramesAroundAnchor(
            current,
            reorderState.draggedInstanceIds,
            dropTarget.anchorInstanceId,
            dropTarget.position,
          ),
        );
      }

      updateFrameReorderState(null);
      updateFrameDropTarget(null);
      commitPendingFrameSelection();
    };

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerEnd);
    window.addEventListener("pointercancel", handlePointerEnd);
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerEnd);
      window.removeEventListener("pointercancel", handlePointerEnd);
    };
  }, [commitPendingFrameSelection, setTimelineFrames, updateFrameDropTarget, updateFrameReorderState]);


  const {
    selectAllFrames,
    clearAllFrames,
    selectOddFrames,
    selectEvenFrames,
    invertFrameSelection,
    moveSelectedFrames,
    moveSelectedFramesTo,
  } = useFrameSelectionCommands({
    timelineFrames,
    selectedInstanceIds,
    setSelectedInstanceIds,
    setTimelineFrames,
    closeFrameContextMenu,
  });

  return {
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
    handleFramePointerEnter,
    handleFrameKeyDown,
    handleFrameContextMenu,
    selectAllFrames,
    clearAllFrames,
    selectOddFrames,
    selectEvenFrames,
    invertFrameSelection,
    moveSelectedFrames,
    moveSelectedFramesTo,
  };
}
