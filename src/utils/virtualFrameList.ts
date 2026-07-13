export const FRAME_RAIL_ROW_HEIGHT = 48;
export const FRAME_RAIL_OVERSCAN = 4;

type VirtualWindowParams = {
  scrollTop: number;
  viewportHeight: number;
  rowHeight: number;
  itemCount: number;
  overscan: number;
};

type RevealIndexParams = {
  scrollTop: number;
  viewportHeight: number;
  rowHeight: number;
  itemCount: number;
  index: number;
};

export type FrameRailNavigationKey =
  | "ArrowUp"
  | "ArrowDown"
  | "Home"
  | "End";

type RovingFrameIndexParams = {
  activeIndex: number;
  itemCount: number;
  key: FrameRailNavigationKey;
};

export type VirtualWindow = {
  start: number;
  end: number;
  offsetTop: number;
};

const EMPTY_VIRTUAL_WINDOW: VirtualWindow = {
  start: 0,
  end: 0,
  offsetTop: 0,
};

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, value));
}

function validListGeometry(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  itemCount: number,
) {
  return (
    Number.isFinite(scrollTop) &&
    Number.isFinite(viewportHeight) &&
    Number.isFinite(rowHeight) &&
    Number.isFinite(itemCount) &&
    viewportHeight > 0 &&
    rowHeight > 0 &&
    itemCount > 0
  );
}

function normalizedItemCount(itemCount: number) {
  if (!Number.isFinite(itemCount)) {
    return 0;
  }

  return Math.max(0, Math.floor(itemCount));
}

export function normalizeRovingFrameIndex(
  activeIndex: number,
  itemCount: number,
) {
  const count = normalizedItemCount(itemCount);
  if (count === 0) {
    return -1;
  }
  if (!Number.isFinite(activeIndex) || activeIndex < 0) {
    return 0;
  }

  return clamp(Math.floor(activeIndex), 0, count - 1);
}

export function computeRovingFrameIndex({
  activeIndex,
  itemCount,
  key,
}: RovingFrameIndexParams) {
  const count = normalizedItemCount(itemCount);
  if (count === 0) {
    return -1;
  }
  if (key === "Home") {
    return 0;
  }
  if (key === "End") {
    return count - 1;
  }
  if (!Number.isFinite(activeIndex) || activeIndex < 0) {
    return key === "ArrowUp" ? count - 1 : 0;
  }

  const currentIndex = normalizeRovingFrameIndex(activeIndex, count);
  return clamp(
    currentIndex + (key === "ArrowDown" ? 1 : -1),
    0,
    count - 1,
  );
}

export function computeVirtualWindow({
  scrollTop,
  viewportHeight,
  rowHeight,
  itemCount,
  overscan,
}: VirtualWindowParams): VirtualWindow {
  if (
    !validListGeometry(scrollTop, viewportHeight, rowHeight, itemCount) ||
    !Number.isFinite(overscan)
  ) {
    return { ...EMPTY_VIRTUAL_WINDOW };
  }

  const normalizedItemCount = Math.floor(itemCount);
  const normalizedOverscan = Math.max(0, Math.floor(overscan));
  const totalHeight = normalizedItemCount * rowHeight;
  if (!Number.isFinite(totalHeight) || normalizedItemCount <= 0) {
    return { ...EMPTY_VIRTUAL_WINDOW };
  }

  const maximumScrollTop = Math.max(0, totalHeight - viewportHeight);
  const effectiveScrollTop = clamp(scrollTop, 0, maximumScrollTop);
  const visibleStart = Math.floor(effectiveScrollTop / rowHeight);
  const visibleEnd = Math.ceil((effectiveScrollTop + viewportHeight) / rowHeight);
  const start = clamp(visibleStart - normalizedOverscan, 0, normalizedItemCount);
  const end = clamp(visibleEnd + normalizedOverscan, start, normalizedItemCount);

  return {
    start,
    end,
    offsetTop: start * rowHeight,
  };
}

export function computeScrollTopToRevealIndex({
  scrollTop,
  viewportHeight,
  rowHeight,
  itemCount,
  index,
}: RevealIndexParams) {
  if (
    !validListGeometry(scrollTop, viewportHeight, rowHeight, itemCount) ||
    !Number.isFinite(index)
  ) {
    return 0;
  }

  const normalizedItemCount = Math.floor(itemCount);
  const totalHeight = normalizedItemCount * rowHeight;
  if (!Number.isFinite(totalHeight) || normalizedItemCount <= 0) {
    return 0;
  }

  const maximumScrollTop = Math.max(0, totalHeight - viewportHeight);
  const effectiveScrollTop = clamp(scrollTop, 0, maximumScrollTop);
  const normalizedIndex = clamp(Math.floor(index), 0, normalizedItemCount - 1);
  const rowTop = normalizedIndex * rowHeight;
  const rowBottom = rowTop + rowHeight;

  if (rowTop < effectiveScrollTop) {
    return clamp(rowTop, 0, maximumScrollTop);
  }
  if (rowBottom > effectiveScrollTop + viewportHeight) {
    return clamp(rowBottom - viewportHeight, 0, maximumScrollTop);
  }
  return effectiveScrollTop;
}
