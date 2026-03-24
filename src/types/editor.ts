export type SourceFrame = {
  sourceFrameId: number;
  startTimeUs: number;
  durationUs: number;
};

export type TimelineFrame = {
  instanceId: string;
  sourceFrameId: number;
  displayNumber: number;
  durationUs: number;
};

export type TimelineFrameView = {
  instanceId: string;
  sourceFrameId: number;
  displayNumber: number;
  durationUs: number;
  durationSeconds: number;
  startTimeSeconds: number;
  sourceStartTimeSeconds: number;
};

export type FrameSelectionModel = {
  readonly selectedInstanceIdSet: ReadonlySet<string>;
  readonly selectedVisibleCount: number;
  readonly hasSelectedFrames: boolean;
};

export type TimelineDragState = {
  pointerId: number;
};

export type DragSelectionState = {
  startInstanceId: string;
  willSelect: boolean;
  preDragIds: string[];
  applyOnPointerUp: boolean;
};

export type FrameContextMenuState = {
  x: number;
  y: number;
  anchorInstanceId: string;
};

export type FrameDropTargetState = {
  anchorInstanceId: string;
  position: "above" | "below";
};

export type FrameReorderState = {
  pointerId: number;
  draggedInstanceIds: string[];
  startY: number;
  currentY: number;
  active: boolean;
};

export type FrameDurationDialogMode = "fps" | "seconds";

export type FrameDurationDialogState = {
  anchorInstanceId: string;
  durationUs: number;
};
