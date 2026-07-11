import type { FrameDropTargetState } from "../types/editor";

type RectBounds = {
  left: number;
  right: number;
  top: number;
  bottom: number;
};

type FrameDropGeometryRow = {
  instanceId: string;
  top: number;
  bottom: number;
};

type ResolveFrameDropTargetFromGeometryParams = {
  listBounds: RectBounds;
  rows: FrameDropGeometryRow[];
  draggedInstanceIds: string[];
  clientX: number;
  clientY: number;
};

type ResolveFrameDropTargetFromVirtualGeometryParams = {
  listBounds: RectBounds;
  scrollTop: number;
  rowHeight: number;
  orderedInstanceIds: readonly string[];
  draggedInstanceIds: readonly string[];
  clientX: number;
  clientY: number;
};

type FrameRailAutoScrollDeltaParams = {
  clientY: number;
  listBounds: RectBounds;
  edgeSize: number;
  maxStep: number;
};

type ClampFrameRailAutoScrollParams = {
  scrollTop: number;
  delta: number;
  scrollHeight: number;
  clientHeight: number;
};

export function resolveFrameDropTargetFromGeometry({
  listBounds,
  rows,
  draggedInstanceIds,
  clientX,
  clientY,
}: ResolveFrameDropTargetFromGeometryParams) {
  if (
    clientX < listBounds.left ||
    clientX > listBounds.right ||
    clientY < listBounds.top ||
    clientY > listBounds.bottom
  ) {
    return null;
  }

  const draggedInstanceIdSet = new Set(draggedInstanceIds);
  let lastAnchorInstanceId: string | null = null;

  for (const row of rows) {
    if (draggedInstanceIdSet.has(row.instanceId)) {
      continue;
    }

    lastAnchorInstanceId = row.instanceId;
    const rowMidpoint = row.top + (row.bottom - row.top) / 2;
    if (clientY <= row.bottom) {
      return {
        anchorInstanceId: row.instanceId,
        position: clientY < rowMidpoint ? "above" : "below",
      } satisfies FrameDropTargetState;
    }
  }

  if (!lastAnchorInstanceId) {
    return null;
  }

  return {
    anchorInstanceId: lastAnchorInstanceId,
    position: "below",
  } satisfies FrameDropTargetState;
}

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, value));
}

export function resolveFrameDropTargetFromVirtualGeometry({
  listBounds,
  scrollTop,
  rowHeight,
  orderedInstanceIds,
  draggedInstanceIds,
  clientX,
  clientY,
}: ResolveFrameDropTargetFromVirtualGeometryParams): FrameDropTargetState | null {
  if (
    orderedInstanceIds.length === 0 ||
    !Number.isFinite(listBounds.left) ||
    !Number.isFinite(listBounds.right) ||
    !Number.isFinite(listBounds.top) ||
    !Number.isFinite(listBounds.bottom) ||
    listBounds.right < listBounds.left ||
    listBounds.bottom < listBounds.top ||
    !Number.isFinite(scrollTop) ||
    !Number.isFinite(rowHeight) ||
    rowHeight <= 0 ||
    !Number.isFinite(clientX) ||
    !Number.isFinite(clientY) ||
    clientX < listBounds.left ||
    clientX > listBounds.right ||
    clientY < listBounds.top ||
    clientY > listBounds.bottom
  ) {
    return null;
  }

  const draggedInstanceIdSet = new Set(draggedInstanceIds);
  if (orderedInstanceIds.every((instanceId) => draggedInstanceIdSet.has(instanceId))) {
    return null;
  }

  const contentY = Math.max(0, scrollTop) + clientY - listBounds.top;
  const rawIndex = clamp(
    Math.floor(contentY / rowHeight),
    0,
    orderedInstanceIds.length - 1,
  );
  const rowMidpoint = rawIndex * rowHeight + rowHeight / 2;
  const position: FrameDropTargetState["position"] =
    contentY < rowMidpoint ? "above" : "below";
  const rawAnchorInstanceId = orderedInstanceIds[rawIndex];

  if (!draggedInstanceIdSet.has(rawAnchorInstanceId)) {
    return {
      anchorInstanceId: rawAnchorInstanceId,
      position,
    };
  }

  if (position === "above") {
    for (let index = rawIndex - 1; index >= 0; index -= 1) {
      const instanceId = orderedInstanceIds[index];
      if (!draggedInstanceIdSet.has(instanceId)) {
        return { anchorInstanceId: instanceId, position: "below" };
      }
    }
    for (let index = rawIndex + 1; index < orderedInstanceIds.length; index += 1) {
      const instanceId = orderedInstanceIds[index];
      if (!draggedInstanceIdSet.has(instanceId)) {
        return { anchorInstanceId: instanceId, position: "above" };
      }
    }
  } else {
    for (let index = rawIndex + 1; index < orderedInstanceIds.length; index += 1) {
      const instanceId = orderedInstanceIds[index];
      if (!draggedInstanceIdSet.has(instanceId)) {
        return { anchorInstanceId: instanceId, position: "above" };
      }
    }
    for (let index = rawIndex - 1; index >= 0; index -= 1) {
      const instanceId = orderedInstanceIds[index];
      if (!draggedInstanceIdSet.has(instanceId)) {
        return { anchorInstanceId: instanceId, position: "below" };
      }
    }
  }

  return null;
}

