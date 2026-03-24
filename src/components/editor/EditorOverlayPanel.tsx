import { AdvancedDetailsPanel } from "../AdvancedDetailsPanel";
import { CloseIcon } from "../AppIcons";
import { EditorResultsOverlay } from "./EditorResultsOverlay";
import { AdvancedOptimizerSettingsPanel } from "./AdvancedOptimizerSettingsPanel";
import type { Locale, MessagesForLocale } from "../../locales/messages";
import type {
  OptimizerPlanResponse,
  OptimizerPresetStrategy,
  OptimizerSearchDepth,
  OptimizerSearchResponse,
} from "../../types/workflow";

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
  optimizerPresetStrategy: OptimizerPresetStrategy;
  optimizerSearchDepth: OptimizerSearchDepth;
  onOptimizerPresetStrategyChange: (value: OptimizerPresetStrategy) => void;
  onOptimizerSearchDepthChange: (value: OptimizerSearchDepth) => void;
  onOpenOutputFolder: (path?: string | null) => void;
  onClose: () => void;
  fitModeLabel: (value: string) => string;
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
  optimizerPresetStrategy,
  optimizerSearchDepth,
  onOptimizerPresetStrategyChange,
  onOptimizerSearchDepthChange,
  onOpenOutputFolder,
  onClose,
  fitModeLabel,
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
              optimizerPresetStrategy={optimizerPresetStrategy}
              optimizerSearchDepth={optimizerSearchDepth}
              onOptimizerPresetStrategyChange={onOptimizerPresetStrategyChange}
              onOptimizerSearchDepthChange={onOptimizerSearchDepthChange}
            />
          ) : null}

          {activePanel === "preview" ? (
            <AdvancedDetailsPanel
              copy={copy}
              locale={locale}
              plan={plan}
              searchResult={null}
              fitModeLabel={fitModeLabel}
              variant="dock"
            />
          ) : null}

          {activePanel === "results" ? (
            <EditorResultsOverlay
              copy={copy}
              locale={locale}
              searchResult={searchResult}
              fitModeLabel={fitModeLabel}
              onOpenOutputFolder={onOpenOutputFolder}
            />
          ) : null}
        </div>
      </section>
    </div>
  );
}
