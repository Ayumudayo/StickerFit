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
