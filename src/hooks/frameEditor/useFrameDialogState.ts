import {
  type Dispatch,
  type SetStateAction,
  useCallback,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  selectNthTimelineFrames,
  setSelectedFrameDurationUs,
} from "../../utils/frameEditing";
import { microsecondsToSeconds, secondsToMicroseconds } from "../../utils/timelineFrames";
import type {
  FrameContextMenuState,
  FrameDurationDialogMode,
  FrameDurationDialogState,
  TimelineFrame,
} from "../../types/editor";

type UseFrameDialogStateParams = {
  timelineFrames: TimelineFrame[];
  selectedInstanceIds: string[];
  frameContextMenu: FrameContextMenuState | null;
  minDurationUs: number;
  setTimelineFrames: Dispatch<SetStateAction<TimelineFrame[]>>;
  setSelectedInstanceIds: Dispatch<SetStateAction<string[]>>;
  closeFrameContextMenu: () => void;
};

export function useFrameDialogState({
  timelineFrames,
  selectedInstanceIds,
  frameContextMenu,
  minDurationUs,
  setTimelineFrames,
  setSelectedInstanceIds,
  closeFrameContextMenu,
}: UseFrameDialogStateParams) {
  const [frameDurationDialog, setFrameDurationDialog] =
    useState<FrameDurationDialogState | null>(null);
  const [frameDurationMode, setFrameDurationMode] =
    useState<FrameDurationDialogMode>("fps");
  const [nthSelectionStep, setNthSelectionStep] = useState(3);
  const [showNthSelectionDialog, setShowNthSelectionDialog] = useState(false);

  const frameDurationDialogRef = useRef<HTMLElement | null>(null);
  const nthSelectionDialogRef = useRef<HTMLElement | null>(null);
  const selectedInstanceIdSet = useMemo(
    () => new Set(selectedInstanceIds),
    [selectedInstanceIds],
  );

  const selectedDurationSet = useMemo(
    () =>
      new Set(
        timelineFrames
          .filter((frame) => selectedInstanceIdSet.has(frame.instanceId))
          .map((frame) => frame.durationUs),
      ),
    [selectedInstanceIdSet, timelineFrames],
  );
  const hasMixedSelectedDurations = selectedDurationSet.size > 1;
  const frameDurationSecondsValue = frameDurationDialog
    ? microsecondsToSeconds(frameDurationDialog.durationUs)
    : 0;
  const frameDurationFpsValue = frameDurationDialog
    ? Math.max(1, Math.round(1 / frameDurationSecondsValue))
    : 1;

  const resetDialogState = useCallback(() => {
    setFrameDurationDialog(null);
    setFrameDurationMode("fps");
    setNthSelectionStep(3);
    setShowNthSelectionDialog(false);
  }, []);

  const openFrameDurationDialog = useCallback(() => {
    const anchorId = frameContextMenu?.anchorInstanceId ?? selectedInstanceIds[0];
    const anchorFrame = timelineFrames.find((frame) => frame.instanceId === anchorId);
    if (!anchorFrame) {
      return;
    }

    setFrameDurationDialog({
      anchorInstanceId: anchorFrame.instanceId,
      durationUs: anchorFrame.durationUs,
    });
    setFrameDurationMode("fps");
    closeFrameContextMenu();
  }, [closeFrameContextMenu, frameContextMenu, selectedInstanceIds, timelineFrames]);

  const applyDurationChange = useCallback(
    (durationUs: number) => {
      setTimelineFrames((current) =>
        setSelectedFrameDurationUs(
          current,
          selectedInstanceIds,
          Math.max(minDurationUs, durationUs),
        ),
      );
      setFrameDurationDialog(null);
    },
    [minDurationUs, selectedInstanceIds, setTimelineFrames],
  );

  const updateFrameDurationFromSeconds = useCallback(
    (seconds: number) => {
      setFrameDurationDialog((current) =>
        current
          ? {
              ...current,
              durationUs: Math.max(minDurationUs, secondsToMicroseconds(seconds)),
            }
          : current,
      );
    },
    [minDurationUs],
  );

  const updateFrameDurationFromFps = useCallback(
    (fps: number) => {
      const normalizedFps = Math.max(1, Math.round(fps));
      updateFrameDurationFromSeconds(1 / normalizedFps);
    },
    [updateFrameDurationFromSeconds],
  );

  const openNthSelectionDialog = useCallback(() => {
    setShowNthSelectionDialog(true);
    closeFrameContextMenu();
  }, [closeFrameContextMenu]);

  const applyNthFrameSelection = useCallback(() => {
    setSelectedInstanceIds(selectNthTimelineFrames(timelineFrames, nthSelectionStep, 0));
    setShowNthSelectionDialog(false);
    closeFrameContextMenu();
  }, [closeFrameContextMenu, nthSelectionStep, setSelectedInstanceIds, timelineFrames]);

  const closeFrameDurationDialog = useCallback(() => {
    setFrameDurationDialog(null);
  }, []);

  const closeNthSelectionDialog = useCallback(() => {
    setShowNthSelectionDialog(false);
  }, []);

  return {
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
  };
}
