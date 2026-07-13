import { describe, expect, it } from "vitest";

import type {
  MediaInspection,
  OptimizerPlanRequest,
} from "../../types/workflow";
import {
  buildFramePreviewRequest,
  buildFramePreviewsRequest,
  buildOptimizerSearchRequest,
  buildStaticImageConversionRequest,
} from "./mediaRequestBuilders";

function successfulInspection(
  overrides: Partial<MediaInspection> = {},
): MediaInspection {
  return {
    ok: true,
    inputPath: "C:\\media\\sticker.apng",
    sourceRevision: "opaque-revision-1",
    backendInputPath: "C:\\media\\sticker.apng",
    previewSrc: "asset://sticker.apng",
    inputSourceKind: "path",
    toolSource: "native",
    toolCommand: null,
    toolDetail: null,
    fallbackReasonCode: null,
    formatName: "apng",
    durationSeconds: 1,
    sizeBytes: 1024,
    width: 320,
    height: 240,
    codecName: "apng",
    pixelFormat: "rgba",
    avgFps: 12,
    frameRateLabel: "12",
    estimatedFrames: 12,
    frameDurationsSeconds: null,
    warnings: [],
    isStaticImage: false,
    canConvertToPng: false,
    errorCode: null,
    reasonCode: null,
    errorMessage: null,
    ...overrides,
  };
}

const optimizerPlan: OptimizerPlanRequest = {
  locale: "en",
  sourceDurationSeconds: 1,
  inputWidth: 320,
  inputHeight: 240,
  avgFps: 12,
  presetStrategy: "auto",
  optimizerGoal: "balanced",
  qualityFrameDropInterval: 2,
  searchDepth: "standard",
  cropRegion: null,
  selectedFrames: [0, 1],
  baseFrameCount: 2,
  timelineFrames: [
    { sourceFrameId: 1, durationUs: 83_333 },
    { sourceFrameId: 2, durationUs: 83_333 },
  ],
};

describe("path-backed media request builders", () => {
  it.each([
    [
      "failed inspection",
      successfulInspection({ ok: false, sourceRevision: null }),
    ],
    ["missing backend path", successfulInspection({ backendInputPath: null })],
    ["missing revision", successfulInspection({ sourceRevision: null })],
    ["empty revision", successfulInspection({ sourceRevision: "" })],
  ])("rejects %s before building a backend request", (_label, inspection) => {
    expect(
      buildOptimizerSearchRequest(inspection, {
        ...optimizerPlan,
        outputDirectory: "C:\\output",
      }),
    ).toBeNull();
  });

  it("serializes the controller optimizer-search request with source identity", () => {
    expect(
      buildOptimizerSearchRequest(successfulInspection(), {
        ...optimizerPlan,
        outputDirectory: "C:\\output",
      }),
    ).toEqual({
      ...optimizerPlan,
      inputPath: "C:\\media\\sticker.apng",
      sourceRevision: "opaque-revision-1",
      outputDirectory: "C:\\output",
    });
  });

  it("serializes the controller static-conversion request with source identity", () => {
    const inspection = successfulInspection({
      inputPath: "C:\\media\\sticker.bmp",
      backendInputPath: "C:\\media\\sticker.bmp",
      isStaticImage: true,
      canConvertToPng: true,
    });

    expect(
      buildStaticImageConversionRequest(inspection, {
        outputDirectory: null,
        locale: "ko",
        cropRegion: { x: 1, y: 2, width: 100, height: 80 },
      }),
    ).toEqual({
      inputPath: "C:\\media\\sticker.bmp",
      sourceRevision: "opaque-revision-1",
      outputDirectory: null,
      locale: "ko",
      cropRegion: { x: 1, y: 2, width: 100, height: 80 },
    });
  });

  it("serializes the App batch frame-preview request with source identity", () => {
    expect(
      buildFramePreviewsRequest(successfulInspection(), {
        sourceFrameIds: [1, 3, 8],
        locale: "en",
      }),
    ).toEqual({
      inputPath: "C:\\media\\sticker.apng",
      sourceRevision: "opaque-revision-1",
      sourceFrameIds: [1, 3, 8],
      locale: "en",
    });
  });

  it("serializes the single frame-preview request with source identity", () => {
    expect(
      buildFramePreviewRequest(successfulInspection(), {
        sourceFrameId: 3,
        locale: "ko",
      }),
    ).toEqual({
      inputPath: "C:\\media\\sticker.apng",
      sourceRevision: "opaque-revision-1",
      sourceFrameId: 3,
      locale: "ko",
    });
  });
});
