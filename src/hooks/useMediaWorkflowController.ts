import { useCallback, useEffect, useRef, useState } from "react";

import {
  FULL_CROP_REGION,
  type CropAspectRatioPreset,
  type CropRegion,
} from "../components/MediaSelectionPreview";
import { type Locale } from "../locales/messages";
import {
  getAppRuntime,
  normalizeLegacyMediaError,
} from "../platform/runtime";
import { createMediaOperationId } from "../platform/mediaOperationId";
import type {
  MediaInspection,
  OptimizerGoal,
  OptimizerPlanRequest,
  OptimizerPlanResponse,
  OptimizerPresetStrategy,
  OptimizerSearchDepth,
  OptimizerSearchResponse,
  OperationProgress,
  StaticImageConversionResult,
  TimelineFrameRequest,
} from "../types/workflow";
import {
  buildOptimizerSearchRequest,
  buildStaticImageConversionRequest,
} from "./mediaWorkflow/mediaRequestBuilders";
import { normalizeOptimizerStopReason } from "../utils/outputSizeEstimate";
import {
  isCurrentWorkflowRequest,
  workflowStateFromResult,
  type VersionedWorkflowState,
  type WorkflowFingerprints,
  type WorkflowRequestTicket,
  type WorkflowResultEnvelope,
} from "./mediaWorkflow/workflowFingerprint";
import { useMediaInputSelector } from "./mediaWorkflow/useMediaInputSelector";
import { useToolHealthReport } from "./mediaWorkflow/useToolHealthReport";

type UseMediaWorkflowControllerParams = {
  locale: Locale;
  initialLocale: Locale;
  onCommitEditorSession: () => void;
  getCurrentWorkflowFingerprints: () => WorkflowFingerprints;
};

type WorkflowRequestContext = {
  baseFrameCount: number;
  editedTimelineFramesForRequest: TimelineFrameRequest[] | undefined;
  fingerprint: string;
};

export type OptimizerBaseRequestContext = Omit<
  WorkflowRequestContext,
  "fingerprint"
> & {
  inspection: MediaInspection;
  locale: Locale;
  presetStrategy: OptimizerPresetStrategy;
  optimizerGoal: OptimizerGoal;
  qualityFrameDropInterval: number;
  searchDepth: OptimizerSearchDepth;
  cropRegion: CropRegion;
};

type WorkflowFingerprintKind = "planner" | "export";

export function buildOptimizerPlanRequest({
  inspection,
  locale,
  presetStrategy,
  optimizerGoal,
  qualityFrameDropInterval,
  searchDepth,
  cropRegion,
  baseFrameCount,
  editedTimelineFramesForRequest,
}: OptimizerBaseRequestContext) {
  return {
    locale,
    sourceDurationSeconds: inspection.durationSeconds,
    inputWidth: inspection.width,
    inputHeight: inspection.height,
    avgFps: inspection.avgFps,
    presetStrategy,
    optimizerGoal,
    qualityFrameDropInterval,
    searchDepth,
    cropRegion,
    selectedFrames: undefined,
    baseFrameCount,
    timelineFrames: editedTimelineFramesForRequest,
  } satisfies OptimizerPlanRequest;
}

