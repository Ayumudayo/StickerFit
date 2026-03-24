import { useCallback } from "react";

import {
  clearTimelineSelection,
  invertTimelineSelection,
  moveSelectedFramesByStep,
  moveSelectedFramesToBoundary,
  selectAllTimelineFrameIds,
  selectNthTimelineFrames,
} from "../../utils/frameEditing";
import type { TimelineFrame } from "../../types/editor";

type UseFrameSelectionCommandsParams = {
  timelineFrames: TimelineFrame[];
  selectedInstanceIds: string[];
  setSelectedInstanceIds: (value: string[]) => void;
  setTimelineFrames: React.Dispatch<React.SetStateAction<TimelineFrame[]>>;
  closeFrameContextMenu: () => void;
};

export function useFrameSelectionCommands({
  timelineFrames,
  selectedInstanceIds,
  setSelectedInstanceIds,
  setTimelineFrames,
  closeFrameContextMenu,
}: UseFrameSelectionCommandsParams) {
  const selectAllFrames = useCallback(() => {
    setSelectedInstanceIds(selectAllTimelineFrameIds(timelineFrames));
    closeFrameContextMenu();
  }, [closeFrameContextMenu, setSelectedInstanceIds, timelineFrames]);

  const clearAllFrames = useCallback(() => {
    setSelectedInstanceIds(clearTimelineSelection());
    closeFrameContextMenu();
  }, [closeFrameContextMenu, setSelectedInstanceIds]);

  const selectOddFrames = useCallback(() => {
    setSelectedInstanceIds(selectNthTimelineFrames(timelineFrames, 2, 1));
    closeFrameContextMenu();
  }, [closeFrameContextMenu, setSelectedInstanceIds, timelineFrames]);

  const selectEvenFrames = useCallback(() => {
    setSelectedInstanceIds(selectNthTimelineFrames(timelineFrames, 2, 0));
    closeFrameContextMenu();
  }, [closeFrameContextMenu, setSelectedInstanceIds, timelineFrames]);

  const invertFrameSelection = useCallback(() => {
    setSelectedInstanceIds(invertTimelineSelection(timelineFrames, selectedInstanceIds));
    closeFrameContextMenu();
  }, [closeFrameContextMenu, selectedInstanceIds, setSelectedInstanceIds, timelineFrames]);

  const moveSelectedFrames = useCallback(
    (direction: -1 | 1) => {
      setTimelineFrames((current) =>
        moveSelectedFramesByStep(current, selectedInstanceIds, direction),
      );
      closeFrameContextMenu();
    },
    [closeFrameContextMenu, selectedInstanceIds, setTimelineFrames],
  );

  const moveSelectedFramesTo = useCallback(
    (boundary: "start" | "end") => {
      setTimelineFrames((current) =>
        moveSelectedFramesToBoundary(current, selectedInstanceIds, boundary),
      );
      closeFrameContextMenu();
    },
    [closeFrameContextMenu, selectedInstanceIds, setTimelineFrames],
  );

  return {
    selectAllFrames,
    clearAllFrames,
    selectOddFrames,
    selectEvenFrames,
    invertFrameSelection,
    moveSelectedFrames,
    moveSelectedFramesTo,
  };
}
