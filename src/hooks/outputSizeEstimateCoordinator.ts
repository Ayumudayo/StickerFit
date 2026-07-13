import type {
  MediaOperationOptions,
  NormalizedMediaError,
} from "../platform/runtime";
import type {
  ExactCandidateSizeEstimate,
  MediaOperationErrorCode,
  MediaOperationReasonCode,
  OperationProgress,
  OptimizerCandidatePreview,
  OutputSizeEstimate,
} from "../types/workflow";
import type { VersionedWorkflowState } from "./mediaWorkflow/workflowFingerprint";

const DEFAULT_ESTIMATE_DEBOUNCE_MS = 400;
const MAX_EXACT_ESTIMATE_CACHE_ENTRIES = 20;

export type VersionedProbeState =
  | { status: "idle"; fingerprint: string; revision: number }
  | {
      status: "loading";
      fingerprint: string;
      revision: number;
      candidateId: string;
      progress: OperationProgress | null;
    }
  | {
      status: "ready";
      fingerprint: string;
      revision: number;
      candidateId: string;
      value: ExactCandidateSizeEstimate;
    }
  | {
      status: "error";
      fingerprint: string;
      revision: number;
      candidateId: string;
      progress: OperationProgress | null;
      code: MediaOperationErrorCode;
      reasonCode: MediaOperationReasonCode | null;
      message: string | null;
    }
  | {
      status: "cancelled";
      fingerprint: string;
      revision: number;
      candidateId: string;
      progress: OperationProgress | null;
    };

export type OutputEstimateCoordinatorState = {
  estimate: VersionedWorkflowState<OutputSizeEstimate[], OperationProgress>;
  probe: VersionedProbeState;
  exactEstimateCache: ReadonlyMap<string, ExactCandidateSizeEstimate>;
};

export type EstimateSchedule = Readonly<{
  fingerprint: string;
  requestKey: string;
  run: (options: MediaOperationOptions) => Promise<OutputSizeEstimate[]>;
}>;

export type ProbeSchedule = Readonly<{
  fingerprint: string;
  candidateId: string;
  run: (options: MediaOperationOptions) => Promise<ExactCandidateSizeEstimate>;
}>;

export type OutputSizeEstimateCoordinator = {
  getState(): OutputEstimateCoordinatorState;
  scheduleEstimate(
    schedule: EstimateSchedule,
    options?: { force?: boolean },
  ): void;
  retryEstimate(): void;
  startProbe(schedule: ProbeSchedule): void;
  cancelProbe(): void;
  invalidate(fingerprint: string): void;
  dispose(): void;
};

type CoordinatorOptions = {
  initialFingerprint: string;
  publish: (state: OutputEstimateCoordinatorState) => void;
  createOperationId: () => string;
  normalizeError: (error: unknown) => NormalizedMediaError;
  debounceMs?: number;
};

function initialState(fingerprint: string): OutputEstimateCoordinatorState {
  return {
    estimate: {
      status: "idle",
      revision: 0,
      fingerprint,
    },
    probe: {
      status: "idle",
      revision: 0,
      fingerprint,
    },
    exactEstimateCache: new Map(),
  };
}

function normalizedErrorCode(error: NormalizedMediaError) {
  return error.errorCode ?? "internal-task-failed";
}

export function selectCandidatesForEstimate(
  candidates: readonly OptimizerCandidatePreview[],
) {
  const compareIds = (
    left: OptimizerCandidatePreview,
    right: OptimizerCandidatePreview,
  ) => (left.id < right.id ? -1 : left.id > right.id ? 1 : 0);
  const byRank = [...candidates].sort(
    (left, right) => left.rank - right.rank || compareIds(left, right),
  );
  const byRelativeSize = [...candidates].sort(
    (left, right) =>
      left.relativeSizeFactor - right.relativeSizeFactor ||
      left.rank - right.rank ||
      compareIds(left, right),
  );
  const selected: OptimizerCandidatePreview[] = [];
  const selectedIds = new Set<string>();

  for (const candidate of [
    ...byRank.slice(0, 3),
    ...byRelativeSize.slice(0, 2),
  ]) {
    if (selectedIds.has(candidate.id)) continue;
    selectedIds.add(candidate.id);
    selected.push(candidate);
    if (selected.length === 5) break;
  }

  return selected;
}

export function sampleSeedFromFingerprint(fingerprint: string) {
  let hash = 0xcbf29ce484222325n;
  for (let index = 0; index < fingerprint.length; index += 1) {
    hash ^= BigInt(fingerprint.charCodeAt(index));
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }
  return hash.toString(16).padStart(16, "0");
}

