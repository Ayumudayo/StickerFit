export type EditableTimelineFrame = {
  instanceId: string;
  sourceFrameId: number;
  displayNumber: number;
  durationUs: number;
};

export function selectAllTimelineFrameIds(
  timelineFrames: EditableTimelineFrame[],
) {
  return timelineFrames.map((frame) => frame.instanceId);
}

export function clearTimelineSelection() {
  return [] as string[];
}

function selectedIdSet(selectedInstanceIds: string[]) {
  return new Set(selectedInstanceIds);
}

export function invertTimelineSelection(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  return timelineFrames
    .map((frame) => frame.instanceId)
    .filter((instanceId) => !selectedIds.has(instanceId));
}

export function selectNthTimelineFrames(
  timelineFrames: EditableTimelineFrame[],
  step: number,
  offset: number,
) {
  if (step <= 0) {
    return [] as string[];
  }

  return timelineFrames
    .filter((_, index) => (index + 1 - offset) % step === 0)
    .map((frame) => frame.instanceId);
}

export function deleteSelectedTimelineFrames(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  return timelineFrames.filter((frame) => !selectedIds.has(frame.instanceId));
}

export function deleteUnselectedTimelineFrames(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  return timelineFrames.filter((frame) => selectedIds.has(frame.instanceId));
}

export function setSelectedFrameDurationUs(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
  durationUs: number,
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  return timelineFrames.map((frame) =>
    selectedIds.has(frame.instanceId)
      ? { ...frame, durationUs }
      : frame,
  );
}

export function scaleSelectedFrameDurations(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
  factor: number,
  minDurationUs: number,
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  return timelineFrames.map((frame) => {
    if (!selectedIds.has(frame.instanceId)) {
      return frame;
    }

    return {
      ...frame,
      durationUs: Math.max(minDurationUs, Math.round(frame.durationUs * factor)),
    };
  });
}

export function splitSelectedFrame(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
  createInstanceId: () => string,
  minDurationUs: number,
) {
  if (selectedInstanceIds.length !== 1) {
    return null;
  }

  const selectedId = selectedInstanceIds[0];
  const splitIndex = timelineFrames.findIndex(
    (frame) => frame.instanceId === selectedId,
  );
  if (splitIndex === -1) {
    return null;
  }

  const target = timelineFrames[splitIndex];
  if (target.durationUs < minDurationUs * 2) {
    return null;
  }

  const firstDurationUs = Math.floor(target.durationUs / 2);
  const secondDurationUs = target.durationUs - firstDurationUs;
  const firstFrame: EditableTimelineFrame = {
    ...target,
    instanceId: createInstanceId(),
    durationUs: firstDurationUs,
  };
  const secondFrame: EditableTimelineFrame = {
    ...target,
    instanceId: createInstanceId(),
    durationUs: secondDurationUs,
  };

  const nextFrames = [...timelineFrames];
  nextFrames.splice(splitIndex, 1, firstFrame, secondFrame);

  return {
    timelineFrames: nextFrames,
    selectedInstanceIds: [firstFrame.instanceId, secondFrame.instanceId],
    anchorInstanceId: firstFrame.instanceId,
  };
}

function orderedSelectedFrames(
  timelineFrames: EditableTimelineFrame[],
  selectedIds: ReadonlySet<string>,
) {
  return timelineFrames.filter((frame) => selectedIds.has(frame.instanceId));
}

function selectedFrameIndexes(
  timelineFrames: EditableTimelineFrame[],
  selectedIds: ReadonlySet<string>,
) {
  return timelineFrames
    .map((frame, index) =>
      selectedIds.has(frame.instanceId) ? index : -1,
    )
    .filter((index) => index !== -1);
}

