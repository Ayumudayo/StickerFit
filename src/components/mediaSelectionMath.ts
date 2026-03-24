import type { MessagesForLocale } from "../locales/messages";

export type CropRegion = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type CropAspectRatioPreset = "free" | "1:1" | "4:3" | "3:4" | "16:9" | "9:16";
export type PreviewZoomMode = "fit" | "manual";
export type PreviewKind = "video" | "image";

export const CROP_ASPECT_RATIO_PRESETS = [
  "free",
  "1:1",
  "4:3",
  "3:4",
  "16:9",
  "9:16",
] satisfies CropAspectRatioPreset[];

export const FULL_CROP_REGION: CropRegion = { x: 0, y: 0, width: 1, height: 1 };

const MIN_CROP_REGION_SIZE = 0.08;
const VIDEO_EXTENSIONS = new Set(["mp4", "webm", "mov", "m4v"]);
const MIN_PREVIEW_ZOOM_SCALE = 0.001;

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export function clampUnit(value: number) {
  return clamp(value, 0, 1);
}

export function clampPreviewZoomScale(value: number) {
  return clamp(value, MIN_PREVIEW_ZOOM_SCALE, 4);
}

export function cropAspectRatioValue(preset: CropAspectRatioPreset) {
  switch (preset) {
    case "1:1":
      return 1;
    case "4:3":
      return 4 / 3;
    case "3:4":
      return 3 / 4;
    case "16:9":
      return 16 / 9;
    case "9:16":
      return 9 / 16;
    default:
      return null;
  }
}

export function normalizeCropRegion(region: CropRegion) {
  const width = clamp(region.width, MIN_CROP_REGION_SIZE, 1);
  const height = clamp(region.height, MIN_CROP_REGION_SIZE, 1);

  return {
    x: clamp(region.x, 0, 1 - width),
    y: clamp(region.y, 0, 1 - height),
    width,
    height,
  } satisfies CropRegion;
}

function clampCropRegionToBounds(region: CropRegion) {
  const width = clamp(region.width, 0.001, 1);
  const height = clamp(region.height, 0.001, 1);

  return {
    x: clamp(region.x, 0, 1 - width),
    y: clamp(region.y, 0, 1 - height),
    width,
    height,
  } satisfies CropRegion;
}

function cropRegionFromAnchoredAspectRatio(
  anchorX: number,
  anchorY: number,
  pointX: number,
  pointY: number,
  sourceWidth: number,
  sourceHeight: number,
  aspectRatio: number,
) {
  const anchorXPx = anchorX * sourceWidth;
  const anchorYPx = anchorY * sourceHeight;
  const pointXPx = pointX * sourceWidth;
  const pointYPx = pointY * sourceHeight;
  const dirX = pointXPx >= anchorXPx ? 1 : -1;
  const dirY = pointYPx >= anchorYPx ? 1 : -1;
  const maxWidthPx = dirX > 0 ? sourceWidth - anchorXPx : anchorXPx;
  const maxHeightPx = dirY > 0 ? sourceHeight - anchorYPx : anchorYPx;
  const minWidthPx = sourceWidth * MIN_CROP_REGION_SIZE;
  const minHeightPx = sourceHeight * MIN_CROP_REGION_SIZE;

  let widthPx = Math.abs(pointXPx - anchorXPx);
  let heightPx = Math.abs(pointYPx - anchorYPx);

  if (widthPx <= 0.001 && heightPx <= 0.001) {
    widthPx = minWidthPx;
    heightPx = minHeightPx;
  }

  if (heightPx <= 0.001) {
    heightPx = widthPx / aspectRatio;
  }
  if (widthPx <= 0.001) {
    widthPx = heightPx * aspectRatio;
  }

  if (widthPx / heightPx > aspectRatio) {
    widthPx = heightPx * aspectRatio;
  } else {
    heightPx = widthPx / aspectRatio;
  }

  const minScale = Math.max(
    minWidthPx / Math.max(widthPx, 0.001),
    minHeightPx / Math.max(heightPx, 0.001),
    1,
  );
  widthPx *= minScale;
  heightPx *= minScale;

  const fitScale = Math.min(maxWidthPx / widthPx, maxHeightPx / heightPx, 1);
  widthPx *= fitScale;
  heightPx *= fitScale;

  if (widthPx / heightPx > aspectRatio) {
    widthPx = heightPx * aspectRatio;
  } else {
    heightPx = widthPx / aspectRatio;
  }

  const leftPx = dirX > 0 ? anchorXPx : anchorXPx - widthPx;
  const topPx = dirY > 0 ? anchorYPx : anchorYPx - heightPx;

  return clampCropRegionToBounds({
    x: leftPx / sourceWidth,
    y: topPx / sourceHeight,
    width: widthPx / sourceWidth,
    height: heightPx / sourceHeight,
  });
}

