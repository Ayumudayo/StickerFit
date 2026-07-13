import type { CSSProperties } from "react";

import {
  mediaOperationMessage,
  type Locale,
  type MessagesForLocale,
} from "../../locales/messages";
import type { VersionedProbeState } from "../../hooks/outputSizeEstimateCoordinator";
import type { VersionedWorkflowState } from "../../hooks/mediaWorkflow/workflowFingerprint";
import type {
  OperationProgress,
  OutputSizeEstimate,
} from "../../types/workflow";
import { formatKiB, formatKiBRange } from "../../utils/formatters";
import {
  classifyEstimate,
  isExactEstimate,
} from "../../utils/outputSizeEstimate";

type EstimateState = VersionedWorkflowState<
  OutputSizeEstimate[],
  OperationProgress
>;

type OutputSizeEstimateCardProps = {
  copy: MessagesForLocale;
  locale: Locale;
  title: string;
  estimate: OutputSizeEstimate | null;
  estimateState: EstimateState;
  candidateId: string | null;
  desktopAvailable: boolean;
  waitingForPlan?: boolean;
  probeState: VersionedProbeState;
  compact?: boolean;
  onRetryEstimate: () => void;
  onProbeCandidate: (candidateId: string) => void;
  onCancelProbe: () => void;
};

type OperationProgressIndicatorProps = {
  label: string;
  message: string;
  progress: OperationProgress | null;
};

function confidenceLabel(
  copy: MessagesForLocale,
  confidence: "low" | "medium" | "high",
) {
  switch (confidence) {
    case "high":
      return copy.estimateConfidenceHigh;
    case "medium":
      return copy.estimateConfidenceMedium;
    case "low":
      return copy.estimateConfidenceLow;
  }
}

function estimateStatusLabel(
  copy: MessagesForLocale,
  estimate: OutputSizeEstimate,
) {
  switch (classifyEstimate(estimate)) {
    case "within":
      return copy.estimateWithin;
    case "over":
      return copy.estimateOver;
    case "likely-within":
      return copy.estimateLikelyWithin;
    case "near-limit":
      return copy.estimateNearLimit;
    case "likely-over":
      return copy.estimateLikelyOver;
  }
}

function estimateSummary(copy: MessagesForLocale, estimate: OutputSizeEstimate) {
  if (isExactEstimate(estimate)) {
    return copy.estimateExactSummary(formatKiB(estimate.bytes));
  }

  return copy.estimateRangeSummary(
    formatKiBRange(estimate.lowerBytes, estimate.upperBytes),
    confidenceLabel(copy, estimate.confidence),
  );
}

export function estimateProgressMessage(
  copy: MessagesForLocale,
  progress: OperationProgress | null,
) {
  switch (progress?.stage) {
    case "queued":
      return copy.estimateProgressQueued;
    case "inspecting":
      return copy.mediaOperationInspecting;
    case "decoding":
      return copy.estimateProgressDecoding;
    case "estimating":
      return copy.estimateProgressEstimating;
    case "encoding":
      return copy.estimateProgressEncoding;
    case "finalizing":
      return copy.estimateProgressFinalizing;
    default:
      return copy.estimateCalculating;
  }
}

export function operationProgressMessage(
  copy: MessagesForLocale,
  progress: OperationProgress | null,
) {
  switch (progress?.stage) {
    case "queued":
      return copy.mediaOperationQueued;
    case "inspecting":
      return copy.mediaOperationInspecting;
    case "decoding":
      return copy.mediaOperationDecoding;
    case "estimating":
      return copy.mediaOperationEstimating;
    case "encoding":
      return copy.mediaOperationEncoding;
    case "finalizing":
      return copy.mediaOperationFinalizing;
    default:
      return copy.runningOptimizer;
  }
}

export function OperationProgressIndicator({
  label,
  message,
  progress,
}: OperationProgressIndicatorProps) {
  const hasKnownTotal =
    progress !== null &&
    progress.total !== null &&
    progress.total > 0;
  const percent = hasKnownTotal
    ? Math.min(100, Math.max(0, (progress.completed / progress.total!) * 100))
    : 0;
  const fillStyle = {
    "--operation-progress": `${percent}%`,
  } as CSSProperties;

  return (
    <div className="operationProgressBlock">
      <span className="detailText" role="status" aria-live="polite" aria-atomic="true">
        {message}
      </span>
      <div
        className={
          hasKnownTotal
            ? "operationProgressTrack"
            : "operationProgressTrack operationProgressTrackIndeterminate"
        }
        role="progressbar"
        aria-label={label}
        aria-valuemin={hasKnownTotal ? 0 : undefined}
        aria-valuenow={hasKnownTotal ? progress.completed : undefined}
        aria-valuemax={hasKnownTotal ? progress.total! : undefined}
        aria-valuetext={hasKnownTotal ? undefined : message}
      >
        <span className="operationProgressFill" style={fillStyle} />
      </div>
    </div>
  );
}

