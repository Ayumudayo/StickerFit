import { AdvancedDetailsPanel } from "../AdvancedDetailsPanel";
import { CloseIcon } from "../AppIcons";
import { EditorResultsOverlay } from "./EditorResultsOverlay";
import { AdvancedOptimizerSettingsPanel } from "./AdvancedOptimizerSettingsPanel";
import type { Locale, MessagesForLocale } from "../../locales/messages";
import type {
  OperationProgress,
  OutputSizeEstimate,
  OptimizerGoal,
  OptimizerPlanResponse,
  OptimizerSearchDepth,
  OptimizerSearchResponse,
} from "../../types/workflow";
import type { VersionedWorkflowState } from "../../hooks/mediaWorkflow/workflowFingerprint";
import type { VersionedProbeState } from "../../hooks/outputSizeEstimateCoordinator";

export type EditorDockPanelMode = "preview" | "results" | "settings";

type EditorOverlayPanelProps = {
  activePanel: EditorDockPanelMode | null;
  copy: MessagesForLocale;
  locale: Locale;
  advancedSettingsPanelId: string;
  previewPanelId: string;
  resultsPanelId: string;
  plan: OptimizerPlanResponse | null;
  searchResult: OptimizerSearchResponse | null;
  estimateState: VersionedWorkflowState<OutputSizeEstimate[], OperationProgress>;
  estimateByCandidateId: ReadonlyMap<string, OutputSizeEstimate>;
  probeState: VersionedProbeState;
  desktopAvailable: boolean;
  onRetryEstimate: () => void;
  onProbeCandidate: (candidateId: string) => void;
  onCancelProbe: () => void;
  optimizerGoal: OptimizerGoal;
  qualityFrameDropInterval: number;
  optimizerSearchDepth: OptimizerSearchDepth;
  onOptimizerGoalChange: (value: OptimizerGoal) => void;
  onQualityFrameDropIntervalChange: (value: number) => void;
  onOptimizerSearchDepthChange: (value: OptimizerSearchDepth) => void;
  onOpenOutputFolder: (path?: string | null) => void;
  onClose: () => void;
};

export function EditorOverlayPanel({
  activePanel,
  copy,
  locale,
  advancedSettingsPanelId,
  previewPanelId,
  resultsPanelId,
  plan,
  searchResult,
  estimateState,
  estimateByCandidateId,
  probeState,
  desktopAvailable,
  onRetryEstimate,
  onProbeCandidate,
  onCancelProbe,
  optimizerGoal,
  qualityFrameDropInterval,
  optimizerSearchDepth,
  onOptimizerGoalChange,
  onQualityFrameDropIntervalChange,
  onOptimizerSearchDepthChange,
  onOpenOutputFolder,
  onClose,
}: EditorOverlayPanelProps) {
  if (!activePanel) {
    return null;
  }

  const panelId =
    activePanel === "settings"
      ? advancedSettingsPanelId
      : activePanel === "preview"
        ? previewPanelId
        : resultsPanelId;
  const panelLabel =
    activePanel === "settings"
      ? copy.advancedSettings
      : activePanel === "preview"
        ? copy.topPreviewCandidates
        : copy.searchSummary;

  return (
    <div className="editorOverlayShell">
      <button
        className="editorOverlayBackdrop"
        type="button"
        aria-label={copy.closePanel}
        onClick={onClose}
      />
      <section
        className={`editorDockPanel editorDockPanel--${activePanel}`}
        aria-live="polite"
        role="dialog"
        aria-modal="true"
        id={panelId}
      >
        <div className="editorDockPanelHeader">
          <p className="panelLabel">{panelLabel}</p>
          <button
            className="subtleAction editorDockCloseButton"
            type="button"
            aria-label={copy.closePanel}
            onClick={onClose}
          >
            <CloseIcon size={16} />
          </button>
        </div>

        <div className="editorDockPanelBody">
          {activePanel === "settings" ? (
            <AdvancedOptimizerSettingsPanel
              panelId={advancedSettingsPanelId}
              layout="dock"
              copy={copy}
              optimizerGoal={optimizerGoal}
              qualityFrameDropInterval={qualityFrameDropInterval}
              optimizerSearchDepth={optimizerSearchDepth}
              onOptimizerGoalChange={onOptimizerGoalChange}
              onQualityFrameDropIntervalChange={onQualityFrameDropIntervalChange}
              onOptimizerSearchDepthChange={onOptimizerSearchDepthChange}
            />
          ) : null}

          {activePanel === "preview" ? (
            <AdvancedDetailsPanel
              copy={copy}
              locale={locale}
              plan={plan}
              searchResult={null}
              estimateState={estimateState}
              estimateByCandidateId={estimateByCandidateId}
              probeState={probeState}
              desktopAvailable={desktopAvailable}
              onRetryEstimate={onRetryEstimate}
              onProbeCandidate={onProbeCandidate}
              onCancelProbe={onCancelProbe}
              variant="dock"
            />
          ) : null}

          {activePanel === "results" ? (
            <EditorResultsOverlay
              copy={copy}
              locale={locale}
              searchResult={searchResult}
              onOpenOutputFolder={onOpenOutputFolder}
            />
          ) : null}
        </div>
      </section>
    </div>
  );
}