export function moveSelectedFramesByStep(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
  direction: -1 | 1,
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  const nextFrames = [...timelineFrames];
  const isSelected = (frame: EditableTimelineFrame) => selectedIds.has(frame.instanceId);

  if (direction === -1) {
    for (let index = 1; index < nextFrames.length; index += 1) {
      if (isSelected(nextFrames[index]) && !isSelected(nextFrames[index - 1])) {
        const current = nextFrames[index];
        nextFrames[index] = nextFrames[index - 1];
        nextFrames[index - 1] = current;
      }
    }
  } else {
    for (let index = nextFrames.length - 2; index >= 0; index -= 1) {
      if (isSelected(nextFrames[index]) && !isSelected(nextFrames[index + 1])) {
        const current = nextFrames[index];
        nextFrames[index] = nextFrames[index + 1];
        nextFrames[index + 1] = current;
      }
    }
  }

  return nextFrames;
}

export function moveSelectedFramesToBoundary(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
  boundary: "start" | "end",
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  const selected = orderedSelectedFrames(timelineFrames, selectedIds);
  const unselected = timelineFrames.filter((frame) => !selectedIds.has(frame.instanceId));

  return boundary === "start"
    ? [...selected, ...unselected]
    : [...unselected, ...selected];
}

export function copySelectedFramesToBoundary(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
  boundary: "start" | "end",
  createInstanceId: () => string,
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  const selectedClones = orderedSelectedFrames(timelineFrames, selectedIds).map((frame) => ({
    ...frame,
    instanceId: createInstanceId(),
  }));

  return boundary === "start"
    ? [...selectedClones, ...timelineFrames]
    : [...timelineFrames, ...selectedClones];
}

export function reverseSelectedFramesInPlace(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
) {
  const selectedIds = selectedIdSet(selectedInstanceIds);
  const indexes = selectedFrameIndexes(timelineFrames, selectedIds);
  const reversedFrames = orderedSelectedFrames(timelineFrames, selectedIds).reverse();
  const nextFrames = [...timelineFrames];

  indexes.forEach((index, order) => {
    nextFrames[index] = reversedFrames[order];
  });

  return nextFrames;
}

export function copySelectedFrames(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
) {
  return orderedSelectedFrames(timelineFrames, selectedIdSet(selectedInstanceIds));
}

export function cutSelectedFrames(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
) {
  return {
    clipboardFrames: copySelectedFrames(timelineFrames, selectedInstanceIds),
    timelineFrames: deleteSelectedTimelineFrames(timelineFrames, selectedInstanceIds),
  };
}

export function pasteClipboardFrames(
  timelineFrames: EditableTimelineFrame[],
  clipboardFrames: EditableTimelineFrame[],
  anchorInstanceId: string,
  position: "above" | "below",
  createInstanceId: () => string,
) {
  const anchorIndex = timelineFrames.findIndex(
    (frame) => frame.instanceId === anchorInstanceId,
  );
  if (anchorIndex === -1 || clipboardFrames.length === 0) {
    return null;
  }

  const clones = clipboardFrames.map((frame) => ({
    ...frame,
    instanceId: createInstanceId(),
  }));
  const insertionIndex = position === "above" ? anchorIndex : anchorIndex + 1;
  const nextFrames = [...timelineFrames];
  nextFrames.splice(insertionIndex, 0, ...clones);

  return {
    timelineFrames: nextFrames,
    selectedInstanceIds: clones.map((frame) => frame.instanceId),
    anchorInstanceId: clones[0]?.instanceId ?? null,
  };
}

export function moveSelectedFramesAroundAnchor(
  timelineFrames: EditableTimelineFrame[],
  selectedInstanceIds: string[],
  anchorInstanceId: string,
  position: "above" | "below",
) {
  if (selectedInstanceIds.length === 0) {
    return timelineFrames;
  }

  const selectedIds = selectedIdSet(selectedInstanceIds);

  if (selectedIds.has(anchorInstanceId)) {
    return timelineFrames;
  }

  const selectedFrames = orderedSelectedFrames(timelineFrames, selectedIds);
  const remainingFrames = timelineFrames.filter((frame) => !selectedIds.has(frame.instanceId));
  const anchorIndex = remainingFrames.findIndex(
    (frame) => frame.instanceId === anchorInstanceId,
  );

  if (anchorIndex === -1) {
    return timelineFrames;
  }

  const insertionIndex = position === "above" ? anchorIndex : anchorIndex + 1;
  const nextFrames = [...remainingFrames];
  nextFrames.splice(insertionIndex, 0, ...selectedFrames);
  return nextFrames;
}