export function cropRegionFromAnchor(
  anchorX: number,
  anchorY: number,
  pointX: number,
  pointY: number,
  sourceWidth: number,
  sourceHeight: number,
  aspectRatio: number,
) {
  return cropRegionFromAnchoredAspectRatio(
    anchorX,
    anchorY,
    pointX,
    pointY,
    sourceWidth,
    sourceHeight,
    aspectRatio,
  );
}

function resizeCropRegionFromHandleWithAspectRatio(
  initialRegion: CropRegion,
  handle: "nw" | "ne" | "sw" | "se",
  pointX: number,
  pointY: number,
  sourceWidth: number,
  sourceHeight: number,
  aspectRatio: number,
) {
  const left = initialRegion.x;
  const top = initialRegion.y;
  const right = initialRegion.x + initialRegion.width;
  const bottom = initialRegion.y + initialRegion.height;

  if (handle === "nw") {
    return cropRegionFromAnchoredAspectRatio(
      right,
      bottom,
      pointX,
      pointY,
      sourceWidth,
      sourceHeight,
      aspectRatio,
    );
  }

  if (handle === "ne") {
    return cropRegionFromAnchoredAspectRatio(
      left,
      bottom,
      pointX,
      pointY,
      sourceWidth,
      sourceHeight,
      aspectRatio,
    );
  }

  if (handle === "sw") {
    return cropRegionFromAnchoredAspectRatio(
      right,
      top,
      pointX,
      pointY,
      sourceWidth,
      sourceHeight,
      aspectRatio,
    );
  }

  return cropRegionFromAnchoredAspectRatio(
    left,
    top,
    pointX,
    pointY,
    sourceWidth,
    sourceHeight,
    aspectRatio,
  );
}

export function constrainCropRegionToAspectRatio(
  region: CropRegion,
  sourceWidth: number,
  sourceHeight: number,
  aspectRatio: number,
) {
  const centerXPx = (region.x + region.width / 2) * sourceWidth;
  const centerYPx = (region.y + region.height / 2) * sourceHeight;
  const minWidthPx = sourceWidth * MIN_CROP_REGION_SIZE;
  const minHeightPx = sourceHeight * MIN_CROP_REGION_SIZE;

  let widthPx = region.width * sourceWidth;
  let heightPx = region.height * sourceHeight;

  if (widthPx / heightPx > aspectRatio) {
    widthPx = heightPx * aspectRatio;
  } else {
    heightPx = widthPx / aspectRatio;
  }

  const minScale = Math.max(
    minWidthPx / Math.max(widthPx, 0.001),
    minHeightPx / Math.max(heightPx, 0.001),
    1,
  );
  widthPx *= minScale;
  heightPx *= minScale;

  const maxWidthPx = 2 * Math.min(centerXPx, sourceWidth - centerXPx);
  const maxHeightPx = 2 * Math.min(centerYPx, sourceHeight - centerYPx);
  const fitScale = Math.min(maxWidthPx / widthPx, maxHeightPx / heightPx, 1);
  widthPx *= fitScale;
  heightPx *= fitScale;

  if (widthPx / heightPx > aspectRatio) {
    widthPx = heightPx * aspectRatio;
  } else {
    heightPx = widthPx / aspectRatio;
  }

  return clampCropRegionToBounds({
    x: (centerXPx - widthPx / 2) / sourceWidth,
    y: (centerYPx - heightPx / 2) / sourceHeight,
    width: widthPx / sourceWidth,
    height: heightPx / sourceHeight,
  });
}

function cropRegionFromPoints(
  startX: number,
  startY: number,
  endX: number,
  endY: number,
) {
  const left = clamp(Math.min(startX, endX), 0, 1);
  const top = clamp(Math.min(startY, endY), 0, 1);
  const right = clamp(Math.max(startX, endX), 0, 1);
  const bottom = clamp(Math.max(startY, endY), 0, 1);

  return normalizeCropRegion({
    x: left,
    y: top,
    width: Math.max(MIN_CROP_REGION_SIZE, right - left),
    height: Math.max(MIN_CROP_REGION_SIZE, bottom - top),
  });
}

export function cropRegionsMatch(left: CropRegion, right: CropRegion) {
  return (
    Math.abs(left.x - right.x) < 0.001 &&
    Math.abs(left.y - right.y) < 0.001 &&
    Math.abs(left.width - right.width) < 0.001 &&
    Math.abs(left.height - right.height) < 0.001
  );
}

function isFullCropRegion(region: CropRegion) {
  return cropRegionsMatch(region, FULL_CROP_REGION);
}

export function isFullSelection(region: CropRegion) {
  return isFullCropRegion(region);
}

export function cropRegionFromStartPoint(pointX: number, pointY: number) {
  return normalizeCropRegion({
    x: pointX,
    y: pointY,
    width: MIN_CROP_REGION_SIZE,
    height: MIN_CROP_REGION_SIZE,
  });
}

