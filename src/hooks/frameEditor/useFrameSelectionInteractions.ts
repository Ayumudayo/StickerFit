import {
  type Dispatch,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type SetStateAction,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { releasePointerCaptureIfHeld, resolveFrameDropTargetFromList } from "../../utils/frameDnD";
import { buildFramePointerSelection } from "../../utils/frameSelection";
import { moveSelectedFramesAroundAnchor } from "../../utils/frameEditing";
import { useFrameContextMenuState } from "./useFrameContextMenuState";
import { useFrameSelectionCommands } from "./useFrameSelectionCommands";
import type {
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

type SingleFrameSelectionOptions = {
  focus?: boolean;
};

function findFrameRowElement(container: HTMLDivElement | null, instanceId: string) {
  if (!container) {
    return null;
  }

  return (
    Array.from(container.querySelectorAll<HTMLButtonElement>("[data-instance-id]"))
      .find((element) => element.dataset.instanceId === instanceId) ?? null
  );
}

export function useFrameSelectionInteractions({
  timelineFrames,
  timelineFrameViews,
  setTimelineFrames,
}: UseFrameSelectionInteractionsParams) {
  const [selectedInstanceIds, setSelectedInstanceIds] = useState<string[]>([]);
  const [frameDropTarget, setFrameDropTarget] = useState<FrameDropTargetState | null>(null);
  const [frameReorderState, setFrameReorderState] = useState<FrameReorderState | null>(null);

  const frameTableBodyRef = useRef<HTMLDivElement | null>(null);
  const selectionAnchorInstanceIdRef = useRef<string | null>(null);
  const frameReorderStateRef = useRef<FrameReorderState | null>(null);
  const frameDropTargetRef = useRef<FrameDropTargetState | null>(null);
  const selectedInstanceIdSet = useMemo(
    () => new Set(selectedInstanceIds),
    [selectedInstanceIds],
  );
  const orderedFrameIds = useMemo(
    () => timelineFrameViews.map((frame) => frame.instanceId),
    [timelineFrameViews],
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
    selectionAnchorInstanceIdRef.current = null;
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

  const scrollFrameIntoView = useCallback((instanceId: string) => {
    findFrameRowElement(frameTableBodyRef.current, instanceId)?.scrollIntoView({
      block: "nearest",
      inline: "nearest",
    });
  }, []);

  const focusFrame = useCallback((instanceId: string) => {
    const targetElement = findFrameRowElement(frameTableBodyRef.current, instanceId);
    targetElement?.focus({ preventScroll: true });
    scrollFrameIntoView(instanceId);
  }, [scrollFrameIntoView]);

  const selectSingleFrame = useCallback(
    (instanceId: string, options: SingleFrameSelectionOptions = {}) => {
      if (!orderedFrameIds.includes(instanceId)) {
        return false;
      }

      setSelectedInstanceIds((current) =>
        current.length === 1 && current[0] === instanceId ? current : [instanceId],
      );
      selectionAnchorInstanceIdRef.current = instanceId;
      updateFrameReorderState(null);
      updateFrameDropTarget(null);

      if (options.focus) {
        focusFrame(instanceId);
      } else {
        scrollFrameIntoView(instanceId);
      }

      return true;
    },
    [
      focusFrame,
      orderedFrameIds,
      scrollFrameIntoView,
      updateFrameDropTarget,
      updateFrameReorderState,
    ],
  );

  const selectAdjacentFrame = useCallback(
    (direction: -1 | 1, anchorInstanceId?: string | null) => {
      if (orderedFrameIds.length === 0) {
        return false;
      }

      const selectedAnchorInstanceId = selectedInstanceIds[selectedInstanceIds.length - 1];
      const resolvedAnchorInstanceId =
        anchorInstanceId && orderedFrameIds.includes(anchorInstanceId)
          ? anchorInstanceId
          : selectedAnchorInstanceId && orderedFrameIds.includes(selectedAnchorInstanceId)
            ? selectedAnchorInstanceId
            : null;
      const currentIndex = resolvedAnchorInstanceId
        ? orderedFrameIds.indexOf(resolvedAnchorInstanceId)
        : direction > 0
          ? -1
          : 0;
      const nextIndex = (currentIndex + direction + orderedFrameIds.length) % orderedFrameIds.length;
      const nextInstanceId = orderedFrameIds[nextIndex];
      if (!nextInstanceId) {
        return false;
      }

      return selectSingleFrame(nextInstanceId, { focus: true });
    },
    [
      orderedFrameIds,
      selectSingleFrame,
      selectedInstanceIds,
    ],
  );

  useLayoutEffect(() => {
    const focusedSelectionId = selectedInstanceIds[selectedInstanceIds.length - 1];
    if (!focusedSelectionId) {
      return;
    }

    scrollFrameIntoView(focusedSelectionId);
  }, [scrollFrameIntoView, selectedInstanceIds, timelineFrameViews]);

  const handleFramePointerDown = useCallback(
    (instanceId: string, event: ReactPointerEvent<HTMLButtonElement>) => {
      if (event.pointerType === "mouse" && event.button !== 0) return;

      const usesSelectionModifier = event.ctrlKey || event.metaKey || event.shiftKey;

      if (usesSelectionModifier) {
        const nextSelection = buildFramePointerSelection({
          orderedFrameIds,
          selectedInstanceIds,
          anchorInstanceId: selectionAnchorInstanceIdRef.current,
          targetInstanceId: instanceId,
          ctrlKey: event.ctrlKey || event.metaKey,
          shiftKey: event.shiftKey,
        });
        setSelectedInstanceIds(nextSelection.selectedInstanceIds);
        selectionAnchorInstanceIdRef.current = nextSelection.anchorInstanceId;
        updateFrameReorderState(null);
        updateFrameDropTarget(null);
        releasePointerCaptureIfHeld(event.currentTarget, event.pointerId);
        return;
      }

      if (selectedInstanceIdSet.has(instanceId)) {
        updateFrameReorderState({
          pointerId: event.pointerId,
          draggedInstanceIds: selectedInstanceIds,
          collapseToInstanceIdOnClick: instanceId,
          startY: event.clientY,
          currentY: event.clientY,
          active: false,
        });
        selectionAnchorInstanceIdRef.current = instanceId;
        releasePointerCaptureIfHeld(event.currentTarget, event.pointerId);
        return;
      }

      setSelectedInstanceIds([instanceId]);
      selectionAnchorInstanceIdRef.current = instanceId;
      updateFrameReorderState({
        pointerId: event.pointerId,
        draggedInstanceIds: [instanceId],
        collapseToInstanceIdOnClick: instanceId,
        startY: event.clientY,
        currentY: event.clientY,
        active: false,
      });

      releasePointerCaptureIfHeld(event.currentTarget, event.pointerId);
      return;
    },
    [
      selectedInstanceIdSet,
      selectedInstanceIds,
      orderedFrameIds,
      timelineFrameViews.length,
      updateFrameDropTarget,
      updateFrameReorderState,
    ],
  );

  const handleFrameKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLButtonElement>) => {
      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") {
        return;
      }

      const currentInstanceId = event.currentTarget.dataset.instanceId;
      const currentIndex = currentInstanceId
        ? orderedFrameIds.indexOf(currentInstanceId)
        : -1;
      if (currentIndex === -1 || orderedFrameIds.length === 0) {
        return;
      }

      event.preventDefault();
      selectAdjacentFrame(
        event.key === "ArrowDown" ? 1 : -1,
        currentInstanceId,
      );
    },
    [
      orderedFrameIds,
      selectAdjacentFrame,
    ],
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
        return;
      }

      if (reorderState.active) {
        if (dropTarget) {
          setTimelineFrames((current) =>
            moveSelectedFramesAroundAnchor(
              current,
              reorderState.draggedInstanceIds,
              dropTarget.anchorInstanceId,
              dropTarget.position,
            ),
          );
        }
      } else if (reorderState.collapseToInstanceIdOnClick) {
        setSelectedInstanceIds([reorderState.collapseToInstanceIdOnClick]);
        selectionAnchorInstanceIdRef.current = reorderState.collapseToInstanceIdOnClick;
      }

      updateFrameReorderState(null);
      updateFrameDropTarget(null);
    };

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerEnd);
    window.addEventListener("pointercancel", handlePointerEnd);
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerEnd);
      window.removeEventListener("pointercancel", handlePointerEnd);
    };
  }, [setTimelineFrames, updateFrameDropTarget, updateFrameReorderState]);


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
  };
}
