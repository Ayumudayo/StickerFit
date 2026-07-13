import {
  mediaOperationMessage,
  type Locale,
  type MessagesForLocale,
} from "../locales/messages";
import type {
  OperationProgress,
  OutputSizeEstimate,
  OptimizerPlanResponse,
  OptimizerSearchResponse,
} from "../types/workflow";
import type { VersionedWorkflowState } from "../hooks/mediaWorkflow/workflowFingerprint";
import type { VersionedProbeState } from "../hooks/outputSizeEstimateCoordinator";
import { useCspDiagnostics } from "../platform/cspDiagnostics";
import {
  formatDuration,
  formatSimilarityScore,
  formatKiB,
  formatScale,
  presetLabel,
  selectionReasonLabel,
  statusClassName,
  statusText,
  stopReasonLabel,
} from "../utils/formatters";
import { compactPathLabel } from "../utils/pathLabels";
import { OutputSizeEstimateCard } from "./editor/OutputSizeEstimateCard";

type AdvancedDetailsPanelProps = {
  copy: MessagesForLocale;
  locale: Locale;
  plan: OptimizerPlanResponse | null;
  searchResult: OptimizerSearchResponse | null;
  estimateState: VersionedWorkflowState<
    OutputSizeEstimate[],
    OperationProgress
  >;
  estimateByCandidateId: ReadonlyMap<string, OutputSizeEstimate>;
  probeState: VersionedProbeState;
  desktopAvailable: boolean;
  onRetryEstimate: () => void;
  onProbeCandidate: (candidateId: string) => void;
  onCancelProbe: () => void;
  variant?: "page" | "dock";
};