export function exactEstimateCacheKey(
  fingerprint: string,
  candidateId: string,
) {
  return JSON.stringify([fingerprint, candidateId]);
}

function cacheExactEstimate(
  cache: ReadonlyMap<string, ExactCandidateSizeEstimate>,
  key: string,
  value: ExactCandidateSizeEstimate,
) {
  const next = new Map(cache);
  next.delete(key);
  next.set(key, value);
  while (next.size > MAX_EXACT_ESTIMATE_CACHE_ENTRIES) {
    const oldestKey = next.keys().next().value;
    if (oldestKey === undefined) break;
    next.delete(oldestKey);
  }
  return next;
}

export function createOutputSizeEstimateCoordinator({
  initialFingerprint,
  publish,
  createOperationId,
  normalizeError,
  debounceMs = DEFAULT_ESTIMATE_DEBOUNCE_MS,
}: CoordinatorOptions): OutputSizeEstimateCoordinator {
  let state = initialState(initialFingerprint);
  let activeFingerprint = initialFingerprint;
  let estimateRevision = 0;
  let probeRevision = 0;
  let estimateRequestKey: string | null = null;
  let estimateTimer: ReturnType<typeof globalThis.setTimeout> | null = null;
  let estimateController: AbortController | null = null;
  let probeController: AbortController | null = null;
  let lastEstimateSchedule: EstimateSchedule | null = null;
  let disposed = false;

  function setState(next: OutputEstimateCoordinatorState) {
    state = next;
    publish(next);
  }

  function setEstimate(
    estimate: VersionedWorkflowState<OutputSizeEstimate[], OperationProgress>,
  ) {
    setState({ ...state, estimate });
  }

  function setProbe(probe: VersionedProbeState) {
    setState({ ...state, probe });
  }

  function clearEstimateWork() {
    if (estimateTimer !== null) {
      globalThis.clearTimeout(estimateTimer);
      estimateTimer = null;
    }
    estimateController?.abort();
    estimateController = null;
  }

  function clearProbeWork() {
    probeController?.abort();
    probeController = null;
  }

  function estimateIsCurrent(
    fingerprint: string,
    requestKey: string,
    revision: number,
    controller?: AbortController,
  ) {
    return (
      !disposed &&
      activeFingerprint === fingerprint &&
      estimateRequestKey === requestKey &&
      estimateRevision === revision &&
      (controller === undefined || estimateController === controller)
    );
  }

  function probeIsCurrent(
    fingerprint: string,
    candidateId: string,
    revision: number,
    controller: AbortController,
  ) {
    return (
      !disposed &&
      activeFingerprint === fingerprint &&
      probeRevision === revision &&
      probeController === controller &&
      state.probe.status === "loading" &&
      state.probe.fingerprint === fingerprint &&
      state.probe.candidateId === candidateId &&
      state.probe.revision === revision
    );
  }

  function beginEstimate(schedule: EstimateSchedule) {
    clearEstimateWork();
    estimateRevision += 1;
    estimateRequestKey = schedule.requestKey;
    lastEstimateSchedule = schedule;
    const revision = estimateRevision;
    const { fingerprint, requestKey } = schedule;

    setEstimate({
      status: "loading",
      revision,
      fingerprint,
      progress: null,
    });

    estimateTimer = globalThis.setTimeout(
      () => {
        estimateTimer = null;
        if (!estimateIsCurrent(fingerprint, requestKey, revision)) return;

        const currentSchedule = lastEstimateSchedule;
        if (
          currentSchedule === null ||
          currentSchedule.fingerprint !== fingerprint ||
          currentSchedule.requestKey !== requestKey
        ) {
          return;
        }

        const controller = new AbortController();
        estimateController = controller;
        const options: MediaOperationOptions = {
          operationId: createOperationId(),
          signal: controller.signal,
          onProgress: (progress) => {
            if (
              !estimateIsCurrent(fingerprint, requestKey, revision, controller)
            )
              return;
            const current = state.estimate;
            if (
              current.status !== "loading" ||
              current.revision !== revision ||
              current.fingerprint !== fingerprint
            ) {
              return;
            }
            setEstimate({ ...current, progress });
          },
        };

        void Promise.resolve()
          .then(() => currentSchedule.run(options))
          .then((value) => {
            if (
              !estimateIsCurrent(fingerprint, requestKey, revision, controller)
            )
              return;
            setEstimate({
              status: "ready",
              revision,
              fingerprint,
              value,
            });
          })
          .catch((error: unknown) => {
            if (
              !estimateIsCurrent(fingerprint, requestKey, revision, controller)
            )
              return;
            const normalized = normalizeError(error);
            if (
              controller.signal.aborted ||
              normalized.errorCode === "cancelled"
            ) {
              setEstimate({
                status: "cancelled",
                revision,
                fingerprint,
              });
              return;
            }
            setEstimate({
              status: "error",
              revision,
              fingerprint,
              code: normalizedErrorCode(normalized),
              reasonCode: normalized.reasonCode,
              message: normalized.diagnostics ?? "",
            });
          })
          .finally(() => {
            if (estimateController === controller) {
              estimateController = null;
            }
          });
      },
      Math.max(0, debounceMs),
    );
  }

  function scheduleEstimate(
    schedule: EstimateSchedule,
    options: { force?: boolean } = {},
  ) {
    if (disposed) return;
    if (schedule.fingerprint !== activeFingerprint) {
      invalidate(schedule.fingerprint);
    }
    if (disposed) return;

    const sameRequest = estimateRequestKey === schedule.requestKey;
    lastEstimateSchedule = schedule;
    if (sameRequest && !options.force) {
      return;
    }
    beginEstimate(schedule);
  }

  function retryEstimate() {
    if (disposed || lastEstimateSchedule === null) return;
    if (lastEstimateSchedule.fingerprint !== activeFingerprint) return;
    beginEstimate(lastEstimateSchedule);
  }

  function startProbe(schedule: ProbeSchedule) {
    if (disposed || schedule.fingerprint !== activeFingerprint) return;

    clearProbeWork();
    probeRevision += 1;
    const revision = probeRevision;
    const { fingerprint, candidateId } = schedule;
    const controller = new AbortController();
    probeController = controller;
    setProbe({
      status: "loading",
      fingerprint,
      revision,
      candidateId,
      progress: null,
    });

    const options: MediaOperationOptions = {
      operationId: createOperationId(),
      signal: controller.signal,
      onProgress: (progress) => {
        if (!probeIsCurrent(fingerprint, candidateId, revision, controller))
          return;
        const current = state.probe;
        if (current.status !== "loading") return;
        setProbe({ ...current, progress });
      },
    };

    void Promise.resolve()
      .then(() => schedule.run(options))
      .then((value) => {
        if (!probeIsCurrent(fingerprint, candidateId, revision, controller))
          return;
        if (value.candidateId !== candidateId) {
          throw new Error(
            "Candidate probe returned a mismatched candidate ID.",
          );
        }
        const exactEstimateCache = cacheExactEstimate(
          state.exactEstimateCache,
          exactEstimateCacheKey(fingerprint, candidateId),
          value,
        );
        setState({
          ...state,
          exactEstimateCache,
          probe: {
            status: "ready",
            fingerprint,
            revision,
            candidateId,
            value,
          },
        });
      })
      .catch((error: unknown) => {
        if (!probeIsCurrent(fingerprint, candidateId, revision, controller))
          return;
        const current = state.probe;
        const progress = current.status === "loading" ? current.progress : null;
        const normalized = normalizeError(error);
        if (controller.signal.aborted || normalized.errorCode === "cancelled") {
          setProbe({
            status: "cancelled",
            fingerprint,
            revision,
            candidateId,
            progress,
          });
          return;
        }
        setProbe({
          status: "error",
          fingerprint,
          revision,
          candidateId,
          progress,
          code: normalizedErrorCode(normalized),
          reasonCode: normalized.reasonCode,
          message: normalized.diagnostics,
        });
      })
      .finally(() => {
        if (probeController === controller) {
          probeController = null;
        }
      });
  }

  function cancelProbe() {
    if (disposed || state.probe.status !== "loading") return;
    const current = state.probe;
    probeController?.abort();
    probeController = null;
    setProbe({
      status: "cancelled",
      fingerprint: current.fingerprint,
      revision: current.revision,
      candidateId: current.candidateId,
      progress: current.progress,
    });
  }

  function invalidate(fingerprint: string) {
    if (disposed || fingerprint === activeFingerprint) return;
    clearEstimateWork();
    clearProbeWork();
    activeFingerprint = fingerprint;
    estimateRevision += 1;
    probeRevision += 1;
    estimateRequestKey = null;
    lastEstimateSchedule = null;
    setState({
      estimate: {
        status: "idle",
        revision: estimateRevision,
        fingerprint,
      },
      probe: {
        status: "idle",
        revision: probeRevision,
        fingerprint,
      },
      exactEstimateCache: state.exactEstimateCache,
    });
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    clearEstimateWork();
    clearProbeWork();
    estimateRevision += 1;
    probeRevision += 1;
    estimateRequestKey = null;
    lastEstimateSchedule = null;
  }

  return {
    getState: () => state,
    scheduleEstimate,
    retryEstimate,
    startProbe,
    cancelProbe,
    invalidate,
    dispose,
  };
}
