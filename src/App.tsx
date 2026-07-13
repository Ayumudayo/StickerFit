import {
  type CSSProperties,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { FolderOpenIcon } from "./components/AppIcons";
import { DesktopHeader } from "./components/editor/DesktopHeader";
import { type EditorDockPanelMode } from "./components/editor/EditorOverlayPanel";
import { OutputSizeEstimateCard } from "./components/editor/OutputSizeEstimateCard";
import { FrameEditingOverlays } from "./components/editor/FrameEditingOverlays";
import { EditorWorkspace } from "./components/editor/EditorWorkspace";
import { PickerGrid } from "./components/editor/PickerGrid";
import { InspectionErrorCard } from "./components/InspectionErrorCard";
import {
  clampPreviewZoomScale,
  constrainCropRegionToAspectRatio,
  cropRegionsMatch,
  cropAspectRatioValue,
  FULL_CROP_REGION,
  selectionSummary,
  type CropAspectRatioPreset,
  type PreviewZoomMode,
} from "./components/MediaSelectionPreview";
import { useFrameEditorController } from "./hooks/useFrameEditorController";
import {
  buildEditedTimelineFramesForRequest,
  useEditorWorkflowBridge,
} from "./hooks/useEditorWorkflowBridge";
import {
  buildOptimizerPlanRequest,
  useMediaWorkflowController,
} from "./hooks/useMediaWorkflowController";
import { useOutputSizeEstimate } from "./hooks/useOutputSizeEstimate";
import {
  buildWorkflowFingerprints,
  currentWorkflowState,
  latestActiveWorkflowState,
  type WorkflowFingerprints,
} from "./hooks/mediaWorkflow/workflowFingerprint";
import { buildFramePreviewsRequest } from "./hooks/mediaWorkflow/mediaRequestBuilders";
import { usePlaybackTimelineController } from "./hooks/usePlaybackTimelineController";
import { createMediaOperationId } from "./platform/mediaOperationId";
import { editorText } from "./locales/editorText";
import {
  detectLocale,
  mediaOperationMessage,
  MESSAGES,
  type Locale,
} from "./locales/messages";
import {
  formatTimelineTime,
  microsecondsToSeconds,
} from "./utils/timelineFrames";
import { shouldHandleEditorShortcut } from "./utils/keyboardShortcuts";
import {
  applyFramePreviewBatchResult,
  chunkFramePreviewIds,
  createPreviewBatchScheduler,
  filterFramePreviewDemand,
  markFramePreviewBatchLoading,
  prioritizeFramePreviewIds,
  retryFramePreviewEntry,
  type PreviewBatchDescriptor,
  type PreviewEntryMap,
} from "./utils/previewBatches";
import type { FramePreviewsResult } from "./types/workflow";

const ADVANCED_PREVIEW_COUNT = 6;
const ADVANCED_SETTINGS_PANEL_ID = "advanced-optimizer-settings";
const EDITOR_RESULTS_PANEL_ID = "editor-results-panel";
const EDITOR_PREVIEW_PANEL_ID = "editor-preview-panel";
const EMPTY_WORKFLOW_FINGERPRINTS: WorkflowFingerprints = {
  encoding: "",
  planner: "",
  export: "",
};

const MIN_DURATION_US = 10_000;
const FRAME_PREVIEW_LOOKAHEAD = 8;
const EMPTY_FRAME_PREVIEW_ENTRIES: PreviewEntryMap = new Map();
const EDITOR_INTERACTIVE_SELECTOR = [
  "[data-editor-interactive]",
  "input",
  "select",
  "textarea",
  "button",
  "a",
  "[contenteditable]:not([contenteditable='false'])",
  "[role='dialog']",
  "[role='slider']",
  "[role='listbox']",
  "[role='option']",
  "[role='menu']",
  "[role='menuitem']",
  "[tabindex]:not([data-editor-shortcut-surface])",
].join(",");

type FramePreviewRunResult = {
  requestId: number;
  result: FramePreviewsResult | null;
};

function filterPathLabel(value: string | null, fallback: string) {
  return value?.trim() ? value : fallback;
}

function isSpaceShortcutKey(event: KeyboardEvent) {
  return (
    event.key === " " || event.key === "Spacebar" || event.code === "Space"
  );
}

export default function App() {
  const [locale, setLocale] = useState<Locale>(detectLocale());
  const [previewDuration, setPreviewDuration] = useState<number | null>(null);
  const [previewZoomMode, setPreviewZoomMode] =
    useState<PreviewZoomMode>("fit");
  const [manualPreviewZoomScale, setManualPreviewZoomScale] = useState(1);
  const [resolvedPreviewZoomScale, setResolvedPreviewZoomScale] = useState(1);
  const [resolvedFitPreviewZoomScale, setResolvedFitPreviewZoomScale] =
    useState(1);
  const [editorSessionKey, setEditorSessionKey] = useState(0);
  const [editorWorkspaceMinHeight, setEditorWorkspaceMinHeight] = useState<
    number | null
  >(null);
  const [activeDockPanel, setActiveDockPanel] =
    useState<EditorDockPanelMode | null>(null);
  const [framePreviewEntries, setFramePreviewEntries] =
    useState<PreviewEntryMap>(() => new Map());
  const [framePreviewEntriesFingerprint, setFramePreviewEntriesFingerprint] =
    useState("");
  const [framePreviewVisibleRange, setFramePreviewVisibleRange] = useState({
    start: 0,
    end: 24,
  });
  const initialLocaleRef = useRef(locale);
  const timelineRailRef = useRef<HTMLDivElement | null>(null);
  const shellRef = useRef<HTMLElement | null>(null);
  const editorWorkspaceRef = useRef<HTMLElement | null>(null);
  const framePreviewEntriesRef = useRef<PreviewEntryMap>(new Map());
  const framePreviewSchedulerRef = useRef<ReturnType<
    typeof createPreviewBatchScheduler<
      PreviewBatchDescriptor,
      FramePreviewRunResult
    >
  > | null>(null);
  const framePreviewFingerprintRef = useRef("");
  const framePreviewRequestIdRef = useRef(0);
  const workflowFingerprintRef = useRef<WorkflowFingerprints>(
    EMPTY_WORKFLOW_FINGERPRINTS,
  );
  const invalidatedWorkflowFingerprintRef = useRef<WorkflowFingerprints | null>(
    null,
  );
  const getCurrentWorkflowFingerprints = useCallback(
    () => workflowFingerprintRef.current,
    [],
  );
  const updateFramePreviewEntries = useCallback(
    (update: (current: PreviewEntryMap) => PreviewEntryMap) => {
      const next = update(framePreviewEntriesRef.current);
      framePreviewEntriesRef.current = next;
      setFramePreviewEntries(next);
    },
    [],
  );
  const handleCommitEditorSession = useCallback(() => {
    setEditorSessionKey((current) => current + 1);
  }, []);

  const mediaWorkflow = useMediaWorkflowController({
    locale,
    initialLocale: initialLocaleRef.current,
    onCommitEditorSession: handleCommitEditorSession,
    getCurrentWorkflowFingerprints,
  });
  const {
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
  } = mediaWorkflow;

  const copy = MESSAGES[locale];
  const ui = useMemo(() => editorText(locale), [locale]);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  const {
    previewKind,
    sourceDuration,
    sourceFrames,
    quickResolution,
    quickFps,
  } = useEditorWorkflowBridge({
    inspection,
    previewDuration,
  });
  const frameEditor = useFrameEditorController({
    editorSessionKey,
    sourceFrames,
    minDurationUs: MIN_DURATION_US,
  });
  const {
    timelineFrames,
    timelineFrameViews,
    selectedInstanceIds,
    selectionModel,
    hasClipboardFrames,
    hasSingleFrameSelection,
    hasMixedSelectedDurations,
    canDeleteUnselectedFrames,
    frameContextMenu,
    frameDropTarget,
    frameReorderState,
    frameDurationDialog,
    frameDurationMode,
    frameDurationSecondsValue,
    frameDurationFpsValue,
    nthSelectionStep,
    showNthSelectionDialog,
    frameTableBodyRef,
    frameContextMenuRef,
    frameDurationDialogRef,
    nthSelectionDialogRef,
    setFrameDurationMode,
    setNthSelectionStep,
    handleFramePointerDown,
    handleFrameKeyDown,
    handleFrameContextMenu,
    closeFrameContextMenu,
    selectSingleFrame,
    selectAdjacentFrame,
    selectAllFrames,
    clearAllFrames,
    openFrameDurationDialog,
    splitCurrentFrame,
    speedAdjustSelectedFrames,
    copySelectedFramesTo,
    moveSelectedFramesTo,
    moveSelectedFrames,
    reverseSelectedFrames,
    deleteSelectedFrames,
    deleteUnselectedFrames,
    selectOddFrames,
    selectEvenFrames,
    openNthSelectionDialog,
    invertFrameSelection,
    renumberTimelineFrames,
    copyFramesToClipboard,
    cutFramesToClipboard,
    pasteClipboard,
    pasteClipboardAtSelection,
    applyDurationChange,
    updateFrameDurationFromSeconds,
    updateFrameDurationFromFps,
    applyNthFrameSelection,
    closeFrameDurationDialog,
    closeNthSelectionDialog,
  } = frameEditor;
  const editedTimelineFramesForRequest = useMemo(
    () => buildEditedTimelineFramesForRequest(timelineFrames, sourceFrames),
    [sourceFrames, timelineFrames],
  );
  const workflowFingerprints = useMemo(
    () =>
      buildWorkflowFingerprints({
        editorSessionKey,
        locale,
        inputPath: inspection?.inputPath ?? null,
        sourceRevision: inspection?.sourceRevision ?? null,
        outputDirectory,
        sourceDurationSeconds: inspection?.durationSeconds ?? null,
        inputWidth: inspection?.width ?? null,
        inputHeight: inspection?.height ?? null,
        avgFps: inspection?.avgFps ?? null,
        optimizerPresetStrategy,
        optimizerGoal,
        qualityFrameDropInterval,
        optimizerSearchDepth,
        cropRegion,
        baseFrameCount: sourceFrames.length,
        timelineFrames: editedTimelineFramesForRequest,
      }),
    [
      cropRegion,
      editedTimelineFramesForRequest,
      editorSessionKey,
      inspection?.avgFps,
      inspection?.durationSeconds,
      inspection?.height,
      inspection?.inputPath,
      inspection?.sourceRevision,
      inspection?.width,
      locale,
      optimizerGoal,
      optimizerPresetStrategy,
      optimizerSearchDepth,
      outputDirectory,
      qualityFrameDropInterval,
      sourceFrames.length,
    ],
  );
  workflowFingerprintRef.current = workflowFingerprints;

  const currentPlanState = currentWorkflowState(
    planState,
    workflowFingerprints.planner,
  );
  const currentSearchState = currentWorkflowState(
    searchState,
    workflowFingerprints.export,
  );
  const currentConversionState = currentWorkflowState(
    conversionState,
    workflowFingerprints.export,
  );
  const fullPlan =
    currentPlanState?.status === "ready" ? currentPlanState.value : null;
  const plan = useMemo(
    () =>
      fullPlan
        ? {
            ...fullPlan,
            candidates: fullPlan.candidates.slice(0, ADVANCED_PREVIEW_COUNT),
          }
        : null,
    [fullPlan],
  );
  const optimizerPlanRequest = useMemo(
    () =>
      inspection?.ok && !inspection.isStaticImage
        ? buildOptimizerPlanRequest({
            inspection,
            locale,
            presetStrategy: optimizerPresetStrategy,
            optimizerGoal,
            qualityFrameDropInterval,
            searchDepth: optimizerSearchDepth,
            cropRegion,
            baseFrameCount: sourceFrames.length,
            editedTimelineFramesForRequest,
          })
        : null,
    [
      cropRegion,
      editedTimelineFramesForRequest,
      inspection,
      locale,
      optimizerGoal,
      optimizerPresetStrategy,
      optimizerSearchDepth,
      qualityFrameDropInterval,
      sourceFrames.length,
    ],
  );
  const outputSizeEstimate = useOutputSizeEstimate({
    runtime,
    inspection,
    plan: fullPlan,
    optimizerPlanRequest,
    encodingFingerprint: workflowFingerprints.encoding,
    cropRegion,
    locale,
  });
  const currentEstimateState =
    outputSizeEstimate.state.estimate.fingerprint ===
    workflowFingerprints.encoding
      ? outputSizeEstimate.state.estimate
      : {
          status: "idle" as const,
          revision: outputSizeEstimate.state.estimate.revision,
          fingerprint: workflowFingerprints.encoding,
        };
  const currentProbeState =
    outputSizeEstimate.state.probe.fingerprint === workflowFingerprints.encoding
      ? outputSizeEstimate.state.probe
      : {
          status: "idle" as const,
          revision: outputSizeEstimate.state.probe.revision,
          fingerprint: workflowFingerprints.encoding,
        };
  const recommendedCandidateId =
    fullPlan?.candidates.find((candidate) => candidate.rank === 1)?.id ??
    outputSizeEstimate.estimates.find(
      (estimate) => estimate.candidateId !== null,
    )?.candidateId ??
    null;
  const primarySizeEstimate = inspection?.isStaticImage
    ? (outputSizeEstimate.estimates[0] ?? null)
    : recommendedCandidateId
      ? (outputSizeEstimate.estimateByCandidateId.get(recommendedCandidateId) ??
        null)
      : null;
  const searchResult =
    currentSearchState?.status === "ready" ? currentSearchState.value : null;
  const conversionResult =
    currentConversionState?.status === "ready"
      ? currentConversionState.value
      : null;
  const planLoading = currentPlanState?.status === "loading";
  const searchLoading = currentSearchState?.status === "loading";
  const conversionLoading = currentConversionState?.status === "loading";
  const primaryOperationState = inspection?.isStaticImage
    ? currentConversionState
    : currentSearchState;
  const primaryOperationProgress =
    primaryOperationState?.status === "loading"
      ? primaryOperationState.progress
      : null;
  const primaryOperationCancelled =
    primaryOperationState?.status === "cancelled";
  const latestWorkflowState = latestActiveWorkflowState([
    currentPlanState,
    currentSearchState,
    currentConversionState,
  ]);
  const plannerError =
    latestWorkflowState?.status === "error"
      ? mediaOperationMessage(
          locale,
          latestWorkflowState.code,
          latestWorkflowState.reasonCode,
        )
      : null;

  useEffect(() => {
    const previous = invalidatedWorkflowFingerprintRef.current;
    invalidateWorkflowResults(workflowFingerprints);

    if (previous) {
      setActiveDockPanel((current) => {
        if (
          current === "preview" &&
          previous.planner !== workflowFingerprints.planner
        ) {
          return null;
        }
        if (
          current === "results" &&
          previous.export !== workflowFingerprints.export
        ) {
          return null;
        }
        return current;
      });
    }

    invalidatedWorkflowFingerprintRef.current = workflowFingerprints;
  }, [invalidateWorkflowResults, workflowFingerprints]);

  const playback = usePlaybackTimelineController({
    editorSessionKey,
    inspection,
    setPreviewDuration,
    sourceDuration,
    timelineFrameViews,
    selectedInstanceIds,
    timelineRailRef,
  });
  const {
    currentTime,
    isPlaying,
    totalDuration,
    timelineRailStyle,
    previewCurrentTime,
    currentFrameInstanceId,
    scrubTo,
    handlePreviewDurationChange,
    togglePlayback,
    handleTimelinePointerDown,
    handleTimelinePointerMove,
    handleTimelinePointerEnd,
  } = playback;
  const currentTimelineTimeUs = Math.max(
    0,
    Math.round(currentTime * 1_000_000),
  );
  const totalTimelineDurationUs = Math.max(
    0,
    Math.round(totalDuration * 1_000_000),
  );
  const currentPreviewFrame = useMemo(
    () =>
      timelineFrameViews.find(
        (frame) => frame.instanceId === currentFrameInstanceId,
      ) ?? null,
    [currentFrameInstanceId, timelineFrameViews],
  );
  const activeFrameInstanceId =
    selectedInstanceIds[selectedInstanceIds.length - 1] ?? null;

  const isWebPreviewMode = runtime.kind === "web";
  const isEditorLayoutActive = inspection?.ok === true;
  const supportsDesktopProcessing = runtime.capabilities.backendProcessing;
  const supportsOutputDirectoryActions =
    runtime.capabilities.outputDirectorySelection &&
    runtime.capabilities.openOutputFolder;
  const healthLabel = isWebPreviewMode
    ? copy.webPreviewMode
    : toolReport?.ready
      ? copy.toolReady
      : copy.toolUnavailable;
  const isToolReady = isWebPreviewMode || toolReport?.ready === true;
  const supportsRailFramePreviews =
    inspection?.ok === true &&
    !inspection.isStaticImage &&
    Boolean(inspection.backendInputPath) &&
    Boolean(inspection.sourceRevision) &&
    /\.(gif|apng|png|mp4|m4v|mov|webm)$/i.test(
      inspection.backendInputPath ?? "",
    );
  const requiresBackendFramePreview =
    supportsRailFramePreviews &&
    previewKind === "image" &&
    /\.(gif|apng|png)$/i.test(inspection?.backendInputPath ?? "");
  const framePreviewFingerprint = useMemo(
    () =>
      supportsRailFramePreviews
        ? JSON.stringify([
            inspection?.backendInputPath,
            inspection?.sourceRevision,
            inspection?.width,
            inspection?.height,
            locale,
          ])
        : "",
    [
      inspection?.backendInputPath,
      inspection?.height,
      inspection?.sourceRevision,
      inspection?.width,
      locale,
      supportsRailFramePreviews,
    ],
  );
  const currentFramePreviewEntries =
    framePreviewEntriesFingerprint === framePreviewFingerprint
      ? framePreviewEntries
      : EMPTY_FRAME_PREVIEW_ENTRIES;
  const visibleFramePreviewIds = useMemo(
    () =>
      timelineFrameViews
        .slice(framePreviewVisibleRange.start, framePreviewVisibleRange.end)
        .map((frame) => frame.sourceFrameId),
    [
      framePreviewVisibleRange.end,
      framePreviewVisibleRange.start,
      timelineFrameViews,
    ],
  );
  const lookaheadFramePreviewIds = useMemo(() => {
    const currentIndex = currentFrameInstanceId
      ? timelineFrameViews.findIndex(
          (frame) => frame.instanceId === currentFrameInstanceId,
        )
      : -1;
    return currentIndex < 0
      ? []
      : timelineFrameViews
          .slice(currentIndex + 1, currentIndex + 1 + FRAME_PREVIEW_LOOKAHEAD)
          .map((frame) => frame.sourceFrameId);
  }, [currentFrameInstanceId, timelineFrameViews]);
  const desiredFramePreviewIds = useMemo(
    () =>
      prioritizeFramePreviewIds({
        currentSourceFrameId: currentPreviewFrame?.sourceFrameId ?? null,
        lookaheadSourceFrameIds: lookaheadFramePreviewIds,
        visibleSourceFrameIds: visibleFramePreviewIds,
      }),
    [
      currentPreviewFrame?.sourceFrameId,
      lookaheadFramePreviewIds,
      visibleFramePreviewIds,
    ],
  );
  const handleFramePreviewVisibleRange = useCallback(
    (start: number, end: number) => {
      setFramePreviewVisibleRange((current) =>
        current.start === start && current.end === end
          ? current
          : { start, end },
      );
    },
    [],
  );
  const handleRetryFramePreview = useCallback(
    (sourceFrameId: number) => {
      updateFramePreviewEntries((current) =>
        retryFramePreviewEntry(current, sourceFrameId),
      );
    },
    [updateFramePreviewEntries],
  );

  useEffect(() => {
    framePreviewSchedulerRef.current?.reset(framePreviewFingerprint);
    framePreviewSchedulerRef.current = null;
    framePreviewFingerprintRef.current = framePreviewFingerprint;
    framePreviewEntriesRef.current = new Map();
    setFramePreviewEntries(new Map());
    setFramePreviewEntriesFingerprint(framePreviewFingerprint);

    if (!supportsRailFramePreviews || !framePreviewFingerprint) return;

    const scheduler = createPreviewBatchScheduler<
      PreviewBatchDescriptor,
      FramePreviewRunResult
    >({
      prepareBatch: (value) => {
        const prioritized = prioritizeFramePreviewIds({
          currentSourceFrameId: value.currentSourceFrameId,
          lookaheadSourceFrameIds: value.sourceFrameIds,
          visibleSourceFrameIds: [],
        });
        const available = filterFramePreviewDemand(
          prioritized,
          framePreviewEntriesRef.current,
        );
        const sourceFrameIds = chunkFramePreviewIds(available)[0] ?? [];
        return sourceFrameIds.length > 0 ? { ...value, sourceFrameIds } : null;
      },
      runBatch: (value, schedulerSignal) => {
        const request = buildFramePreviewsRequest(inspection, {
          sourceFrameIds: value.sourceFrameIds,
          sourceWidth: inspection?.width ?? null,
          sourceHeight: inspection?.height ?? null,
          locale,
        });
        const requestId = framePreviewRequestIdRef.current + 1;
        framePreviewRequestIdRef.current = requestId;
        const controller = new AbortController();
        const operationId = createMediaOperationId();
        updateFramePreviewEntries((current) =>
          markFramePreviewBatchLoading(
            current,
            value.sourceFrameIds,
            operationId,
          ),
        );

        if (!request) return Promise.resolve({ requestId, result: null });

        return new Promise<FramePreviewRunResult>((resolve) => {
          let settled = false;
          const finish = (result: FramePreviewsResult | null) => {
            if (settled) return;
            settled = true;
            schedulerSignal.removeEventListener("abort", abortRequest);
            resolve({ requestId, result });
          };
          const timeoutId = window.setTimeout(() => {
            void runtime
              .extractFramePreviews(request, {
                operationId,
                signal: controller.signal,
              })
              .then((result) => finish(result))
              .catch(() => finish(null));
          }, 0);
          function abortRequest() {
            window.clearTimeout(timeoutId);
            controller.abort();
            finish(null);
          }
          schedulerSignal.addEventListener("abort", abortRequest, {
            once: true,
          });
          if (schedulerSignal.aborted) abortRequest();
        });
      },
      commitBatch: (value, completed) => {
        if (
          framePreviewFingerprintRef.current !== value.fingerprint ||
          framePreviewRequestIdRef.current !== completed.requestId
        ) {
          return;
        }
        const fallbackMessage =
          locale === "ko"
            ? "프레임 미리보기를 불러오지 못했습니다."
            : "Unable to load the frame preview.";
        updateFramePreviewEntries((current) =>
          applyFramePreviewBatchResult(
            current,
            value.sourceFrameIds,
            completed.result,
            fallbackMessage,
          ),
        );
      },
    });
    framePreviewSchedulerRef.current = scheduler;

    return () => {
      scheduler.reset("");
      if (framePreviewSchedulerRef.current === scheduler) {
        framePreviewSchedulerRef.current = null;
        framePreviewFingerprintRef.current = "";
        framePreviewEntriesRef.current = new Map();
      }
    };
  }, [
    framePreviewFingerprint,
    inspection,
    locale,
    runtime,
    supportsRailFramePreviews,
    updateFramePreviewEntries,
  ]);

  useEffect(() => {
    const scheduler = framePreviewSchedulerRef.current;
    if (
      !scheduler ||
      !framePreviewFingerprint ||
      desiredFramePreviewIds.length === 0
    )
      return;
    const next: PreviewBatchDescriptor = {
      fingerprint: framePreviewFingerprint,
      currentSourceFrameId: currentPreviewFrame?.sourceFrameId ?? null,
      sourceFrameIds: desiredFramePreviewIds,
    };
    scheduler.schedule(next);
  }, [
    currentPreviewFrame?.sourceFrameId,
    desiredFramePreviewIds,
    framePreviewEntries,
    framePreviewFingerprint,
  ]);

  const backendInputPath = inspection?.backendInputPath;
  const sourceRevision = inspection?.sourceRevision;
  const framePreviewSrc = useMemo(() => {
    if (
      !requiresBackendFramePreview ||
      !backendInputPath ||
      !sourceRevision ||
      !currentPreviewFrame
    ) {
      return null;
    }
    const entry = currentFramePreviewEntries.get(
      currentPreviewFrame.sourceFrameId,
    );
    return entry?.status === "ready" ? entry.dataUrl : null;
  }, [
    backendInputPath,
    currentPreviewFrame,
    currentFramePreviewEntries,
    requiresBackendFramePreview,
    sourceRevision,
  ]);

  useEffect(() => {
    function handleGlobalKeyboardShortcuts(event: KeyboardEvent) {
      const target = event.target instanceof Element ? event.target : null;
      const insideEditorSurface = Boolean(
        target?.closest("[data-editor-shortcut-surface]"),
      );
      const insideInteractiveSurface = Boolean(
        target?.closest(EDITOR_INTERACTIVE_SELECTOR),
      );
      if (
        !inspection?.ok ||
        inspection.isStaticImage ||
        !shouldHandleEditorShortcut({
          defaultPrevented: event.defaultPrevented,
          isComposing: event.isComposing,
          hasModifier: event.altKey || event.ctrlKey || event.metaKey,
          dialogOpen:
            activeDockPanel !== null ||
            frameDurationDialog !== null ||
            showNthSelectionDialog,
          insideEditorSurface,
          insideInteractiveSurface,
        })
      ) {
        return;
      }

      if (isSpaceShortcutKey(event)) {
        event.preventDefault();
        event.stopPropagation();
        if (!event.repeat) {
          togglePlayback();
        }
        return;
      }

      if (event.key === "Delete") {
        if (selectedInstanceIds.length > 0) {
          event.preventDefault();
          event.stopPropagation();
          deleteSelectedFrames();
        }
        return;
      }

      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") {
        return;
      }

      const selectedAnchorInstanceId =
        selectedInstanceIds[selectedInstanceIds.length - 1] ??
        currentFrameInstanceId;
      const didMoveSelection = selectAdjacentFrame(
        event.key === "ArrowDown" ? 1 : -1,
        selectedAnchorInstanceId,
      );
      if (didMoveSelection) {
        event.preventDefault();
        event.stopPropagation();
      }
    }

    window.addEventListener("keydown", handleGlobalKeyboardShortcuts, true);
    return () => {
      window.removeEventListener(
        "keydown",
        handleGlobalKeyboardShortcuts,
        true,
      );
    };
  }, [
    activeDockPanel,
    currentFrameInstanceId,
    deleteSelectedFrames,
    frameDurationDialog,
    inspection?.isStaticImage,
    inspection?.ok,
    selectAdjacentFrame,
    selectedInstanceIds,
    showNthSelectionDialog,
    togglePlayback,
  ]);

  useEffect(() => {
    if (!isPlaying || !currentFrameInstanceId) {
      return;
    }

    selectSingleFrame(currentFrameInstanceId);
  }, [currentFrameInstanceId, isPlaying, selectSingleFrame]);

  const selectionLabel = selectionSummary(
    cropRegion,
    copy,
    inspection?.width ?? null,
    inspection?.height ?? null,
  );
  const lockedCropAspectRatio = cropAspectRatioValue(cropAspectRatioPreset);
  const resetCropRegion = useMemo(() => {
    const aspectRatio = lockedCropAspectRatio;
    const sourceWidth = inspection?.width ?? null;
    const sourceHeight = inspection?.height ?? null;
    if (aspectRatio !== null && sourceWidth !== null && sourceHeight !== null) {
      return constrainCropRegionToAspectRatio(
        FULL_CROP_REGION,
        sourceWidth,
        sourceHeight,
        aspectRatio,
      );
    }

    return FULL_CROP_REGION;
  }, [inspection?.height, inspection?.width, lockedCropAspectRatio]);
  const isCropSelectionReset = cropRegionsMatch(cropRegion, resetCropRegion);
  const previewDurationLabel =
    inspection && !inspection.isStaticImage && totalDuration > 0
      ? formatTimelineTime(totalDuration, locale)
      : null;

  useEffect(() => {
    setPreviewZoomMode("fit");
    setManualPreviewZoomScale(1);
    setResolvedPreviewZoomScale(1);
    setResolvedFitPreviewZoomScale(1);
    setActiveDockPanel(null);
  }, [editorSessionKey]);

  useEffect(() => {
    function updateEditorWorkspaceMinHeight() {
      const shell = shellRef.current;
      const workspace = editorWorkspaceRef.current;
      if (!shell || !workspace || !inspection?.ok) {
        setEditorWorkspaceMinHeight(null);
        return;
      }

      const shellStyles = window.getComputedStyle(shell);
      const shellBottomPadding =
        Number.parseFloat(shellStyles.paddingBottom) || 0;
      const workspaceRect = workspace.getBoundingClientRect();
      const nextMinHeight = Math.max(
        0,
        Math.floor(window.innerHeight - workspaceRect.top - shellBottomPadding),
      );

      setEditorWorkspaceMinHeight(nextMinHeight);
    }

    updateEditorWorkspaceMinHeight();
    window.addEventListener("resize", updateEditorWorkspaceMinHeight);

    return () => {
      window.removeEventListener("resize", updateEditorWorkspaceMinHeight);
    };
  }, [inspection?.inputPath, inspection?.isStaticImage, locale]);

  function handlePreviewZoomModeChange(nextMode: PreviewZoomMode) {
    setPreviewZoomMode(nextMode);
  }

  function handlePreviewZoomSliderChange(nextScale: number) {
    setPreviewZoomMode("manual");
    setManualPreviewZoomScale(clampPreviewZoomScale(nextScale));
  }

  function handlePreviewZoomStep(delta: number) {
    setPreviewZoomMode("manual");
    setManualPreviewZoomScale(
      clampPreviewZoomScale(resolvedPreviewZoomScale + delta),
    );
  }

  function resetCropSelection() {
    setCropRegion(resetCropRegion);
  }

  function handleCropAspectRatioPresetChange(
    nextPreset: CropAspectRatioPreset,
  ) {
    setCropAspectRatioPreset(nextPreset);

    const nextAspectRatio = cropAspectRatioValue(nextPreset);
    const sourceWidth = inspection?.width ?? null;
    const sourceHeight = inspection?.height ?? null;
    if (
      nextAspectRatio === null ||
      sourceWidth === null ||
      sourceHeight === null
    ) {
      return;
    }

    setCropRegion(() =>
      constrainCropRegionToAspectRatio(
        FULL_CROP_REGION,
        sourceWidth,
        sourceHeight,
        nextAspectRatio,
      ),
    );
  }

  async function handleBuildPreviewCandidates() {
    const result = await buildPlan({
      baseFrameCount: sourceFrames.length,
      editedTimelineFramesForRequest,
      fingerprint: workflowFingerprints.planner,
    });

    if (
      result &&
      result.fingerprint === workflowFingerprintRef.current.planner
    ) {
      setActiveDockPanel("preview");
    }
  }

  async function handlePreviewCandidatesToggle() {
    if (activeDockPanel === "preview") {
      setActiveDockPanel(null);
      return;
    }

    await handleBuildPreviewCandidates();
  }

  async function handleOptimizerRun() {
    const result = await runBoundedSearch({
      baseFrameCount: sourceFrames.length,
      editedTimelineFramesForRequest,
      fingerprint: workflowFingerprints.export,
    });

    if (
      result &&
      result.fingerprint === workflowFingerprintRef.current.export
    ) {
      setActiveDockPanel("results");
    }
  }

  function handleAdvancedSettingsToggle() {
    setActiveDockPanel((current) =>
      current === "settings" ? null : "settings",
    );
  }

  function handleResultsPanelToggle() {
    setActiveDockPanel((current) => (current === "results" ? null : "results"));
  }

  const editorWorkspaceStyle = useMemo<CSSProperties | undefined>(() => {
    if (!inspection?.ok || editorWorkspaceMinHeight === null) {
      return undefined;
    }

    return {
      "--editor-workspace-min-height": `${editorWorkspaceMinHeight}px`,
    } as CSSProperties;
  }, [editorWorkspaceMinHeight, inspection?.ok]);

  const staticImageResultsProps =
    inspection?.ok && inspection.isStaticImage
      ? {
          copy,
          locale,
          searchResult: null,
          conversionResult,
          onOpenOutputFolder: (path?: string | null) =>
            void openOutputFolder(path),
          variant: "page" as const,
        }
      : null;
  const fallbackWarning =
    inspection?.fallbackReasonCode === "media-foundation-failed"
      ? copy.mediaFoundationFallbackWarning
      : null;
  const primaryEstimateCard = inspection?.ok ? (
    <OutputSizeEstimateCard
      copy={copy}
      locale={locale}
      title={
        inspection.isStaticImage
          ? copy.outputSizeEstimate
          : copy.recommendedCandidateEstimate
      }
      estimate={primarySizeEstimate}
      estimateState={currentEstimateState}
      candidateId={inspection.isStaticImage ? null : recommendedCandidateId}
      desktopAvailable={supportsDesktopProcessing}
      waitingForPlan={!inspection.isStaticImage && fullPlan === null}
      planLoading={!inspection.isStaticImage && planLoading}
      probeState={currentProbeState}
      onRequestPlan={() => void handleBuildPreviewCandidates()}
      onRetryEstimate={outputSizeEstimate.retryEstimate}
      onProbeCandidate={outputSizeEstimate.probeCandidate}
      onCancelProbe={outputSizeEstimate.cancelProbe}
    />
  ) : null;

  return (
    <>
      {isDragging && (
        <div className="dragOverlay">
          <div className="dragOverlayContent">
            <FolderOpenIcon size={48} className="dragOverlayIcon" />
            <h2>{ui.inputPlaceholder || "Drop media file here"}</h2>
          </div>
        </div>
      )}
      <main
        ref={shellRef}
        className={
          isEditorLayoutActive
            ? "shell desktopShell desktopShellEditing"
            : "shell desktopShell"
        }
      >
        <DesktopHeader
          copy={copy}
          locale={locale}
          healthLabel={healthLabel}
          isToolReady={isToolReady}
          onLocaleChange={setLocale}
        />

        <PickerGrid
          copy={copy}
          inputLabel={filterPathLabel(
            inspection?.inputPath ?? null,
            ui.inputPlaceholder,
          )}
          outputLabel={outputDirectory ?? ui.outputPlaceholder}
          hasInputPath={Boolean(inspection?.inputPath)}
          hasOutputDirectory={Boolean(outputDirectory)}
          inspectionLoading={inspectionLoading}
          openFolderDisabled={!inspection?.inputPath && !outputDirectory}
          outputActionsDisabled={!supportsOutputDirectoryActions}
          onPickInputFile={() => void pickInputFile()}
          onPickOutputDirectory={() => void pickOutputDirectory()}
          onResetOutputDirectory={() => setOutputDirectory(null)}
          onOpenOutputFolder={() => void openOutputFolder()}
        />

        {!inspection ? (
          <section className="emptyState emptyStateWide">
            <p className="panelLabel">{copy.startHere}</p>
            <h2>{copy.pickSourceTitle}</h2>
            <p className="summaryText">{copy.pickSourceBody}</p>
            {isWebPreviewMode ? (
              <p className="summaryText">{copy.webPreviewNotice}</p>
            ) : null}
            {toolError ? <p className="errorText">{toolError}</p> : null}
          </section>
        ) : inspection.ok ? (
          <>
            <EditorWorkspace
              editorWorkspaceRef={editorWorkspaceRef}
              editorWorkspaceStyle={editorWorkspaceStyle}
              inspection={inspection}
              previewKey={inspection.previewSrc}
              shortcutSurfaceLabel={ui.previewTitle}
              isWebPreviewMode={isWebPreviewMode}
              webPreviewNotice={copy.webPreviewNotice}
              plannerError={plannerError}
              frameRailProps={{
                ui,
                locale,
                timelineFrameViews,
                selection: selectionModel,
                hasClipboardFrames,
                frameDropTarget,
                frameReorderState,
                frameTableBodyRef,
                activeInstanceId: activeFrameInstanceId,
                showFramePreviews: supportsRailFramePreviews,
                previewEntries: currentFramePreviewEntries,
                onVisibleRange: handleFramePreviewVisibleRange,
                onRetryFramePreview: handleRetryFramePreview,
                onFramePointerDown: handleFramePointerDown,
                onFrameContextMenu: handleFrameContextMenu,
                onFrameKeyDown: handleFrameKeyDown,
                onFrameFocus: scrubTo,
                onPasteFramesBelow: () => pasteClipboardAtSelection("below"),
              }}
              previewInfoBarProps={{
                copy,
                quickResolution,
                quickFps,
                previewDurationLabel,
                selectionLabel,
                cropAspectRatioPreset,
                isCropSelectionReset,
                onCropAspectRatioPresetChange:
                  handleCropAspectRatioPresetChange,
                onResetSelection: resetCropSelection,
              }}
              previewProps={{
                previewSrc: inspection.previewSrc,
                framePreviewSrc,
                requiresFramePreview: requiresBackendFramePreview,
                previewKind,
                sourceWidth: inspection.width,
                sourceHeight: inspection.height,
                cropRegion,
                lockedAspectRatio: lockedCropAspectRatio,
                onCropRegionChange: setCropRegion,
                onResetSelection: resetCropSelection,
                copy,
                isPlaying: !inspection.isStaticImage ? isPlaying : undefined,
                currentTime: !inspection.isStaticImage
                  ? previewCurrentTime
                  : undefined,
                onCurrentTimeChange: undefined,
                onDurationChange: !inspection.isStaticImage
                  ? handlePreviewDurationChange
                  : undefined,
                syncVideoTimeToParent: false,
                showDetails: false,
                previewZoomMode,
                manualZoomScale: manualPreviewZoomScale,
                onResolvedZoomChange: ({ effectiveScale, fitScale }) => {
                  setResolvedPreviewZoomScale(effectiveScale);
                  setResolvedFitPreviewZoomScale(fitScale);
                },
              }}
              overlayPanelProps={{
                activePanel: activeDockPanel,
                copy,
                locale,
                advancedSettingsPanelId: ADVANCED_SETTINGS_PANEL_ID,
                previewPanelId: EDITOR_PREVIEW_PANEL_ID,
                resultsPanelId: EDITOR_RESULTS_PANEL_ID,
                plan,
                searchResult,
                estimateState: currentEstimateState,
                estimateByCandidateId: outputSizeEstimate.estimateByCandidateId,
                probeState: currentProbeState,
                desktopAvailable: supportsDesktopProcessing,
                onRetryEstimate: outputSizeEstimate.retryEstimate,
                onProbeCandidate: outputSizeEstimate.probeCandidate,
                onCancelProbe: outputSizeEstimate.cancelProbe,
                optimizerGoal,
                qualityFrameDropInterval,
                optimizerSearchDepth,
                onOptimizerGoalChange: setOptimizerGoal,
                onQualityFrameDropIntervalChange: setQualityFrameDropInterval,
                onOptimizerSearchDepthChange: setOptimizerSearchDepth,
                onOpenOutputFolder: (path) => void openOutputFolder(path),
                onClose: () => setActiveDockPanel(null),
              }}
              previewControlBarProps={{
                copy,
                ui,
                locale,
                isStaticImage: inspection.isStaticImage,
                currentTime,
                totalDuration,
                currentTimeUs: currentTimelineTimeUs,
                totalDurationUs: totalTimelineDurationUs,
                timelineFrameViews,
                selection: selectionModel,
                isPlaying,
                timelineRailStyle,
                timelineRailRef,
                previewZoomMode,
                previewZoomPercent: Math.round(resolvedPreviewZoomScale * 100),
                previewZoomSliderValue: resolvedPreviewZoomScale * 100,
                previewZoomSliderMin: Math.max(
                  0.1,
                  Math.min(10, resolvedFitPreviewZoomScale * 100),
                ),
                onTogglePlayback: togglePlayback,
                onPointerDown: handleTimelinePointerDown,
                onPointerMove: handleTimelinePointerMove,
                onPointerUp: handleTimelinePointerEnd,
                onPointerCancel: handleTimelinePointerEnd,
                onTimelineTimeChangeUs: (nextTimeUs) =>
                  scrubTo(microsecondsToSeconds(nextTimeUs)),
                onPreviewZoomFit: () => handlePreviewZoomModeChange("fit"),
                onPreviewZoomStep: handlePreviewZoomStep,
                onPreviewZoomChange: handlePreviewZoomSliderChange,
              }}
              previewUtilityActionsProps={{
                activeDockPanel,
                copy,
                isStaticImage: inspection.isStaticImage,
                canConvertToPng: inspection.canConvertToPng,
                conversionLoading,
                planLoading,
                searchLoading,
                timelineFrameCount: timelineFrames.length,
                supportsDesktopProcessing,
                hasSearchResult: searchResult !== null,
                estimateCard: primaryEstimateCard,
                operationProgress: primaryOperationProgress,
                operationCancelled: primaryOperationCancelled,
                fallbackWarning,
                advancedSettingsPanelId: ADVANCED_SETTINGS_PANEL_ID,
                previewPanelId: EDITOR_PREVIEW_PANEL_ID,
                resultsPanelId: EDITOR_RESULTS_PANEL_ID,
                onTogglePreviewCandidates: () =>
                  void handlePreviewCandidatesToggle(),
                onToggleAdvancedSettings: handleAdvancedSettingsToggle,
                onToggleResults: handleResultsPanelToggle,
                onRunOptimizer: () => void handleOptimizerRun(),
                onCancelOptimizer: cancelOptimizerSearch,
                onConvertToPng: () =>
                  void convertStaticImageToPng(workflowFingerprints.export),
              }}
              staticImageResultsProps={staticImageResultsProps}
            />
          </>
        ) : (
          <InspectionErrorCard
            copy={copy}
            message={
              mediaOperationMessage(
                locale,
                inspection.errorCode,
                inspection.reasonCode,
              ) ?? copy.inspectionFailed
            }
          />
        )}
      </main>

      <FrameEditingOverlays
        frameContextMenuProps={
          frameContextMenu
            ? {
                ui,
                frameContextMenu,
                frameContextMenuRef,
                hasSingleFrameSelection,
                canDeleteUnselectedFrames,
                hasClipboardFrames,
                onClose: closeFrameContextMenu,
                onOpenFrameDurationDialog: openFrameDurationDialog,
                onSplitCurrentFrame: splitCurrentFrame,
                onSpeedUpFrames: () => speedAdjustSelectedFrames(1 / 1.1),
                onSlowDownFrames: () => speedAdjustSelectedFrames(1.1),
                onCopyFramesToStart: () => copySelectedFramesTo("start"),
                onMoveFramesToStart: () => moveSelectedFramesTo("start"),
                onMoveFramesUp: () => moveSelectedFrames(-1),
                onMoveFramesDown: () => moveSelectedFrames(1),
                onCopyFramesToEnd: () => copySelectedFramesTo("end"),
                onMoveFramesToEnd: () => moveSelectedFramesTo("end"),
                onReverseFrames: reverseSelectedFrames,
                onDeleteUnselectedFrames: deleteUnselectedFrames,
                onSelectAllFrames: selectAllFrames,
                onSelectOddFrames: selectOddFrames,
                onSelectEvenFrames: selectEvenFrames,
                onOpenNthFrameDialog: openNthSelectionDialog,
                onClearAllFrames: clearAllFrames,
                onInvertSelection: invertFrameSelection,
                onRenumberFrames: renumberTimelineFrames,
                onCopyFrames: copyFramesToClipboard,
                onCutFrames: cutFramesToClipboard,
                onPasteFramesAbove: () => pasteClipboard("above"),
                onPasteFramesBelow: () => pasteClipboard("below"),
              }
            : null
        }
        frameDialogsProps={{
          ui,
          frameDurationDialog,
          frameDurationDialogRef,
          frameDurationMode,
          frameDurationFpsValue,
          frameDurationSecondsValue,
          hasMixedSelectedDurations,
          onFrameDurationModeChange: setFrameDurationMode,
          onFrameDurationFpsChange: updateFrameDurationFromFps,
          onFrameDurationSecondsChange: updateFrameDurationFromSeconds,
          onApplyFrameDuration: applyDurationChange,
          onCloseFrameDurationDialog: closeFrameDurationDialog,
          showNthSelectionDialog,
          nthSelectionDialogRef,
          nthSelectionStep,
          onNthSelectionStepChange: setNthSelectionStep,
          onApplyNthFrameSelection: applyNthFrameSelection,
          onCloseNthSelectionDialog: closeNthSelectionDialog,
        }}
      />
    </>
  );
}