export function useMediaWorkflowController({
  locale,
  initialLocale,
  onCommitEditorSession,
  getCurrentWorkflowFingerprints,
}: UseMediaWorkflowControllerParams) {
  const runtime = getAppRuntime();
  const [planState, setPlanState] = useState<
    VersionedWorkflowState<OptimizerPlanResponse, OperationProgress>
  >({
    status: "idle",
    revision: 0,
    fingerprint: "",
  });
  const [searchState, setSearchState] = useState<
    VersionedWorkflowState<OptimizerSearchResponse, OperationProgress>
  >({
    status: "idle",
    revision: 0,
    fingerprint: "",
  });
  const [conversionState, setConversionState] = useState<
    VersionedWorkflowState<StaticImageConversionResult, OperationProgress>
  >({
    status: "idle",
    revision: 0,
    fingerprint: "",
  });
  const [optimizerPresetStrategy, setOptimizerPresetStrategy] =
    useState<OptimizerPresetStrategy>("auto");
  const [optimizerGoal, setOptimizerGoal] =
    useState<OptimizerGoal>("balanced");
  const [qualityFrameDropInterval, setQualityFrameDropInterval] = useState(3);
  const [optimizerSearchDepth, setOptimizerSearchDepth] =
    useState<OptimizerSearchDepth>("standard");
  const [cropRegion, setCropRegion] =
    useState<CropRegion>(FULL_CROP_REGION);
  const [cropAspectRatioPreset, setCropAspectRatioPreset] =
    useState<CropAspectRatioPreset>("free");

  const planTicketRef = useRef(0);
  const searchTicketRef = useRef(0);
  const conversionTicketRef = useRef(0);
  const planAbortControllerRef = useRef<AbortController | null>(null);
  const searchAbortControllerRef = useRef<AbortController | null>(null);
  const conversionAbortControllerRef = useRef<AbortController | null>(null);
  const workflowRevisionRef = useRef(0);
  const invalidatedFingerprintsRef = useRef<WorkflowFingerprints | null>(null);
  const mountedRef = useRef(true);

  const nextWorkflowRevision = useCallback(() => {
    workflowRevisionRef.current += 1;
    return workflowRevisionRef.current;
  }, []);

  const invalidateWorkflowResults = useCallback(
    (fingerprints: WorkflowFingerprints, force = false) => {
      const previous = invalidatedFingerprintsRef.current;

      if (force || previous?.planner !== fingerprints.planner) {
        planAbortControllerRef.current?.abort();
        planTicketRef.current += 1;
        setPlanState({
          status: "idle",
          revision: nextWorkflowRevision(),
          fingerprint: fingerprints.planner,
        });
      }

      if (force || previous?.export !== fingerprints.export) {
        searchAbortControllerRef.current?.abort();
        conversionAbortControllerRef.current?.abort();
        searchTicketRef.current += 1;
        conversionTicketRef.current += 1;
        setSearchState({
          status: "idle",
          revision: nextWorkflowRevision(),
          fingerprint: fingerprints.export,
        });
        setConversionState({
          status: "idle",
          revision: nextWorkflowRevision(),
          fingerprint: fingerprints.export,
        });
      }

      invalidatedFingerprintsRef.current = fingerprints;
    },
    [nextWorkflowRevision],
  );

  const isCurrentOperation = useCallback(
    (
      kind: WorkflowFingerprintKind,
      ticket: WorkflowRequestTicket,
      currentTicket: number,
    ) =>
      mountedRef.current &&
      isCurrentWorkflowRequest(
        ticket,
        currentTicket,
        getCurrentWorkflowFingerprints()[kind],
      ),
    [getCurrentWorkflowFingerprints],
  );

  const isCurrentProgressUpdate = useCallback(
    (
      kind: WorkflowFingerprintKind,
      ticket: WorkflowRequestTicket,
      currentTicket: number,
      controllerRef: { current: AbortController | null },
      controller: AbortController,
    ) =>
      mountedRef.current &&
      controllerRef.current === controller &&
      isCurrentWorkflowRequest(
        ticket,
        currentTicket,
        getCurrentWorkflowFingerprints()[kind],
      ),
    [getCurrentWorkflowFingerprints],
  );

  const resetForNewInspection = useCallback(() => {
    invalidateWorkflowResults(getCurrentWorkflowFingerprints(), true);
    setCropRegion(FULL_CROP_REGION);
    setCropAspectRatioPreset("free");
  }, [getCurrentWorkflowFingerprints, invalidateWorkflowResults]);

  const { toolReport, toolError } = useToolHealthReport(
    runtime,
    initialLocale,
  );
  const {
    inspection,
    inspectionLoading,
    isDragging,
    outputDirectory,
    setOutputDirectory,
    pickInputFile,
    pickOutputDirectory,
    openOutputFolder,
  } = useMediaInputSelector({
    runtime,
    locale,
    onResetForNewInspection: resetForNewInspection,
    onCommitEditorSession,
  });

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      planAbortControllerRef.current?.abort();
      searchAbortControllerRef.current?.abort();
      conversionAbortControllerRef.current?.abort();
      planTicketRef.current += 1;
      searchTicketRef.current += 1;
      conversionTicketRef.current += 1;
    };
  }, []);

  const buildPlan = useCallback(
    async ({
      baseFrameCount,
      editedTimelineFramesForRequest,
      fingerprint,
    }: WorkflowRequestContext): Promise<
      WorkflowResultEnvelope<OptimizerPlanResponse> | null
    > => {
      if (!inspection?.ok || inspection.isStaticImage) {
        return null;
      }

      planAbortControllerRef.current?.abort();
      searchAbortControllerRef.current?.abort();
      const controller = new AbortController();
      planAbortControllerRef.current = controller;
      const operationId = createMediaOperationId();

      searchTicketRef.current += 1;
      setSearchState({
        status: "idle",
        revision: nextWorkflowRevision(),
        fingerprint: getCurrentWorkflowFingerprints().export,
      });

      const ticket: WorkflowRequestTicket = {
        revision: planTicketRef.current + 1,
        fingerprint,
      };
      planTicketRef.current = ticket.revision;
      const stateRevision = nextWorkflowRevision();
      setPlanState({
        status: "loading",
        revision: stateRevision,
        fingerprint,
        progress: null,
      });

      try {
        if (!runtime.capabilities.backendProcessing) {
          throw new Error(
            "Desktop optimization is unavailable in browser preview mode.",
          );
        }

        const request = buildOptimizerPlanRequest({
          inspection,
          locale,
          presetStrategy: optimizerPresetStrategy,
          optimizerGoal,
          qualityFrameDropInterval,
          searchDepth: optimizerSearchDepth,
          cropRegion,
          baseFrameCount,
          editedTimelineFramesForRequest,
        });
        const result = await runtime.buildOptimizerPlan(request, {
          operationId,
          signal: controller.signal,
          onProgress: (progress) => {
            if (
              !isCurrentProgressUpdate(
                "planner",
                ticket,
                planTicketRef.current,
                planAbortControllerRef,
                controller,
              )
            ) {
              return;
            }
            setPlanState((current) =>
              current.status === "loading" &&
              current.revision === stateRevision &&
              current.fingerprint === fingerprint
                ? { ...current, progress }
                : current,
            );
          },
        });
        if (
          !isCurrentOperation(
            "planner",
            ticket,
            planTicketRef.current,
          )
        ) {
          return null;
        }

        const nextState = workflowStateFromResult(
          result,
          stateRevision,
          fingerprint,
        );
        setPlanState(nextState);
        return nextState.status === "cancelled"
          ? null
          : { fingerprint, value: result };
      } catch (error) {
        if (
          !isCurrentOperation(
            "planner",
            ticket,
            planTicketRef.current,
          )
        ) {
          return null;
        }

        const normalized = normalizeLegacyMediaError(error);
        setPlanState(
          normalized.errorCode === "cancelled"
            ? { status: "cancelled", revision: stateRevision, fingerprint }
            : {
                status: "error",
                revision: stateRevision,
                fingerprint,
                code: normalized.errorCode ?? "internal-task-failed",
                reasonCode: normalized.reasonCode,
                message: normalized.diagnostics ?? "",
              },
        );
        return null;
      } finally {
        if (
          isCurrentOperation(
            "planner",
            ticket,
            planTicketRef.current,
          )
        ) {
          setPlanState((current) =>
            current.status === "loading" &&
            current.revision === stateRevision &&
            current.fingerprint === fingerprint
              ? {
                  status: "cancelled",
                  revision: stateRevision,
                  fingerprint,
                }
              : current,
          );
        }
        if (planAbortControllerRef.current === controller) {
          planAbortControllerRef.current = null;
        }
      }
    },
    [
      cropRegion,
      getCurrentWorkflowFingerprints,
      inspection,
      isCurrentOperation,
      isCurrentProgressUpdate,
      locale,
      nextWorkflowRevision,
      optimizerGoal,
      optimizerPresetStrategy,
      optimizerSearchDepth,
      qualityFrameDropInterval,
      runtime,
    ],
  );

  const runBoundedSearch = useCallback(
    async ({
      baseFrameCount,
      editedTimelineFramesForRequest,
      fingerprint,
    }: WorkflowRequestContext): Promise<
      WorkflowResultEnvelope<OptimizerSearchResponse> | null
    > => {
      if (!inspection?.ok || inspection.isStaticImage) {
        return null;
      }

      const request = buildOptimizerSearchRequest(inspection, {
        ...buildOptimizerPlanRequest({
          inspection,
          locale,
          presetStrategy: optimizerPresetStrategy,
          optimizerGoal,
          qualityFrameDropInterval,
          searchDepth: optimizerSearchDepth,
          cropRegion,
          baseFrameCount,
          editedTimelineFramesForRequest,
        }),
        outputDirectory,
      });
      if (!request) {
        return null;
      }

      searchAbortControllerRef.current?.abort();
      const controller = new AbortController();
      searchAbortControllerRef.current = controller;
      const operationId = createMediaOperationId();

      const ticket: WorkflowRequestTicket = {
        revision: searchTicketRef.current + 1,
        fingerprint,
      };
      searchTicketRef.current = ticket.revision;
      const stateRevision = nextWorkflowRevision();
      setSearchState({
        status: "loading",
        revision: stateRevision,
        fingerprint,
        progress: null,
      });

      try {
        if (!runtime.capabilities.backendProcessing) {
          throw new Error(
            "Desktop optimization is unavailable in browser preview mode.",
          );
        }

        const result = await runtime.runOptimizerSearch(request, {
          operationId,
          signal: controller.signal,
          onProgress: (progress) => {
            if (
              !isCurrentProgressUpdate(
                "export",
                ticket,
                searchTicketRef.current,
                searchAbortControllerRef,
                controller,
              )
            ) {
              return;
            }
            setSearchState((current) =>
              current.status === "loading" &&
              current.revision === stateRevision &&
              current.fingerprint === fingerprint
                ? { ...current, progress }
                : current,
            );
          },
        });

        if (
          !isCurrentOperation(
            "export",
            ticket,
            searchTicketRef.current,
          )
        ) {
          return null;
        }

        const normalizedResult = {
          ...result,
          stopReason: normalizeOptimizerStopReason(result.stopReason),
        };
        const nextState = workflowStateFromResult(
          normalizedResult,
          stateRevision,
          fingerprint,
        );
        setSearchState(nextState);
        return nextState.status === "cancelled"
          ? null
          : { fingerprint, value: normalizedResult };
      } catch (error) {
        if (
          !isCurrentOperation(
            "export",
            ticket,
            searchTicketRef.current,
          )
        ) {
          return null;
        }

        const normalized = normalizeLegacyMediaError(error);
        setSearchState(
          normalized.errorCode === "cancelled"
            ? { status: "cancelled", revision: stateRevision, fingerprint }
            : {
                status: "error",
                revision: stateRevision,
                fingerprint,
                code: normalized.errorCode ?? "internal-task-failed",
                reasonCode: normalized.reasonCode,
                message: normalized.diagnostics ?? "",
              },
        );
        return null;
      } finally {
        if (
          isCurrentOperation(
            "export",
            ticket,
            searchTicketRef.current,
          )
        ) {
          setSearchState((current) =>
            current.status === "loading" &&
            current.revision === stateRevision &&
            current.fingerprint === fingerprint
              ? {
                  status: "cancelled",
                  revision: stateRevision,
                  fingerprint,
                }
              : current,
          );
        }
        if (searchAbortControllerRef.current === controller) {
          searchAbortControllerRef.current = null;
        }
      }
    },
    [
      cropRegion,
      inspection,
      isCurrentOperation,
      isCurrentProgressUpdate,
      locale,
      nextWorkflowRevision,
      optimizerGoal,
      optimizerPresetStrategy,
      optimizerSearchDepth,
      outputDirectory,
      qualityFrameDropInterval,
      runtime,
    ],
  );

  const cancelOptimizerSearch = useCallback(() => {
    searchAbortControllerRef.current?.abort();
  }, []);

  const convertStaticImageToPng = useCallback(
    async (
      fingerprint: string,
    ): Promise<
      WorkflowResultEnvelope<StaticImageConversionResult> | null
    > => {
      if (!inspection?.ok || !inspection.isStaticImage) {
        return null;
      }

      const request = buildStaticImageConversionRequest(inspection, {
        outputDirectory,
        locale,
        cropRegion,
      });
      if (!request) {
        return null;
      }

      conversionAbortControllerRef.current?.abort();
      const controller = new AbortController();
      conversionAbortControllerRef.current = controller;
      const operationId = createMediaOperationId();

      const ticket: WorkflowRequestTicket = {
        revision: conversionTicketRef.current + 1,
        fingerprint,
      };
      conversionTicketRef.current = ticket.revision;
      const stateRevision = nextWorkflowRevision();
      setConversionState({
        status: "loading",
        revision: stateRevision,
        fingerprint,
        progress: null,
      });

      try {
        if (!runtime.capabilities.backendProcessing) {
          throw new Error(
            "Desktop export is unavailable in browser preview mode.",
          );
        }

        const result = await runtime.convertStaticImageToPng(request, {
          operationId,
          signal: controller.signal,
          onProgress: (progress) => {
            if (
              !isCurrentProgressUpdate(
                "export",
                ticket,
                conversionTicketRef.current,
                conversionAbortControllerRef,
                controller,
              )
            ) {
              return;
            }
            setConversionState((current) =>
              current.status === "loading" &&
              current.revision === stateRevision &&
              current.fingerprint === fingerprint
                ? { ...current, progress }
                : current,
            );
          },
        });

        if (
          !isCurrentOperation(
            "export",
            ticket,
            conversionTicketRef.current,
          )
        ) {
          return null;
        }

        const nextState = workflowStateFromResult(
          result,
          stateRevision,
          fingerprint,
        );
        setConversionState(nextState);
        return nextState.status === "cancelled"
          ? null
          : { fingerprint, value: result };
      } catch (error) {
        if (
          !isCurrentOperation(
            "export",
            ticket,
            conversionTicketRef.current,
          )
        ) {
          return null;
        }

        const normalized = normalizeLegacyMediaError(error);
        setConversionState(
          normalized.errorCode === "cancelled"
            ? { status: "cancelled", revision: stateRevision, fingerprint }
            : {
                status: "error",
                revision: stateRevision,
                fingerprint,
                code: normalized.errorCode ?? "internal-task-failed",
                reasonCode: normalized.reasonCode,
                message: normalized.diagnostics ?? "",
              },
        );
        return null;
      } finally {
        if (
          isCurrentOperation(
            "export",
            ticket,
            conversionTicketRef.current,
          )
        ) {
          setConversionState((current) =>
            current.status === "loading" &&
            current.revision === stateRevision &&
            current.fingerprint === fingerprint
              ? {
                  status: "cancelled",
                  revision: stateRevision,
                  fingerprint,
                }
              : current,
          );
        }
        if (conversionAbortControllerRef.current === controller) {
          conversionAbortControllerRef.current = null;
        }
      }
    },
    [
      cropRegion,
      inspection,
      isCurrentOperation,
      isCurrentProgressUpdate,
      locale,
      nextWorkflowRevision,
      outputDirectory,
      runtime,
    ],
  );

  return {
    runtime,
    toolReport,
    toolError,
    inspection,
    planState,
    searchState,
    conversionState,
    outputDirectory,
    optimizerPresetStrategy,
    optimizerGoal,
    qualityFrameDropInterval,
    optimizerSearchDepth,
    cropRegion,
    cropAspectRatioPreset,
    inspectionLoading,
    isDragging,
    setOutputDirectory,
    setOptimizerPresetStrategy,
    setOptimizerGoal,
    setQualityFrameDropInterval,
    setOptimizerSearchDepth,
    setCropRegion,
    setCropAspectRatioPreset,
    pickInputFile,
    pickOutputDirectory,
    openOutputFolder,
    buildPlan,
    runBoundedSearch,
    cancelOptimizerSearch,
    convertStaticImageToPng,
    invalidateWorkflowResults,
  };
}
