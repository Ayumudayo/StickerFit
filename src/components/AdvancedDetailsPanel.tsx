import type { Locale, MessagesForLocale } from "../locales/messages";
import type {
	OptimizerPlanResponse,
	OptimizerSearchResponse,
} from "../types/workflow";
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

type AdvancedDetailsPanelProps = {
	copy: MessagesForLocale;
	locale: Locale;
	plan: OptimizerPlanResponse | null;
	searchResult: OptimizerSearchResponse | null;
	fitModeLabel: (fitMode: string) => string;
	variant?: "page" | "dock";
};

export function AdvancedDetailsPanel({
	copy,
	locale,
	plan,
	searchResult,
	fitModeLabel,
	variant = "page",
}: AdvancedDetailsPanelProps) {
	const hasAdvancedContent = Boolean(plan) || Boolean(searchResult);
	const selectedResultCandidateId =
		searchResult?.winningCandidateId ?? searchResult?.closestCandidateId ?? null;
	const panelClassName =
		variant === "dock" ? "detailsPanel detailsPanelDock" : "detailsPanel";

	return (
		<section className={panelClassName}>
			{variant === "page" ? <p className="panelLabel">{copy.advancedDetails}</p> : null}
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
										<div className="candidateMetaGrid">
											<div>
												<span className="metaLabel">{copy.contentScale}</span>
												<strong>{formatScale(candidate.contentScale)}</strong>
											</div>
											<div>
												<span className="metaLabel">{copy.fit}</span>
												<strong>{fitModeLabel(candidate.fitMode)}</strong>
											</div>
											<div>
												<span className="metaLabel">{copy.duration}</span>
												<strong>
													{formatDuration(candidate.durationSeconds, locale)}
												</strong>
											</div>
											<div>
												<span className="metaLabel">{copy.sourceMatch}</span>
												<strong>{formatSimilarityScore(candidate.sourceSimilarityScore)}</strong>
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
											<div className="attemptPathRow">
												<code className="pathCode" title={attempt.outputPath}>
													{outputPathLabel}
												</code>
											</div>
										) : null}
										{attempt.errorMessage ? (
											<p>{attempt.errorMessage}</p>
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