export const MIN_CROP_REGION_RATIO = MIN_CROP_REGION_SIZE;

function extensionOf(path: string | null) {
  if (!path) {
    return null;
  }

  const lastDot = path.lastIndexOf(".");
  const lastSlash = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));

  if (lastDot < 0 || lastDot < lastSlash) {
    return null;
  }

  return path.slice(lastDot + 1).toLowerCase();
}

export function previewKindForPath(path: string | null): PreviewKind {
  const extension = extensionOf(path);
  return extension && VIDEO_EXTENSIONS.has(extension) ? "video" : "image";
}

export function selectionSummary(
  region: CropRegion,
  copy: MessagesForLocale,
  sourceWidth?: number | null,
  sourceHeight?: number | null,
) {
  if (isFullCropRegion(region)) {
    return copy.fullFrameSelection;
  }

  if (sourceWidth && sourceHeight) {
    const widthPx = Math.max(1, Math.round(region.width * sourceWidth));
    const heightPx = Math.max(1, Math.round(region.height * sourceHeight));

    return `${widthPx} x ${heightPx} px`;
  }

  return copy.customSelection(
    Math.round(region.width * 100),
    Math.round(region.height * 100),
  );
}

function resizeCropRegionFromHandle(
  initialRegion: CropRegion,
  handle: "nw" | "ne" | "sw" | "se",
  pointX: number,
  pointY: number,
) {
  const left = initialRegion.x;
  const top = initialRegion.y;
  const right = initialRegion.x + initialRegion.width;
  const bottom = initialRegion.y + initialRegion.height;

  if (handle === "nw") {
    const nextLeft = clamp(pointX, 0, right - MIN_CROP_REGION_SIZE);
    const nextTop = clamp(pointY, 0, bottom - MIN_CROP_REGION_SIZE);
    return {
      x: nextLeft,
      y: nextTop,
      width: right - nextLeft,
      height: bottom - nextTop,
    } satisfies CropRegion;
  }

  if (handle === "ne") {
    const nextRight = clamp(pointX, left + MIN_CROP_REGION_SIZE, 1);
    const nextTop = clamp(pointY, 0, bottom - MIN_CROP_REGION_SIZE);
    return {
      x: left,
      y: nextTop,
      width: nextRight - left,
      height: bottom - nextTop,
    } satisfies CropRegion;
  }

  if (handle === "sw") {
    const nextLeft = clamp(pointX, 0, right - MIN_CROP_REGION_SIZE);
    const nextBottom = clamp(pointY, top + MIN_CROP_REGION_SIZE, 1);
    return {
      x: nextLeft,
      y: top,
      width: right - nextLeft,
      height: nextBottom - top,
    } satisfies CropRegion;
  }

  const nextRight = clamp(pointX, left + MIN_CROP_REGION_SIZE, 1);
  const nextBottom = clamp(pointY, top + MIN_CROP_REGION_SIZE, 1);
  return {
    x: left,
    y: top,
    width: nextRight - left,
    height: nextBottom - top,
  } satisfies CropRegion;
}

export function nextCropRegionFromDrag(params: {
  dragKind: "create" | "move" | "resize";
  startX: number;
  startY: number;
  initialRegion: CropRegion;
  pointX: number;
  pointY: number;
  lockedAspectRatio: number | null;
  sourceWidth: number | null;
  sourceHeight: number | null;
  resizeHandle?: "nw" | "ne" | "sw" | "se";
}) {
  const {
    dragKind,
    startX,
    startY,
    initialRegion,
    pointX,
    pointY,
    lockedAspectRatio,
    sourceWidth,
    sourceHeight,
    resizeHandle,
  } = params;

  if (dragKind === "create") {
    return lockedAspectRatio && sourceWidth && sourceHeight
      ? cropRegionFromAnchoredAspectRatio(
        startX,
        startY,
        pointX,
        pointY,
        sourceWidth,
        sourceHeight,
        lockedAspectRatio,
      )
      : cropRegionFromPoints(startX, startY, pointX, pointY);
  }

  if (dragKind === "move") {
    return normalizeCropRegion({
      ...initialRegion,
      x: initialRegion.x + (pointX - startX),
      y: initialRegion.y + (pointY - startY),
    });
  }

  if (resizeHandle) {
    return lockedAspectRatio && sourceWidth && sourceHeight
      ? resizeCropRegionFromHandleWithAspectRatio(
        initialRegion,
        resizeHandle,
        pointX,
        pointY,
        sourceWidth,
        sourceHeight,
        lockedAspectRatio,
      )
      : normalizeCropRegion(
        resizeCropRegionFromHandle(
          initialRegion,
          resizeHandle,
          pointX,
          pointY,
        ),
      );
  }

  return initialRegion;
}
