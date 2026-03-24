import type { MessagesForLocale } from "../../locales/messages";
import { ExpandIcon } from "../AppIcons";
import type { EditorDockPanelMode } from "./EditorOverlayPanel";

type PreviewUtilityActionsProps = {
  activeDockPanel: EditorDockPanelMode | null;
  copy: MessagesForLocale;
  isStaticImage: boolean;
  canConvertToPng: boolean;
  conversionLoading: boolean;
  planLoading: boolean;
  searchLoading: boolean;
  timelineFrameCount: number;
  supportsDesktopProcessing: boolean;
  hasSearchResult: boolean;
  advancedSettingsPanelId: string;
  previewPanelId: string;
  resultsPanelId: string;
  onTogglePreviewCandidates: () => void;
  onToggleAdvancedSettings: () => void;
  onToggleResults: () => void;
  onRunOptimizer: () => void;
  onConvertToPng: () => void;
};

export function PreviewUtilityActions({
  activeDockPanel,
  copy,
  isStaticImage,
  canConvertToPng,
  conversionLoading,
  planLoading,
  searchLoading,
  timelineFrameCount,
  supportsDesktopProcessing,
  hasSearchResult,
  advancedSettingsPanelId,
  previewPanelId,
  resultsPanelId,
  onTogglePreviewCandidates,
  onToggleAdvancedSettings,
  onToggleResults,
  onRunOptimizer,
  onConvertToPng,
}: PreviewUtilityActionsProps) {
  const previewCandidatesDisabledReason = !supportsDesktopProcessing
    ? copy.desktopOnlyFeature
    : timelineFrameCount === 0
      ? copy.selectFramesFirst
      : undefined;
  const optimizerDisabledReason = !supportsDesktopProcessing
    ? copy.desktopOnlyFeature
    : timelineFrameCount === 0
      ? copy.selectFramesFirst
      : undefined;
  const staticImageDisabledReason = !canConvertToPng ? copy.desktopOnlyFeature : undefined;

  if (isStaticImage) {
    return (
      <div className="previewUtilityArea previewUtilityAreaStatic">
        <button
          className="primaryAction previewUtilityPrimaryAction"
          type="button"
          disabled={!canConvertToPng || conversionLoading}
          title={staticImageDisabledReason}
          aria-label={
            staticImageDisabledReason
              ? `${copy.convertToPng}. ${staticImageDisabledReason}`
              : copy.convertToPng
          }
          onClick={onConvertToPng}
        >
          <span>{conversionLoading ? copy.convertingToPng : copy.convertToPng}</span>
          <ExpandIcon size={18} className="ctaIcon gapIcon" />
        </button>
      </div>
    );
  }

  return (
    <div className="previewUtilityArea">
      <div className="previewUtilityStrip">
        <button
          className="secondaryAction previewUtilityButton"
          type="button"
          disabled={!supportsDesktopProcessing || planLoading || timelineFrameCount === 0}
          title={previewCandidatesDisabledReason}
          aria-label={
            previewCandidatesDisabledReason
              ? `${copy.buildPreview}. ${previewCandidatesDisabledReason}`
              : copy.buildPreview
          }
          aria-expanded={activeDockPanel === "preview"}
          aria-controls={previewPanelId}
          onClick={onTogglePreviewCandidates}
        >
          {planLoading
            ? copy.buildingPreview
            : activeDockPanel === "preview"
              ? copy.hidePreviewCandidates
              : copy.buildPreview}
        </button>
        <button
          className="secondaryAction previewUtilityButton"
          type="button"
          aria-expanded={activeDockPanel === "settings"}
          aria-controls={advancedSettingsPanelId}
          onClick={onToggleAdvancedSettings}
        >
          {activeDockPanel === "settings" ? copy.hideAdvancedSettings : copy.showAdvancedSettings}
        </button>
        {hasSearchResult ? (
          <button
            className="secondaryAction previewUtilityButton"
            type="button"
            aria-expanded={activeDockPanel === "results"}
            aria-controls={resultsPanelId}
            onClick={onToggleResults}
          >
            {activeDockPanel === "results" ? copy.hideResults : copy.viewResults}
          </button>
        ) : null}
      </div>

      <button
        className="primaryAction previewUtilityPrimaryAction"
        type="button"
        disabled={!supportsDesktopProcessing || searchLoading || timelineFrameCount === 0}
        title={optimizerDisabledReason}
        aria-label={
          optimizerDisabledReason
            ? `${copy.runOptimizer}. ${optimizerDisabledReason}`
            : copy.runOptimizer
        }
        onClick={onRunOptimizer}
      >
        <span>{searchLoading ? copy.runningOptimizer : copy.runOptimizer}</span>
        <ExpandIcon size={18} className="ctaIcon gapIcon" />
      </button>
    </div>
  );
}