export function OutputSizeEstimateCard({
  copy,
  locale,
  title,
  estimate,
  estimateState,
  candidateId,
  desktopAvailable,
  waitingForPlan = false,
  probeState,
  compact = false,
  onRetryEstimate,
  onProbeCandidate,
  onCancelProbe,
}: OutputSizeEstimateCardProps) {
  const activeProbe =
    candidateId !== null &&
    probeState.status !== "idle" &&
    probeState.candidateId === candidateId
      ? probeState
      : null;
  const canProbe =
    candidateId !== null &&
    estimate?.kind === "range" &&
    classifyEstimate(estimate) === "near-limit";
  const estimateError = estimateState.status === "error"
    ? mediaOperationMessage(
        locale,
        estimateState.code,
        estimateState.reasonCode,
      ) ?? copy.estimateCalculating
    : null;
  const probeError = activeProbe?.status === "error"
    ? mediaOperationMessage(
        locale,
        activeProbe.code,
        activeProbe.reasonCode,
      ) ?? copy.estimateCalculating
    : null;
  const hasNestedLiveRegion =
    estimateState.status === "loading" ||
    activeProbe?.status === "loading" ||
    activeProbe?.status === "error";
  const cardRole = estimateError
    ? "alert"
    : hasNestedLiveRegion
      ? "group"
      : "status";

  if (compact && !estimate && activeProbe === null) {
    return null;
  }

  return (
    <section
      className={compact ? "outputEstimateCard outputEstimateCardCompact" : "outputEstimateCard"}
      role={cardRole}
      aria-live={
        cardRole === "alert"
          ? "assertive"
          : cardRole === "status"
            ? "polite"
            : undefined
      }
      aria-atomic={cardRole === "status" ? "false" : undefined}
    >
      <div className="outputEstimateHeading">
        <span className="metaLabel">{title}</span>
        {estimate && isExactEstimate(estimate) ? (
          <span className="badge badgeOk">{copy.estimateExactLabel}</span>
        ) : null}
      </div>

      {!desktopAvailable ? (
        <p className="summaryText">{copy.estimateDesktopOnly}</p>
      ) : estimateError && !estimate ? (
        <div className="outputEstimateStateActions">
          <p className="errorText">{estimateError}</p>
          <button className="subtleAction" type="button" onClick={onRetryEstimate}>
            {copy.estimateRetry}
          </button>
        </div>
      ) : estimateState.status === "loading" && !estimate ? (
        <OperationProgressIndicator
          label={copy.outputSizeEstimate}
          message={estimateProgressMessage(copy, estimateState.progress)}
          progress={estimateState.progress}
        />
      ) : estimateState.status === "cancelled" && !estimate ? (
        <p className="summaryText">{copy.estimateCancelled}</p>
      ) : estimate ? (
        <>
          <strong className="outputEstimateSummary">{estimateSummary(copy, estimate)}</strong>
          <p className="detailText">
            {estimateStatusLabel(copy, estimate)}
            {isExactEstimate(estimate) ? ` · ${copy.estimateCompressionBasis}` : ""}
          </p>
        </>
      ) : waitingForPlan ? (
        <p className="summaryText">{copy.estimateWaitingForPlan}</p>
      ) : (
        <p className="summaryText">{copy.estimateCalculating}</p>
      )}

      {activeProbe?.status === "loading" ? (
        <div className="outputEstimateProbe">
          <OperationProgressIndicator
            label={copy.checkExactCandidateSize}
            message={estimateProgressMessage(copy, activeProbe.progress)}
            progress={activeProbe.progress}
          />
          <p className="detailText">{copy.exactProbeNoOutput}</p>
          <button className="subtleAction" type="button" onClick={onCancelProbe}>
            {copy.cancelExactProbe}
          </button>
        </div>
      ) : activeProbe?.status === "error" ? (
        <div className="outputEstimateProbe" role="alert">
          <p className="errorText">{probeError}</p>
          <p className="detailText">{copy.exactProbeNoOutput}</p>
          <button
            className="subtleAction"
            type="button"
            onClick={() => candidateId && onProbeCandidate(candidateId)}
          >
            {copy.checkExactCandidateSize}
          </button>
        </div>
      ) : activeProbe?.status === "cancelled" ? (
        <div className="outputEstimateProbe">
          <p className="detailText">{copy.estimateCancelled}</p>
          <button
            className="subtleAction"
            type="button"
            onClick={() => candidateId && onProbeCandidate(candidateId)}
          >
            {copy.checkExactCandidateSize}
          </button>
        </div>
      ) : canProbe ? (
        <div className="outputEstimateProbe">
          <p className="detailText">{copy.exactProbeNoOutput}</p>
          <button
            className="secondaryAction"
            type="button"
            onClick={() => candidateId && onProbeCandidate(candidateId)}
          >
            {copy.checkExactCandidateSize}
          </button>
        </div>
      ) : null}
    </section>
  );
}
