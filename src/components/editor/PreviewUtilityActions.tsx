import type { ReactNode } from "react";

import type { MessagesForLocale } from "../../locales/messages";
import type { OperationProgress } from "../../types/workflow";
import { ExpandIcon } from "../AppIcons";
import type { EditorDockPanelMode } from "./EditorOverlayPanel";
import {
  OperationProgressIndicator,
  operationProgressMessage,
} from "./OutputSizeEstimateCard";

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
  estimateCard: ReactNode;
  operationProgress: OperationProgress | null;
  operationCancelled: boolean;
  fallbackWarning: string | null;
  advancedSettingsPanelId: string;
  previewPanelId: string;
  resultsPanelId: string;
  onTogglePreviewCandidates: () => void;
  onToggleAdvancedSettings: () => void;
  onToggleResults: () => void;
  onRunOptimizer: () => void;
  onCancelOptimizer: () => void;
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
  estimateCard,
  operationProgress,
  operationCancelled,
  fallbackWarning,
  advancedSettingsPanelId,
  previewPanelId,
  resultsPanelId,
  onTogglePreviewCandidates,
  onToggleAdvancedSettings,
  onToggleResults,
  onRunOptimizer,
  onCancelOptimizer,
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
  const staticImageDisabledReason = !canConvertToPng
    ? copy.desktopOnlyFeature
    : undefined;

  if (isStaticImage) {
    return (
      <div className="previewUtilityArea previewUtilityAreaStatic">
        <div className="previewUtilityPrimaryStack">
          {fallbackWarning ? (
            <section className="noticeCard outputEstimateNotice" role="status">
              <p>{fallbackWarning}</p>
            </section>
          ) : null}
          {conversionLoading ? (
            <OperationProgressIndicator
              label={copy.convertingToPng}
              message={
                operationProgress
                  ? operationProgressMessage(copy, operationProgress)
                  : copy.mediaOperationQueued
              }
              progress={operationProgress}
            />
          ) : operationCancelled ? (
            <p
              className="summaryText operationCancelledMessage"
              role="status"
              aria-live="polite"
              aria-atomic="true"
            >
              {copy.operationCancelled}
            </p>
          ) : null}
          {estimateCard}
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
            <span>
              {conversionLoading ? copy.convertingToPng : copy.convertToPng}
            </span>
            <ExpandIcon size={18} className="ctaIcon gapIcon" />
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="previewUtilityArea">
      <div className="previewUtilityStrip">
        <button
          className="secondaryAction previewUtilityButton"
          type="button"
          disabled={
            !supportsDesktopProcessing ||
            planLoading ||
            timelineFrameCount === 0
          }
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
          {activeDockPanel === "settings"
            ? copy.hideAdvancedSettings
            : copy.showAdvancedSettings}
        </button>
        {hasSearchResult ? (
          <button
            className="secondaryAction previewUtilityButton"
            type="button"
            aria-expanded={activeDockPanel === "results"}
            aria-controls={resultsPanelId}
            onClick={onToggleResults}
          >
            {activeDockPanel === "results"
              ? copy.hideResults
              : copy.viewResults}
          </button>
        ) : null}
      </div>

      <div className="previewUtilityPrimaryStack">
        {fallbackWarning ? (
          <section className="noticeCard outputEstimateNotice" role="status">
            <p>{fallbackWarning}</p>
          </section>
        ) : null}
        {searchLoading ? (
          <OperationProgressIndicator
            label={copy.runningOptimizer}
            message={
              operationProgress
                ? operationProgressMessage(copy, operationProgress)
                : copy.mediaOperationQueued
            }
            progress={operationProgress}
          />
        ) : operationCancelled ? (
          <p
            className="summaryText operationCancelledMessage"
            role="status"
            aria-live="polite"
            aria-atomic="true"
          >
            {copy.operationCancelled}
          </p>
        ) : null}
        {estimateCard}
        <button
          className="primaryAction previewUtilityPrimaryAction"
          type="button"
          disabled={
            !searchLoading &&
            (!supportsDesktopProcessing || timelineFrameCount === 0)
          }
          title={searchLoading ? undefined : optimizerDisabledReason}
          aria-label={
            searchLoading
              ? copy.cancelOptimizer
              : optimizerDisabledReason
                ? `${copy.runOptimizer}. ${optimizerDisabledReason}`
                : copy.runOptimizer
          }
          onClick={searchLoading ? onCancelOptimizer : onRunOptimizer}
        >
          <span>
            {searchLoading ? copy.cancelOptimizer : copy.runOptimizer}
          </span>
          <ExpandIcon size={18} className="ctaIcon gapIcon" />
        </button>
      </div>
    </div>
  );
}
