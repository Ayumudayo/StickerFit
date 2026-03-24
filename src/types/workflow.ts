import type { CropRegion } from "../components/MediaSelectionPreview";
import type { Locale } from "../locales/messages";

export type FitMode = "contain" | "cover" | "fill";

export type OptimizerPresetStrategy = "auto" | "quality" | "size";

export type OptimizerSearchDepth = "standard" | "thorough";

export type InputSourceKind = "path" | "file";

export type TimelineFrameRequest = {
  sourceFrameId: number;
  durationUs: number;
};

export type OptimizerPlanRequest = {
  locale: Locale;
  sourceDurationSeconds: number | null;
  inputWidth: number | null;
  inputHeight: number | null;
  avgFps: number | null;
  fitMode: FitMode;
  presetStrategy: OptimizerPresetStrategy;
  searchDepth: OptimizerSearchDepth;
  cropRegion: CropRegion | null;
  selectedFrames?: number[];
  baseFrameCount?: number;
  timelineFrames?: TimelineFrameRequest[];
};

export type OptimizerSearchRequest = OptimizerPlanRequest & {
  inputPath: string;
  outputDirectory: string | null;
};

export type StaticImageConversionRequest = {
  inputPath: string;
  outputDirectory: string | null;
  locale: Locale;
  cropRegion: CropRegion;
};

export type ToolCheck = {
  tool: string;
  available: boolean;
  source: "sidecar" | "missing";
  resolvedCommand: string | null;
  fallbackReason: string | null;
  versionLine: string | null;
  detail: string;
  expectedSidecarName: string;
  attemptedSidecarPaths: string[];
};

export type ToolHealthReport = {
  ready: boolean;
  checks: ToolCheck[];
  summary: string;
};

export type MediaInspection = {
  ok: boolean;
  inputPath: string;
  backendInputPath: string | null;
  previewSrc: string;
  inputSourceKind: InputSourceKind;
  toolSource: string | null;
  toolCommand: string | null;
  toolDetail: string | null;
  formatName: string | null;
  durationSeconds: number | null;
  sizeBytes: number | null;
  width: number | null;
  height: number | null;
  codecName: string | null;
  pixelFormat: string | null;
  avgFps: number | null;
  frameRateLabel: string | null;
  estimatedFrames: number | null;
  frameDurationsSeconds: number[] | null;
  isStaticImage: boolean;
  canConvertToPng: boolean;
  errorCode: string | null;
  errorMessage: string | null;
};

export type OptimizerCandidatePreview = {
  id: string;
  rank: number;
  durationSeconds: number;
  fps: number;
  contentScale: number;
  preset: string;
  fitMode: FitMode;
  score: number;
  sourceSimilarityScore: number;
  summary: string;
};

export type OptimizerPlanResponse = {
  ok: boolean;
  fitMode: FitMode;
  selectedDurationSeconds: number | null;
  recommendedMaxDurationSeconds: number;
  searchBudget: number;
  warnings: string[];
  candidates: OptimizerCandidatePreview[];
  errorCode: string | null;
  errorMessage: string | null;
};

export type EncodedCandidateResult = {
  ok: boolean;
  candidateId: string;
  outputPath: string | null;
  sizeBytes: number | null;
  elapsedMs: number | null;
  toolSource: string | null;
  toolCommand: string | null;
  toolDetail: string | null;
  warnings: string[];
  errorCode: string | null;
  errorMessage: string | null;
};

export type StaticImageConversionResult = {
  ok: boolean;
  outputPath: string | null;
  sizeBytes: number | null;
  elapsedMs: number | null;
  toolSource: string | null;
  toolCommand: string | null;
  toolDetail: string | null;
  warnings: string[];
  errorCode: string | null;
  errorMessage: string | null;
};

export type SearchAttemptResult = {
  candidateId: string;
  canonicalCandidateId: string;
  equivalentToCandidateId: string | null;
  rank: number;
  durationSeconds: number;
  fps: number;
  contentScale: number;
  preset: string;
  fitMode: FitMode;
  score: number;
  sourceSimilarityScore: number;
  summary: string;
  skipped: boolean;
  withinLimit: boolean;
  outputPath: string | null;
  sizeBytes: number | null;
  elapsedMs: number | null;
  toolSource: string | null;
  toolCommand: string | null;
  toolDetail: string | null;
  warnings: string[];
  errorCode: string | null;
  errorMessage: string | null;
};

export type OptimizerSearchResponse = {
  ok: boolean;
  fitMode: FitMode;
  selectedDurationSeconds: number | null;
  limitBytes: number;
  searchBudget: number;
  realAttemptCount: number;
  stopReason: string | null;
  selectionReason: "best_within_limit" | "smallest_oversize" | "no_fit_found";
  summary: string;
  warnings: string[];
  attempts: SearchAttemptResult[];
  winningCandidateId: string | null;
  closestCandidateId: string | null;
  bestOutputPath: string | null;
  bestSizeBytes: number | null;
  bestWithinLimit: boolean;
  errorCode: string | null;
  errorMessage: string | null;
};

