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
import type {
  MediaInspection,
  OptimizerGoal,
  OptimizerPlanRequest,
  OptimizerPlanResponse,
  OptimizerPresetStrategy,
  OptimizerSearchDepth,
  OptimizerSearchRequest,
  OptimizerSearchResponse,
  StaticImageConversionRequest,
  StaticImageConversionResult,
  TimelineFrameRequest,
} from "../types/workflow";
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
  advancedPreviewCount: number;
  onCommitEditorSession: () => void;
  getCurrentWorkflowFingerprints: () => WorkflowFingerprints;
};

type WorkflowRequestContext = {
  baseFrameCount: number;
  editedTimelineFramesForRequest: TimelineFrameRequest[] | undefined;
  fingerprint: string;
};

type OptimizerBaseRequestContext = Omit<WorkflowRequestContext, "fingerprint"> & {
  inspection: MediaInspection;
  locale: Locale;
  presetStrategy: OptimizerPresetStrategy;
  optimizerGoal: OptimizerGoal;
  qualityFrameDropInterval: number;
  searchDepth: OptimizerSearchDepth;
  cropRegion: CropRegion;
};

type WorkflowFingerprintKind = "planner" | "export";

function buildOptimizerBaseRequest({
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
  advancedPreviewCount,
  onCommitEditorSession,
  getCurrentWorkflowFingerprints,
}: UseMediaWorkflowControllerParams) {
  const runtime = getAppRuntime();
  const [planState, setPlanState] = useState<
    VersionedWorkflowState<OptimizerPlanResponse>
  >({
    status: "idle",
    revision: 0,
    fingerprint: "",
  });
  const [searchState, setSearchState] = useState<
    VersionedWorkflowState<OptimizerSearchResponse>
  >({
    status: "idle",
    revision: 0,
    fingerprint: "",
  });
  const [conversionState, setConversionState] = useState<
    VersionedWorkflowState<StaticImageConversionResult>
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
        planTicketRef.current += 1;
        setPlanState({
          status: "idle",
          revision: nextWorkflowRevision(),
          fingerprint: fingerprints.planner,
        });
      }

      if (force || previous?.export !== fingerprints.export) {
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

        const request = buildOptimizerBaseRequest({
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
        const result = await runtime.buildOptimizerPlan(request);
        const trimmedResult = {
          ...result,
          candidates: result.candidates.slice(0, advancedPreviewCount),
        };

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
          trimmedResult,
          stateRevision,
          fingerprint,
        );
        setPlanState(nextState);
        return nextState.status === "cancelled"
          ? null
          : { fingerprint, value: trimmedResult };
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
      }
    },
    [
      advancedPreviewCount,
      cropRegion,
      getCurrentWorkflowFingerprints,
      inspection,
      isCurrentOperation,
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
        if (
          !runtime.capabilities.backendProcessing ||
          !inspection.backendInputPath
        ) {
          throw new Error(
            "Desktop optimization is unavailable in browser preview mode.",
          );
        }

        const request = {
          ...buildOptimizerBaseRequest({
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
          inputPath: inspection.backendInputPath,
          outputDirectory,
        } satisfies OptimizerSearchRequest;
        const result = await runtime.runOptimizerSearch(request);

        if (
          !isCurrentOperation(
            "export",
            ticket,
            searchTicketRef.current,
          )
        ) {
          return null;
        }

        const nextState = workflowStateFromResult(
          result,
          stateRevision,
          fingerprint,
        );
        setSearchState(nextState);
        return nextState.status === "cancelled"
          ? null
          : { fingerprint, value: result };
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
      }
    },
    [
      cropRegion,
      inspection,
      isCurrentOperation,
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

  const convertStaticImageToPng = useCallback(
    async (
      fingerprint: string,
    ): Promise<
      WorkflowResultEnvelope<StaticImageConversionResult> | null
    > => {
      if (!inspection?.ok || !inspection.isStaticImage) {
        return null;
      }

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
        if (
          !runtime.capabilities.backendProcessing ||
          !inspection.backendInputPath
        ) {
          throw new Error(
            "Desktop export is unavailable in browser preview mode.",
          );
        }

        const result = await runtime.convertStaticImageToPng({
          inputPath: inspection.backendInputPath,
          outputDirectory,
          locale,
          cropRegion,
        } satisfies StaticImageConversionRequest);

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
      }
    },
    [
      cropRegion,
      inspection,
      isCurrentOperation,
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
    convertStaticImageToPng,
    invalidateWorkflowResults,
  };
}
