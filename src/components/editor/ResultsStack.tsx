import type { Locale, MessagesForLocale } from "../../locales/messages";
import type {
  OptimizerSearchResponse,
  StaticImageConversionResult,
} from "../../types/workflow";
import {
  formatDuration,
  formatKiB,
  formatSimilarityScore,
  selectionReasonLabel,
} from "../../utils/formatters";
import { compactPathLabel } from "../../utils/pathLabels";

type ResultsStackProps = {
  copy: MessagesForLocale;
  locale: Locale;
  searchResult: OptimizerSearchResponse | null;
  conversionResult: StaticImageConversionResult | null;
  onOpenOutputFolder: (path?: string | null) => void;
  variant?: "page" | "dock";
};

export function ResultsStack({
  copy,
  locale,
  searchResult,
  conversionResult,
  onOpenOutputFolder,
  variant = "page",
}: ResultsStackProps) {
  if (!searchResult && !conversionResult) {
    return null;
  }

  const selectedCandidateId = searchResult
    ? searchResult.winningCandidateId ?? searchResult.closestCandidateId
    : null;
  const selectedAttempt = selectedCandidateId && searchResult
    ? searchResult.attempts.find((attempt) => attempt.candidateId === selectedCandidateId) ?? null
    : null;
  const bestOutputPathLabel =
    variant === "dock" && searchResult?.bestOutputPath
      ? compactPathLabel(searchResult.bestOutputPath, 56)
      : searchResult?.bestOutputPath ?? null;
  const conversionOutputPathLabel =
    variant === "dock" && conversionResult?.outputPath
      ? compactPathLabel(conversionResult.outputPath, 56)
      : conversionResult?.outputPath ?? null;

  return (
    <section className="bottomPanelsStack" aria-live="polite">
      {searchResult ? (
        <section className="figmaCandidatesSection resultSummarySection">
          {variant === "page" ? (
            <div className="figmaSectionHeader">
              <div>
                <h2>{copy.searchSummary}</h2>
                <p className="summaryText">{searchResult.summary}</p>
              </div>
            </div>
          ) : (
            <p className="summaryText">{searchResult.summary}</p>
          )}

          <section className={searchResult.ok ? "resultCard resultOk resultCardInline" : "resultCard resultError resultCardInline"}>
            <p className="detailText resultSelectionReason">
              <span className="metaLabel">{copy.selectionBasis}</span>
              <span>{selectionReasonLabel(searchResult.selectionReason, copy)}</span>
            </p>
            {selectedAttempt ? (
              <p className="summaryText">{selectedAttempt.summary}</p>
            ) : null}
            <div className="resultMetaGrid">
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
                <strong>{formatSimilarityScore(selectedAttempt?.sourceSimilarityScore ?? null)}</strong>
              </article>
            </div>
            {searchResult.bestOutputPath ? (
              <div className="outputPathRow">
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
          </section>
        </section>
      ) : null}

      {conversionResult ? (
        <section className="figmaCandidatesSection resultSummarySection">
          {variant === "page" ? (
            <div className="figmaSectionHeader">
              <div>
                <h2>{copy.pngConversion}</h2>
                <p className="summaryText">
                  {conversionResult.ok ? copy.pngCreated : copy.pngConversionFailed}
                </p>
              </div>
            </div>
          ) : (
            <p className="summaryText">
              {conversionResult.ok ? copy.pngCreated : copy.pngConversionFailed}
            </p>
          )}

          <section className={conversionResult.ok ? "resultCard resultOk resultCardInline" : "resultCard resultError resultCardInline"}>
            {conversionResult.outputPath ? (
              <div className="outputPathRow">
                <code className="pathCode" title={conversionResult.outputPath}>
                  {conversionOutputPathLabel}
                </code>
                <button
                  className="secondaryAction"
                  type="button"
                  onClick={() => onOpenOutputFolder(conversionResult.outputPath)}
                >
                  {copy.openOutputFolder}
                </button>
              </div>
            ) : null}
          </section>
        </section>
      ) : null}
    </section>
  );
}
