import {
  type CSSProperties,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { FolderOpenIcon } from "./components/AppIcons";
import { DesktopHeader } from "./components/editor/DesktopHeader";
import { type EditorDockPanelMode } from "./components/editor/EditorOverlayPanel";
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
import { useMediaWorkflowController } from "./hooks/useMediaWorkflowController";
import { usePlaybackTimelineController } from "./hooks/usePlaybackTimelineController";
import { editorText } from "./locales/editorText";
import { detectLocale, MESSAGES, type Locale, type MessagesForLocale } from "./locales/messages";
import type { FitMode } from "./types/workflow";
import { formatTimelineTime } from "./utils/timelineFrames";

const ADVANCED_PREVIEW_COUNT = 6;
const ADVANCED_SETTINGS_PANEL_ID = "advanced-optimizer-settings";
const EDITOR_RESULTS_PANEL_ID = "editor-results-panel";
const EDITOR_PREVIEW_PANEL_ID = "editor-preview-panel";

const MIN_DURATION_US = 10_000;

function filterPathLabel(value: string | null, fallback: string) {
  return value?.trim() ? value : fallback;
}

function fitModeText(fitMode: FitMode, copy: MessagesForLocale) {
  if (fitMode === "cover") {
    return copy.cover;
  }

  if (fitMode === "fill") {
    return copy.fill;
  }

  return copy.contain;
}
export default function App() {
  const [locale, setLocale] = useState<Locale>(detectLocale());
  const [previewDuration, setPreviewDuration] = useState<number | null>(null);
  const [previewZoomMode, setPreviewZoomMode] = useState<PreviewZoomMode>("fit");
  const [manualPreviewZoomScale, setManualPreviewZoomScale] = useState(1);
  const [resolvedPreviewZoomScale, setResolvedPreviewZoomScale] = useState(1);
  const [resolvedFitPreviewZoomScale, setResolvedFitPreviewZoomScale] = useState(1);
  const [editorSessionKey, setEditorSessionKey] = useState(0);
  const [editorWorkspaceMinHeight, setEditorWorkspaceMinHeight] = useState<number | null>(null);
  const [activeDockPanel, setActiveDockPanel] = useState<EditorDockPanelMode | null>(null);
  const initialLocaleRef = useRef(locale);
  const timelineRailRef = useRef<HTMLDivElement | null>(null);
  const shellRef = useRef<HTMLElement | null>(null);
  const editorWorkspaceRef = useRef<HTMLElement | null>(null);

  const mediaWorkflow = useMediaWorkflowController({
    locale,
    initialLocale: initialLocaleRef.current,
    advancedPreviewCount: ADVANCED_PREVIEW_COUNT,
    onCommitEditorSession: () => {
      setEditorSessionKey((current) => current + 1);
    },
  });
  const {
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
    setOptimizerSearchDepth,
    setCropRegion,
    setCropAspectRatioPreset,
    pickInputFile,
    pickOutputDirectory,
    openOutputFolder,
    buildPlan,
    runBoundedSearch,
    convertStaticImageToPng,
  } = mediaWorkflow;

  const copy = MESSAGES[locale];
  const ui = useMemo(() => editorText(locale), [locale]);
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
    handleFramePointerEnter,
    handleFrameKeyDown,
    handleFrameContextMenu,
    selectAllFrames,
    clearAllFrames,
    openFrameDurationDialog,
    splitCurrentFrame,
    speedAdjustSelectedFrames,
    copySelectedFramesTo,
    moveSelectedFramesTo,
    moveSelectedFrames,
    reverseSelectedFrames,
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
    () => buildEditedTimelineFramesForRequest(timelineFrames),
    [timelineFrames],
  );
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

  const isWebPreviewMode = runtime.kind === "web";
  const isEditorLayoutActive = inspection?.ok === true;
  const supportsDesktopProcessing = runtime.capabilities.backendProcessing;
  const supportsOutputDirectoryActions =
    runtime.capabilities.outputDirectorySelection && runtime.capabilities.openOutputFolder;
  const healthLabel = isWebPreviewMode
    ? copy.webPreviewMode
    : toolReport?.ready
      ? copy.toolReady
      : copy.toolUnavailable;
  const isToolReady = isWebPreviewMode || toolReport?.ready === true;
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
    if (!inspection?.inputPath) {
      setPreviewZoomMode("fit");
      setManualPreviewZoomScale(1);
      setResolvedPreviewZoomScale(1);
      setResolvedFitPreviewZoomScale(1);
      setActiveDockPanel(null);
      return;
    }

    setPreviewZoomMode("fit");
    setManualPreviewZoomScale(1);
    setResolvedPreviewZoomScale(1);
    setResolvedFitPreviewZoomScale(1);
    setActiveDockPanel(null);
  }, [inspection?.inputPath]);

  useEffect(() => {
    function updateEditorWorkspaceMinHeight() {
      const shell = shellRef.current;
      const workspace = editorWorkspaceRef.current;
      if (!shell || !workspace || !inspection?.ok) {
        setEditorWorkspaceMinHeight(null);
        return;
      }

      const shellStyles = window.getComputedStyle(shell);
      const shellBottomPadding = Number.parseFloat(shellStyles.paddingBottom) || 0;
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

  function handleCropAspectRatioPresetChange(nextPreset: CropAspectRatioPreset) {
    setCropAspectRatioPreset(nextPreset);

    const nextAspectRatio = cropAspectRatioValue(nextPreset);
    const sourceWidth = inspection?.width ?? null;
    const sourceHeight = inspection?.height ?? null;
    if (nextAspectRatio === null || sourceWidth === null || sourceHeight === null) {
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

  async function handlePreviewCandidatesToggle() {
    if (activeDockPanel === "preview") {
      setActiveDockPanel(null);
      return;
    }

    const result = await buildPlan({
      baseFrameCount: sourceFrames.length,
      editedTimelineFramesForRequest,
    });

    if (result) {
      setActiveDockPanel("preview");
    }
  }

  async function handleOptimizerRun() {
    const result = await runBoundedSearch({
      baseFrameCount: sourceFrames.length,
      editedTimelineFramesForRequest,
    });

    if (result) {
      setActiveDockPanel("results");
    }
  }

  function handleAdvancedSettingsToggle() {
    setActiveDockPanel((current) => (current === "settings" ? null : "settings"));
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
          onOpenOutputFolder: (path?: string | null) => void openOutputFolder(path),
          variant: "page" as const,
        }
      : null;

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
        className={isEditorLayoutActive ? "shell desktopShell desktopShellEditing" : "shell desktopShell"}
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
          inputLabel={filterPathLabel(inspection?.inputPath ?? null, ui.inputPlaceholder)}
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
            {isWebPreviewMode ? <p className="summaryText">{copy.webPreviewNotice}</p> : null}
            {toolError ? <p className="errorText">{toolError}</p> : null}
          </section>
        ) : inspection.ok ? (
          <>
            <EditorWorkspace
              editorWorkspaceRef={editorWorkspaceRef}
              editorWorkspaceStyle={editorWorkspaceStyle}
              inspection={inspection}
              previewKey={inspection.previewSrc}
              isWebPreviewMode={isWebPreviewMode}
              webPreviewNotice={copy.webPreviewNotice}
              plannerError={plannerError}
              frameRailProps={{
                ui,
                locale,
                timelineFrameViews,
                selection: selectionModel,
                currentFrameInstanceId,
                hasClipboardFrames,
                frameDropTarget,
                frameReorderState,
                frameTableBodyRef,
                onFramePointerDown: handleFramePointerDown,
                onFramePointerEnter: handleFramePointerEnter,
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
                fitMode,
                cropAspectRatioPreset,
                isStaticImage: inspection.isStaticImage,
                isCropSelectionReset,
                onFitModeChange: setFitMode,
                onCropAspectRatioPresetChange: handleCropAspectRatioPresetChange,
                onResetSelection: resetCropSelection,
              }}
              previewProps={{
                previewSrc: inspection.previewSrc,
                previewKind,
                sourceWidth: inspection.width,
                sourceHeight: inspection.height,
                cropRegion,
                lockedAspectRatio: lockedCropAspectRatio,
                onCropRegionChange: setCropRegion,
                onResetSelection: resetCropSelection,
                copy,
                isPlaying: !inspection.isStaticImage ? isPlaying : undefined,
                currentTime: !inspection.isStaticImage ? previewCurrentTime : undefined,
                onCurrentTimeChange: undefined,
                onDurationChange: !inspection.isStaticImage ? handlePreviewDurationChange : undefined,
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
                optimizerPresetStrategy,
                optimizerSearchDepth,
                onOptimizerPresetStrategyChange: setOptimizerPresetStrategy,
                onOptimizerSearchDepthChange: setOptimizerSearchDepth,
                onOpenOutputFolder: (path) => void openOutputFolder(path),
                onClose: () => setActiveDockPanel(null),
                fitModeLabel: (value) => fitModeText(value as FitMode, copy),
              }}
              previewControlBarProps={{
                copy,
                ui,
                locale,
                isStaticImage: inspection.isStaticImage,
                currentTime,
                totalDuration,
                timelineFrameViews,
                selection: selectionModel,
                isPlaying,
                timelineRailStyle,
                timelineRailRef,
                previewZoomMode,
                previewZoomPercent: Math.round(resolvedPreviewZoomScale * 100),
                previewZoomSliderValue: resolvedPreviewZoomScale * 100,
                previewZoomSliderMin: Math.max(0.1, Math.min(10, resolvedFitPreviewZoomScale * 100)),
                onTogglePlayback: togglePlayback,
                onPointerDown: handleTimelinePointerDown,
                onPointerMove: handleTimelinePointerMove,
                onPointerUp: handleTimelinePointerEnd,
                onPointerCancel: handleTimelinePointerEnd,
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
                advancedSettingsPanelId: ADVANCED_SETTINGS_PANEL_ID,
                previewPanelId: EDITOR_PREVIEW_PANEL_ID,
                resultsPanelId: EDITOR_RESULTS_PANEL_ID,
                onTogglePreviewCandidates: () => void handlePreviewCandidatesToggle(),
                onToggleAdvancedSettings: handleAdvancedSettingsToggle,
                onToggleResults: handleResultsPanelToggle,
                onRunOptimizer: () => void handleOptimizerRun(),
                onConvertToPng: () => void convertStaticImageToPng(),
              }}
              staticImageResultsProps={staticImageResultsProps}
            />

          </>
        ) : (
          <InspectionErrorCard copy={copy} message={inspection.errorMessage} />
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















