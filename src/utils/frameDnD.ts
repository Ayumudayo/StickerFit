import type { FrameDropTargetState } from "../types/editor";

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
  if (
    clientX < bounds.left ||
    clientX > bounds.right ||
    clientY < bounds.top ||
    clientY > bounds.bottom
  ) {
    return null;
  }

  const rows = Array.from(
    listElement.querySelectorAll<HTMLButtonElement>("[data-instance-id]"),
  ).filter((row) => {
    const instanceId = row.dataset.instanceId;
    return instanceId ? !draggedInstanceIds.includes(instanceId) : false;
  });
  if (rows.length === 0) {
    return null;
  }

  for (const row of rows) {
    const anchorInstanceId = row.dataset.instanceId;
    if (!anchorInstanceId) {
      continue;
    }

    const rowBounds = row.getBoundingClientRect();
    const rowMidpoint = rowBounds.top + rowBounds.height / 2;
    if (clientY < rowMidpoint) {
      return {
        anchorInstanceId,
        position: "above",
      } satisfies FrameDropTargetState;
    }
  }

  const lastRow = rows[rows.length - 1];
  const anchorInstanceId = lastRow?.dataset.instanceId;
  if (!anchorInstanceId) {
    return null;
  }

  return {
    anchorInstanceId,
    position: "below",
  } satisfies FrameDropTargetState;
}

export function releasePointerCaptureIfHeld(
  element: HTMLButtonElement,
  pointerId: number,
) {
  if (element.hasPointerCapture(pointerId)) {
    element.releasePointerCapture(pointerId);
  }
}