export function computeFrameRailAutoScrollDelta({
  clientY,
  listBounds,
  edgeSize,
  maxStep,
}: FrameRailAutoScrollDeltaParams) {
  const height = listBounds.bottom - listBounds.top;
  if (
    !Number.isFinite(clientY) ||
    !Number.isFinite(listBounds.top) ||
    !Number.isFinite(listBounds.bottom) ||
    !Number.isFinite(height) ||
    !Number.isFinite(edgeSize) ||
    !Number.isFinite(maxStep) ||
    height <= 0 ||
    edgeSize <= 0 ||
    maxStep <= 0 ||
    clientY < listBounds.top ||
    clientY > listBounds.bottom
  ) {
    return 0;
  }

  const effectiveEdgeSize = Math.min(edgeSize, height / 2);
  const topEdgeEnd = listBounds.top + effectiveEdgeSize;
  if (clientY < topEdgeEnd) {
    const intensity = (topEdgeEnd - clientY) / effectiveEdgeSize;
    return -Math.min(maxStep, maxStep * intensity);
  }

  const bottomEdgeStart = listBounds.bottom - effectiveEdgeSize;
  if (clientY > bottomEdgeStart) {
    const intensity = (clientY - bottomEdgeStart) / effectiveEdgeSize;
    return Math.min(maxStep, maxStep * intensity);
  }

  return 0;
}

export function clampFrameRailAutoScroll({
  scrollTop,
  delta,
  scrollHeight,
  clientHeight,
}: ClampFrameRailAutoScrollParams) {
  if (
    !Number.isFinite(scrollTop) ||
    !Number.isFinite(delta) ||
    !Number.isFinite(scrollHeight) ||
    !Number.isFinite(clientHeight) ||
    scrollHeight < 0 ||
    clientHeight < 0
  ) {
    return 0;
  }

  const maximumScrollTop = Math.max(0, scrollHeight - clientHeight);
  return clamp(scrollTop + delta, 0, maximumScrollTop);
}

export function advanceFrameRailAutoScroll(params: ClampFrameRailAutoScrollParams) {
  const currentScrollTop = clampFrameRailAutoScroll({ ...params, delta: 0 });
  const nextScrollTop = clampFrameRailAutoScroll(params);
  return {
    scrollTop: nextScrollTop,
    didScroll: nextScrollTop !== currentScrollTop,
  };
}

export function resolveFrameDropTargetFromList(
  listElement: HTMLDivElement | null,
  draggedInstanceIds: string[],
  clientX: number,
  clientY: number,
) {
  if (!listElement) {
    return null;
  }

  const bounds = listElement.getBoundingClientRect();
  const rows = Array.from(listElement.children)
    .filter((row): row is HTMLButtonElement => row instanceof HTMLButtonElement)
    .map((row) => {
      const rowBounds = row.getBoundingClientRect();
      return {
        instanceId: row.dataset.instanceId ?? "",
        top: rowBounds.top,
        bottom: rowBounds.bottom,
      };
    })
    .filter((row) => row.instanceId.length > 0);

  return resolveFrameDropTargetFromGeometry({
    listBounds: {
      left: bounds.left,
      right: bounds.right,
      top: bounds.top,
      bottom: bounds.bottom,
    },
    rows,
    draggedInstanceIds,
    clientX,
    clientY,
  });
}

export function releasePointerCaptureIfHeld(
  element: HTMLButtonElement,
  pointerId: number,
) {
  if (element.hasPointerCapture(pointerId)) {
    element.releasePointerCapture(pointerId);
  }
}
