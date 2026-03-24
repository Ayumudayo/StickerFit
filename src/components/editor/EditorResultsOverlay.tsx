import type { Locale, MessagesForLocale } from "../../locales/messages";
import type { OptimizerSearchResponse } from "../../types/workflow";
import {
  formatDuration,
  formatKiB,
  formatSimilarityScore,
  selectionReasonLabel,
  statusClassName,
  statusText,
  stopReasonLabel,
} from "../../utils/formatters";
import { compactPathLabel } from "../../utils/pathLabels";

type EditorResultsOverlayProps = {
  copy: MessagesForLocale;
  locale: Locale;
  searchResult: OptimizerSearchResponse | null;
  fitModeLabel: (value: string) => string;
  onOpenOutputFolder: (path?: string | null) => void;
};

export function EditorResultsOverlay({
  copy,
  locale,
  searchResult,
  fitModeLabel,
  onOpenOutputFolder,
}: EditorResultsOverlayProps) {
  if (!searchResult) {
    return (
      <section className="editorResultsOverlay">
        <section className="editorResultsSection">
          <p className="summaryText">{copy.noAttemptsYet}</p>
        </section>
      </section>
    );
  }

  const selectedCandidateId = searchResult.winningCandidateId ?? searchResult.closestCandidateId;
  const selectedAttempt = selectedCandidateId
    ? searchResult.attempts.find((attempt) => attempt.candidateId === selectedCandidateId) ?? null
    : null;
  const bestOutputPathLabel = searchResult.bestOutputPath
    ? compactPathLabel(searchResult.bestOutputPath, 72)
    : null;

  return (
    <section className="editorResultsOverlay" aria-live="polite">
      <p className="summaryText">{searchResult.summary}</p>

      {selectedAttempt ? (
        <section className="editorResultsSection">
          <p className="panelLabel">{copy.selectionBasis}</p>
          <article className="editorResultsCard editorResultsCardSelected">
            <p className="detailText">{selectionReasonLabel(searchResult.selectionReason, copy)}</p>
            <p className="summaryText">{selectedAttempt.summary}</p>

            <div className="editorResultsMetrics">
              <article className="metricPill">
                <span className="metaLabel">{copy.bestOutput}</span>
                <strong>{searchResult.bestWithinLimit ? copy.fits : copy.over}</strong>
              </article>
              <article className="metricPill">
                <span className="metaLabel">{copy.size}</span>
                <strong>{formatKiB(searchResult.bestSizeBytes)}</strong>
              </article>
              <article className="metricPill">
                <span className="metaLabel">{copy.duration}</span>
                <strong>{formatDuration(searchResult.selectedDurationSeconds, locale)}</strong>
              </article>
              <article className="metricPill">
                <span className="metaLabel">{copy.sourceMatch}</span>
                <strong>{formatSimilarityScore(selectedAttempt.sourceSimilarityScore)}</strong>
              </article>
            </div>

            {searchResult.bestOutputPath ? (
              <div className="editorResultsPathRow">
                <code className="pathCode" title={searchResult.bestOutputPath}>
                  {bestOutputPathLabel}
                </code>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={() => onOpenOutputFolder(searchResult.bestOutputPath)}
                >
                  {copy.openOutputFolder}
                </button>
              </div>
            ) : null}
          </article>
        </section>
      ) : null}

      <section className="editorResultsSection">
        <p className="panelLabel">{copy.attemptLog}</p>
        <p className="summaryText">{selectionReasonLabel(searchResult.selectionReason, copy)}</p>
        <p className="detailText">{stopReasonLabel(searchResult.stopReason, copy)}</p>

        <div className="editorResultsAttempts">
          {searchResult.attempts.map((attempt) => {
            const outputPathLabel = attempt.outputPath
              ? compactPathLabel(attempt.outputPath, 72)
              : null;

            return (
              <article className="editorResultsAttemptCard" key={attempt.candidateId}>
                <div className="editorResultsAttemptHeader">
                  <span className="rankBadge">#{attempt.rank}</span>
                  {attempt.candidateId === selectedCandidateId ? (
                    <span className="badge badgeNeutral">{copy.selectedResult}</span>
                  ) : null}
                  <span className={statusClassName(attempt)}>{statusText(attempt, copy)}</span>
                </div>

                <h3>{attempt.summary}</h3>

                <div className="editorResultsAttemptMeta">
                  <div>
                    <span className="metaLabel">{copy.size}</span>
                    <strong>{formatKiB(attempt.sizeBytes)}</strong>
                  </div>
                  <div>
                    <span className="metaLabel">{copy.fit}</span>
                    <strong>{fitModeLabel(attempt.fitMode)}</strong>
                  </div>
                  <div>
                    <span className="metaLabel">{copy.frameRate}</span>
                    <strong>{attempt.fps}</strong>
                  </div>
                  <div>
                    <span className="metaLabel">{copy.sourceMatch}</span>
                    <strong>{formatSimilarityScore(attempt.sourceSimilarityScore)}</strong>
                  </div>
                </div>

                {attempt.outputPath ? (
                  <div className="editorResultsAttemptPathRow">
                    <code className="pathCode" title={attempt.outputPath}>
                      {outputPathLabel}
                    </code>
                  </div>
                ) : null}

                {attempt.errorMessage ? <p className="detailText">{attempt.errorMessage}</p> : null}
              </article>
            );
          })}
        </div>
      </section>
    </section>
  );
}
