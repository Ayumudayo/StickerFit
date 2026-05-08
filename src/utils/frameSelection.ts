import type { TimelineFrameView } from "../types/editor";

type BuildFramePointerSelectionParams = {
  orderedFrameIds: string[];
  selectedInstanceIds: string[];
  anchorInstanceId: string | null;
  targetInstanceId: string;
  ctrlKey: boolean;
  shiftKey: boolean;
};

export function frameIdsInRange(
  orderedFrameIds: string[],
  startInstanceId: string,
  endInstanceId: string,
) {
  const startIndex = orderedFrameIds.indexOf(startInstanceId);
  const endIndex = orderedFrameIds.indexOf(endInstanceId);
  if (startIndex === -1 || endIndex === -1) {
    return [] as string[];
  }

  const minIndex = Math.min(startIndex, endIndex);
  const maxIndex = Math.max(startIndex, endIndex);
  const range = orderedFrameIds.slice(minIndex, maxIndex + 1);
  return startIndex <= endIndex ? range : range.reverse();
}

function validSelectionHistory(
  orderedFrameIds: string[],
  selectedInstanceIds: string[],
) {
  const validIds = new Set(orderedFrameIds);
  const seenIds = new Set<string>();
  const nextIds: string[] = [];

  for (const instanceId of selectedInstanceIds) {
    if (!validIds.has(instanceId) || seenIds.has(instanceId)) {
      continue;
    }

    seenIds.add(instanceId);
    nextIds.push(instanceId);
  }

  return nextIds;
}

function addRangeToSelectionHistory(
  orderedFrameIds: string[],
  selectedInstanceIds: string[],
  rangeIds: string[],
  targetInstanceId: string,
) {
  const nextIds = validSelectionHistory(orderedFrameIds, selectedInstanceIds).filter(
    (instanceId) => instanceId !== targetInstanceId,
  );
  const selectedIds = new Set(nextIds);

  for (const instanceId of rangeIds) {
    if (instanceId === targetInstanceId || selectedIds.has(instanceId)) {
      continue;
    }

    selectedIds.add(instanceId);
    nextIds.push(instanceId);
  }

  if (orderedFrameIds.includes(targetInstanceId)) {
    nextIds.push(targetInstanceId);
  }

  return nextIds;
}

export function buildFramePointerSelection({
  orderedFrameIds,
  selectedInstanceIds,
  anchorInstanceId,
  targetInstanceId,
  ctrlKey,
  shiftKey,
}: BuildFramePointerSelectionParams) {
  if (shiftKey) {
    const rangeAnchor =
      anchorInstanceId && orderedFrameIds.includes(anchorInstanceId)
        ? anchorInstanceId
        : selectedInstanceIds[selectedInstanceIds.length - 1] ?? targetInstanceId;
    const rangeIds = frameIdsInRange(orderedFrameIds, rangeAnchor, targetInstanceId);

    if (ctrlKey) {
      return {
        selectedInstanceIds: addRangeToSelectionHistory(
          orderedFrameIds,
          selectedInstanceIds,
          rangeIds,
          targetInstanceId,
        ),
        anchorInstanceId: rangeAnchor,
        action: "range" as const,
      };
    }

    return {
      selectedInstanceIds: rangeIds,
      anchorInstanceId: rangeAnchor,
      action: "range" as const,
    };
  }

  if (ctrlKey) {
    const selectedHistory = validSelectionHistory(orderedFrameIds, selectedInstanceIds);
    const isSelected = selectedHistory.includes(targetInstanceId);
    const nextSelectedIds = isSelected
      ? selectedHistory.filter((instanceId) => instanceId !== targetInstanceId)
      : [...selectedHistory, targetInstanceId];

    if (!isSelected && !orderedFrameIds.includes(targetInstanceId)) {
      return {
        selectedInstanceIds: selectedHistory,
        anchorInstanceId: targetInstanceId,
        action: "toggle" as const,
      };
    } else {
      return {
        selectedInstanceIds: nextSelectedIds,
        anchorInstanceId: targetInstanceId,
        action: "toggle" as const,
      };
    }
  }

  return {
    selectedInstanceIds: [targetInstanceId],
    anchorInstanceId: targetInstanceId,
    action: "replace" as const,
  };
}

export function lastSelectedFrameView(
  timelineFrameViews: TimelineFrameView[],
  selectedInstanceIds: string[],
) {
  const frameByInstanceId = new Map(
    timelineFrameViews.map((frame) => [frame.instanceId, frame]),
  );

  for (let index = selectedInstanceIds.length - 1; index >= 0; index -= 1) {
    const frame = frameByInstanceId.get(selectedInstanceIds[index]);
    if (frame) {
      return frame;
    }
  }

  return null;
}

export function selectedPlaybackFrames(
  timelineFrameViews: TimelineFrameView[],
  _selectedInstanceIds: string[],
) {
  return timelineFrameViews;
}
