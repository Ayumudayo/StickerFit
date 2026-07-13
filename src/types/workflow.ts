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

export type MediaOperationProgressStage =
  | "queued"
  | "inspecting"
  | "decoding"
  | "estimating"
  | "encoding"
  | "finalizing";

export type MediaOperationProgressMessageCode =
  | "media-operation-queued"
  | "media-operation-inspecting"
  | "media-operation-decoding"
  | "media-operation-estimating"
  | "media-operation-encoding"
  | "media-operation-finalizing";

export type OperationProgress = Readonly<{
  operationId: string;
  stage: MediaOperationProgressStage;
  completed: number;
  total: number | null;
  messageCode: MediaOperationProgressMessageCode;
}>;

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
  sourceRevision: string;
  outputDirectory: string | null;
};

export type ExactStaticSizeEstimate = {
  kind: "exact-static";
  basis: "exact-static";
  bytes: number;
  candidateId: null;
  limitBytes: number;
  outputFrameCount: 1;
};

export type ExactCandidateSizeEstimate = {
  kind: "exact-candidate";
  basis: "exact-full-sequence" | "probe";
  bytes: number;
  candidateId: string;
  limitBytes: number;
  outputFrameCount: number;
};

export type SampledSizeEstimate = {
  kind: "range";
  basis: "sampled";
  lowerBytes: number;
  predictedBytes: number;
  upperBytes: number;
  confidence: "low" | "medium" | "high";
  candidateId: string;
  limitBytes: number;
  measuredContributionCount: number;
  outputFrameCount: number;
};

export type OutputSizeEstimate =
  ExactStaticSizeEstimate | ExactCandidateSizeEstimate | SampledSizeEstimate;

export type StaticSizeEstimateRequest = {
  inputPath: string;
  sourceRevision: string;
  locale: Locale;
  cropRegion: CropRegion | null;
};

export type OptimizerSizeEstimateRequest = OptimizerPlanRequest & {
  inputPath: string;
  sourceRevision: string;
  sampleSeed: string; // Exactly 16 lowercase hexadecimal characters.
  candidateIds: string[];
};

export type CandidateSizeProbeRequest = OptimizerPlanRequest & {
  inputPath: string;
  sourceRevision: string;
  candidateId: string;
};

export type StaticImageConversionRequest = {
  inputPath: string;
  sourceRevision: string;
  outputDirectory: string | null;
  locale: Locale;
  cropRegion: CropRegion;
};

export type FramePreviewRequest = {
  inputPath: string;
  sourceRevision: string;
  sourceFrameId: number;
  sourceWidth?: number | null;
  sourceHeight?: number | null;
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
  sourceRevision: string;
  sourceFrameIds: number[];
  sourceWidth?: number | null;
  sourceHeight?: number | null;
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

export type FramePreviewLoadState =
  | { status: "idle" }
  | { status: "loading"; batchToken: string }
  | { status: "ready"; dataUrl: string; width: number; height: number }
  | {
      status: "error";
      errorCode?: MediaOperationErrorCode;
      reasonCode?: MediaOperationReasonCode;
      message: string;
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

export type MediaInspectionFallbackReasonCode = "media-foundation-failed";

export type MediaInspection = {
  ok: boolean;
  inputPath: string;
  sourceRevision: string | null;
  backendInputPath: string | null;
  previewSrc: string;
  inputSourceKind: InputSourceKind;
  toolSource: string | null;
  toolCommand: string | null;
  toolDetail: string | null;
  fallbackReasonCode: MediaInspectionFallbackReasonCode | null;
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
  warnings: string[];
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
  relativeSizeFactor: number;
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
