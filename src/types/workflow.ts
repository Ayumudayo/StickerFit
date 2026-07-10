import type { CropRegion } from "../components/MediaSelectionPreview";
import type { Locale } from "../locales/messages";

export type MediaOperationErrorCode =
  | "cancelled"
  | "timed-out"
  | "operation-conflict"
  | "invalid-request"
  | "source-changed"
  | "media-input-too-large"
  | "media-dimensions-too-large"
  | "media-frame-limit"
  | "decoded-byte-limit"
  | "png-chunk-limit"
  | "malformed-media"
  | "malformed-process-output"
  | "tool-missing"
  | "process-failed"
  | "output-conflict"
  | "internal-task-failed";

export type MediaOperationReasonCode =
  | "no-frames-selected"
  | "invalid-frame-selection"
  | "invalid-frame-duration"
  | "duration-too-long"
  | "invalid-crop"
  | "invalid-output-directory"
  | "unsupported-source-format"
  | "unsupported-frame-preview"
  | "frame-preview-decode-failed"
  | "frame-preview-encode-failed"
  | "decode-failed"
  | "encode-failed"
  | "missing-output"
  | "plan-invalid"
  | "invoke-failed";

export type MediaOperationErrorFields = {
  errorCode: MediaOperationErrorCode | null;
  reasonCode: MediaOperationReasonCode | null;
  errorMessage: string | null;
};

export type OptimizerPresetStrategy = "auto" | "quality" | "size";

export type OptimizerGoal = "balanced" | "motion" | "quality";

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
  presetStrategy: OptimizerPresetStrategy;
  optimizerGoal: OptimizerGoal;
  qualityFrameDropInterval: number;
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

export type FramePreviewRequest = {
  inputPath: string;
  sourceFrameId: number;
  locale: Locale;
};

export type FramePreviewResult = {
  ok: boolean;
  dataUrl: string | null;
  width: number | null;
  height: number | null;
} & MediaOperationErrorFields;

export type FramePreviewsRequest = {
  inputPath: string;
  sourceFrameIds: number[];
  locale: Locale;
};

export type FramePreviewItem = {
  sourceFrameId: number;
  dataUrl: string;
  width: number;
  height: number;
};

export type FramePreviewsResult = {
  ok: boolean;
  previews: FramePreviewItem[];
} & MediaOperationErrorFields;

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
  sourceRevision?: string | null;
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
} & MediaOperationErrorFields;

export type OptimizerCandidatePreview = {
  id: string;
  rank: number;
  durationSeconds: number;
  fps: number;
  contentScale: number;
  preset: string;
  score: number;
  sourceSimilarityScore: number;
  summary: string;
};

export type OptimizerPlanResponse = {
  ok: boolean;
  selectedDurationSeconds: number | null;
  recommendedMaxDurationSeconds: number;
  searchBudget: number;
  warnings: string[];
  candidates: OptimizerCandidatePreview[];
} & MediaOperationErrorFields;

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
} & MediaOperationErrorFields;

export type StaticImageConversionResult = {
  ok: boolean;
  outputPath: string | null;
  sizeBytes: number | null;
  elapsedMs: number | null;
  toolSource: string | null;
  toolCommand: string | null;
  toolDetail: string | null;
  warnings: string[];
} & MediaOperationErrorFields;

export type SearchAttemptResult = {
  candidateId: string;
  canonicalCandidateId: string;
  equivalentToCandidateId: string | null;
  rank: number;
  durationSeconds: number;
  fps: number;
  contentScale: number;
  preset: string;
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
} & MediaOperationErrorFields;

export type OptimizerSearchResponse = {
  ok: boolean;
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
} & MediaOperationErrorFields;