export function AdvancedDetailsPanel({
  copy,
  locale,
  plan,
  searchResult,
  estimateState,
  estimateByCandidateId,
  probeState,
  desktopAvailable,
  onRetryEstimate,
  onProbeCandidate,
  onCancelProbe,
  variant = "page",
}: AdvancedDetailsPanelProps) {
  const { violationCount } = useCspDiagnostics();
  const hasAdvancedContent = Boolean(plan) || Boolean(searchResult);
  const selectedResultCandidateId =
    searchResult?.winningCandidateId ??
    searchResult?.closestCandidateId ??
    null;
  const hasVisibleCandidateEstimate =
    plan?.candidates.some((candidate) =>
      estimateByCandidateId.has(candidate.id),
    ) ?? false;
  const showSharedCandidateEstimateState =
    estimateState.status === "loading" ||
    estimateState.status === "error" ||
    estimateState.status === "cancelled" ||
    !hasVisibleCandidateEstimate;
  const panelClassName =
    variant === "dock" ? "detailsPanel detailsPanelDock" : "detailsPanel";

  return (
    <section className={panelClassName}>
      {variant === "page" ? (
        <p className="panelLabel">{copy.advancedDetails}</p>
      ) : null}
      <section
        className="detailsCard detailsCardWide"
        aria-live="polite"
        aria-atomic="true"
      >
        <p className="summaryText">{copy.cspViolationCount(violationCount)}</p>
      </section>
      {!hasAdvancedContent ? (
        <section className="detailsCard detailsCardWide">
          <p className="summaryText">{copy.noPlanYet}</p>
        </section>
      ) : (
        <div className="detailsGrid">
          {plan ? (
            <section className="detailsCard detailsCardWide">
              <p className="panelLabel">{copy.topPreviewCandidates}</p>
              <p className="summaryText">
                {copy.previewBudgetNote(
                  plan.searchBudget,
                  plan.candidates.length,
                )}
              </p>
              <p className="detailText">{copy.sourceMatchHint}</p>
              {plan.warnings.length > 0 ? (
                <div className="warningBox">
                  <p className="metaLabel">{copy.warnings}</p>
                  <ul className="warningList">
                    {plan.warnings.map((warning) => (
                      <li key={warning}>{warning}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              {showSharedCandidateEstimateState ? (
                <OutputSizeEstimateCard
                  copy={copy}
                  locale={locale}
                  title={copy.outputSizeEstimate}
                  estimate={null}
                  estimateState={estimateState}
                  candidateId={null}
                  desktopAvailable={desktopAvailable}
                  probeState={probeState}
                  onRetryEstimate={onRetryEstimate}
                  onProbeCandidate={onProbeCandidate}
                  onCancelProbe={onCancelProbe}
                />
              ) : null}
              <div className="advancedGrid">
                {plan.candidates.map((candidate) => (
                  <article className="candidateCard" key={candidate.id}>
                    <div className="candidateHeaderRow">
                      <span className="rankBadge">#{candidate.rank}</span>
                      {candidate.rank === 1 ? (
                        <span className="badge badgeNeutral">
                          {copy.recommendedCandidate}
                        </span>
                      ) : null}
                      <span className="candidateScore">
                        {formatSimilarityScore(candidate.sourceSimilarityScore)}
                      </span>
                    </div>
                    <h3>
                      {presetLabel(candidate.preset, locale)} / {candidate.fps}{" "}
                      FPS
                    </h3>
                    <p>{candidate.summary}</p>
                    <p className="candidateIdLine">
                      <span className="metaLabel">{copy.candidateIdLabel}</span>{" "}
                      <code>{candidate.id}</code>
                    </p>
                    <OutputSizeEstimateCard
                      copy={copy}
                      locale={locale}
                      title={copy.outputSizeEstimate}
                      estimate={estimateByCandidateId.get(candidate.id) ?? null}
                      estimateState={estimateState}
                      candidateId={candidate.id}
                      desktopAvailable={desktopAvailable}
                      probeState={probeState}
                      compact
                      onRetryEstimate={onRetryEstimate}
                      onProbeCandidate={onProbeCandidate}
                      onCancelProbe={onCancelProbe}
                    />
                    <div className="candidateMetaGrid">
                      <div>
                        <span className="metaLabel">{copy.contentScale}</span>
                        <strong>{formatScale(candidate.contentScale)}</strong>
                      </div>
                      <div>
                        <span className="metaLabel">{copy.duration}</span>
                        <strong>
                          {formatDuration(candidate.durationSeconds, locale)}
                        </strong>
                      </div>
                      <div>
                        <span className="metaLabel">{copy.sourceMatch}</span>
                        <strong>
                          {formatSimilarityScore(
                            candidate.sourceSimilarityScore,
                          )}
                        </strong>
                      </div>
                    </div>
                  </article>
                ))}
              </div>
            </section>
          ) : null}

          {searchResult ? (
            <section className="detailsCard detailsCardWide">
              <p className="panelLabel">{copy.attemptLog}</p>
              <p className="summaryText">
                {selectionReasonLabel(searchResult.selectionReason, copy)}
              </p>
              <p className="detailText">
                {stopReasonLabel(searchResult.stopReason, copy)}
              </p>
              {searchResult.warnings.length > 0 ? (
                <div className="warningBox">
                  <p className="metaLabel">{copy.warnings}</p>
                  <ul className="warningList">
                    {searchResult.warnings.map((warning) => (
                      <li key={warning}>{warning}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              <div className="attemptList">
                {searchResult.attempts.map((attempt) => {
                  const outputPathLabel =
                    variant === "dock" && attempt.outputPath
                      ? compactPathLabel(attempt.outputPath, 56)
                      : attempt.outputPath;

                  return (
                    <article className="attemptCard" key={attempt.candidateId}>
                      <div className="attemptLead">
                        <div className="candidateHeaderRow">
                          <span className="rankBadge">#{attempt.rank}</span>
                          {attempt.candidateId === selectedResultCandidateId ? (
                            <span className="badge badgeNeutral">
                              {copy.selectedResult}
                            </span>
                          ) : null}
                          <span className={statusClassName(attempt)}>
                            {statusText(attempt, copy)}
                          </span>
                        </div>
                        <h3>{attempt.summary}</h3>
                      </div>
                      <div className="candidateMetaGrid">
                        <div>
                          <span className="metaLabel">{copy.size}</span>
                          <strong>{formatKiB(attempt.sizeBytes)}</strong>
                        </div>
                        <div>
                          <span className="metaLabel">{copy.frameRate}</span>
                          <strong>{attempt.fps}</strong>
                        </div>
                        <div>
                          <span className="metaLabel">{copy.sourceMatch}</span>
                          <strong>
                            {formatSimilarityScore(
                              attempt.sourceSimilarityScore,
                            )}
                          </strong>
                        </div>
                      </div>
                      {attempt.outputPath ? (
                        <div className="attemptPathRow">
                          <code className="pathCode" title={attempt.outputPath}>
                            {outputPathLabel}
                          </code>
                        </div>
                      ) : null}
                      {attempt.errorCode ? (
                        <p>
                          {mediaOperationMessage(
                            locale,
                            attempt.errorCode,
                            attempt.reasonCode,
                          )}
                        </p>
                      ) : null}
                    </article>
                  );
                })}
              </div>
            </section>
          ) : null}
        </div>
      )}
    </section>
  );
}
