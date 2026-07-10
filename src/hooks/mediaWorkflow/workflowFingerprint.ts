import type { CropRegion } from "../../components/MediaSelectionPreview";
import type { Locale } from "../../locales/messages";
import type {
  MediaOperationErrorCode,
  MediaOperationErrorFields,
  MediaOperationReasonCode,
  OptimizerGoal,
  OptimizerPresetStrategy,
  OptimizerSearchDepth,
  TimelineFrameRequest,
} from "../../types/workflow";

export type WorkflowFingerprints = {
  encoding: string;
  planner: string;
  export: string;
};

export type WorkflowFingerprintInput = {
  editorSessionKey: number;
  locale: Locale;
  inputPath: string | null;
  sourceRevision: string | null;
  outputDirectory: string | null;
  sourceDurationSeconds: number | null;
  inputWidth: number | null;
  inputHeight: number | null;
  avgFps: number | null;
  optimizerPresetStrategy: OptimizerPresetStrategy;
  optimizerGoal: OptimizerGoal;
  qualityFrameDropInterval: number;
  optimizerSearchDepth: OptimizerSearchDepth;
  cropRegion: CropRegion;
  baseFrameCount: number;
  timelineFrames: readonly TimelineFrameRequest[] | undefined;
};

export type VersionedWorkflowState<T, TProgress = never> =
  | { status: "idle"; revision: number; fingerprint: string }
  | {
      status: "loading";
      revision: number;
      fingerprint: string;
      progress: TProgress | null;
    }
  | {
      status: "ready";
      revision: number;
      fingerprint: string;
      value: T;
    }
  | {
      status: "error";
      revision: number;
      fingerprint: string;
      code: MediaOperationErrorCode;
      reasonCode: MediaOperationReasonCode | null;
      message: string;
    }
  | { status: "cancelled"; revision: number; fingerprint: string };

export type WorkflowResultEnvelope<T> = Readonly<{
  fingerprint: string;
  value: T;
}>;

export type WorkflowRequestTicket = Readonly<{
  revision: number;
  fingerprint: string;
}>;

export function buildWorkflowFingerprints(
  input: WorkflowFingerprintInput,
): WorkflowFingerprints {
  const encoding = JSON.stringify([
    input.editorSessionKey,
    input.inputPath,
    input.sourceRevision,
    input.sourceDurationSeconds,
    input.inputWidth,
    input.inputHeight,
    input.avgFps,
    [
      input.cropRegion.x,
      input.cropRegion.y,
      input.cropRegion.width,
      input.cropRegion.height,
    ],
    input.optimizerPresetStrategy,
    input.optimizerGoal,
    input.qualityFrameDropInterval,
    input.optimizerSearchDepth,
    input.baseFrameCount,
    input.timelineFrames?.map((frame) => [
      frame.sourceFrameId,
      frame.durationUs,
    ]) ?? null,
  ]);

  return {
    encoding,
    planner: JSON.stringify([encoding, input.locale]),
    export: JSON.stringify([encoding, input.locale, input.outputDirectory]),
  };
}

export function currentWorkflowState<T, TProgress = never>(
  state: VersionedWorkflowState<T, TProgress>,
  fingerprint: string,
) {
  return state.fingerprint === fingerprint ? state : null;
}

export function latestActiveWorkflowState(
  states: readonly (VersionedWorkflowState<unknown> | null)[],
) {
  let latest: VersionedWorkflowState<unknown> | null = null;

  for (const state of states) {
    if (state === null || state.status === "idle") {
      continue;
    }
    if (latest === null || state.revision > latest.revision) {
      latest = state;
    }
  }

  return latest;
}

export function isCurrentWorkflowRequest(
  ticket: WorkflowRequestTicket,
  currentRevision: number,
  currentFingerprint: string,
) {
  return (
    ticket.revision === currentRevision &&
    ticket.fingerprint === currentFingerprint
  );
}

export function workflowStateFromResult<T extends MediaOperationErrorFields>(
  value: T,
  revision: number,
  fingerprint: string,
): VersionedWorkflowState<T> {
  if (value.errorCode === "cancelled") {
    return { status: "cancelled", revision, fingerprint };
  }

  if (value.errorCode) {
    return {
      status: "error",
      revision,
      fingerprint,
      code: value.errorCode,
      reasonCode: value.reasonCode,
      message: value.errorMessage ?? "",
    };
  }

  return { status: "ready", revision, fingerprint, value };
}
