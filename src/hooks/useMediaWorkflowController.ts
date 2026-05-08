import { useCallback, useState } from "react";

import {
  FULL_CROP_REGION,
  type CropAspectRatioPreset,
  type CropRegion,
} from "../components/MediaSelectionPreview";
import { getAppRuntime } from "../platform/runtime";
import { useMediaInputSelector } from "./mediaWorkflow/useMediaInputSelector";
import { useToolHealthReport } from "./mediaWorkflow/useToolHealthReport";
import type { Locale } from "../locales/messages";
import type {
  FitMode,
  MediaInspection,
  OptimizerGoal,
  OptimizerPlanRequest,
  OptimizerPresetStrategy,
  OptimizerPlanResponse,
  OptimizerSearchDepth,
  OptimizerSearchRequest,
  OptimizerSearchResponse,
  StaticImageConversionRequest,
  StaticImageConversionResult,
  TimelineFrameRequest,
} from "../types/workflow";

type UseMediaWorkflowControllerParams = {
  locale: Locale;
  initialLocale: Locale;
  advancedPreviewCount: number;
  onCommitEditorSession: () => void;
};

type WorkflowRequestContext = {
  baseFrameCount: number;
  editedTimelineFramesForRequest: TimelineFrameRequest[] | undefined;
};

type OptimizerBaseRequestContext = WorkflowRequestContext & {
  inspection: MediaInspection;
  locale: Locale;
  fitMode: FitMode;
  presetStrategy: OptimizerPresetStrategy;
  optimizerGoal: OptimizerGoal;
  qualityFrameDropInterval: number;
  searchDepth: OptimizerSearchDepth;
  cropRegion: CropRegion;
};

function buildOptimizerBaseRequest({
  inspection,
  locale,
  fitMode,
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
    fitMode,
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
}: UseMediaWorkflowControllerParams) {
  const runtime = getAppRuntime();
  const [plan, setPlan] = useState<OptimizerPlanResponse | null>(null);
  const [searchResult, setSearchResult] = useState<OptimizerSearchResponse | null>(null);
  const [conversionResult, setConversionResult] =
    useState<StaticImageConversionResult | null>(null);
  const [fitMode, setFitMode] = useState<FitMode>("contain");
  const [optimizerPresetStrategy, setOptimizerPresetStrategy] =
    useState<OptimizerPresetStrategy>("auto");
  const [optimizerGoal, setOptimizerGoal] = useState<OptimizerGoal>("balanced");
  const [qualityFrameDropInterval, setQualityFrameDropInterval] = useState(3);
  const [optimizerSearchDepth, setOptimizerSearchDepth] =
    useState<OptimizerSearchDepth>("standard");
  const [cropRegion, setCropRegion] = useState<CropRegion>(FULL_CROP_REGION);
  const [cropAspectRatioPreset, setCropAspectRatioPreset] =
    useState<CropAspectRatioPreset>("free");
  const [planLoading, setPlanLoading] = useState(false);
  const [searchLoading, setSearchLoading] = useState(false);
  const [conversionLoading, setConversionLoading] = useState(false);
  const [plannerError, setPlannerError] = useState<string | null>(null);

  const resetWorkflowDerivedState = useCallback(() => {
    setPlan(null);
    setSearchResult(null);
    setConversionResult(null);
    setPlannerError(null);
    setCropRegion(FULL_CROP_REGION);
    setCropAspectRatioPreset("free");
  }, []);

  const resetForNewInspection = useCallback(() => {
    resetWorkflowDerivedState();
  }, [resetWorkflowDerivedState]);
  const { toolReport, toolError } = useToolHealthReport(runtime, initialLocale);
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

  const buildPlan = useCallback(async ({ baseFrameCount, editedTimelineFramesForRequest }: WorkflowRequestContext) => {
    if (!inspection?.ok || inspection.isStaticImage) {
      return null;
    }

    setPlanLoading(true);
    setPlannerError(null);
    setSearchResult(null);

    try {
      if (!runtime.capabilities.backendProcessing) {
        throw new Error("Desktop optimization is unavailable in browser preview mode.");
      }

      const request = buildOptimizerBaseRequest({
        inspection,
        locale,
        fitMode,
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
      setPlan(trimmedResult);
      if (!result.ok && result.errorMessage) {
        setPlannerError(result.errorMessage);
      }
      return trimmedResult;
    } catch (error) {
      setPlannerError(error instanceof Error ? error.message : String(error));
      return null;
    } finally {
      setPlanLoading(false);
    }
  }, [
    advancedPreviewCount,
    cropRegion,
    fitMode,
    inspection,
    locale,
    optimizerPresetStrategy,
    optimizerGoal,
    qualityFrameDropInterval,
    optimizerSearchDepth,
    runtime,
  ]);

  const runBoundedSearch = useCallback(async ({ baseFrameCount, editedTimelineFramesForRequest }: WorkflowRequestContext) => {
    if (!inspection?.ok || inspection.isStaticImage) {
      return null;
    }

    setSearchLoading(true);
    setPlannerError(null);

    try {
      if (!runtime.capabilities.backendProcessing || !inspection.backendInputPath) {
        throw new Error("Desktop optimization is unavailable in browser preview mode.");
      }

      const request = {
        ...buildOptimizerBaseRequest({
          inspection,
          locale,
          fitMode,
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
      setSearchResult(result);
      return result;
    } catch (error) {
      setPlannerError(error instanceof Error ? error.message : String(error));
      return null;
    } finally {
      setSearchLoading(false);
    }
  }, [
    cropRegion,
    fitMode,
    inspection,
    locale,
    optimizerPresetStrategy,
    optimizerGoal,
    qualityFrameDropInterval,
    optimizerSearchDepth,
    outputDirectory,
    runtime,
  ]);

  const convertStaticImageToPng = useCallback(async () => {
    if (!inspection?.ok || !inspection.isStaticImage) {
      return;
    }

    setConversionLoading(true);
    setPlannerError(null);

    try {
      if (!runtime.capabilities.backendProcessing || !inspection.backendInputPath) {
        throw new Error("Desktop export is unavailable in browser preview mode.");
      }

      const result = await runtime.convertStaticImageToPng({
        inputPath: inspection.backendInputPath,
        outputDirectory,
        locale,
        cropRegion,
      } satisfies StaticImageConversionRequest);
      setConversionResult(result);
    } catch (error) {
      setPlannerError(error instanceof Error ? error.message : String(error));
    } finally {
      setConversionLoading(false);
    }
  }, [cropRegion, inspection, locale, outputDirectory, runtime]);

  return {
    runtime,
    toolReport,
    toolError,
    inspection,
    plan,
    searchResult,
    conversionResult,
    outputDirectory,
    fitMode,
    optimizerPresetStrategy,
    optimizerGoal,
    qualityFrameDropInterval,
    optimizerSearchDepth,
    cropRegion,
    cropAspectRatioPreset,
    inspectionLoading,
    planLoading,
    searchLoading,
    conversionLoading,
    plannerError,
    isDragging,
    setOutputDirectory,
    setFitMode,
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
  };
}
