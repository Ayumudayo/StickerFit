import type {
  ExactCandidateSizeEstimate,
  ExactStaticSizeEstimate,
  OutputSizeEstimate,
} from "../types/workflow";

export type EstimateLimitStatus =
  | "within"
  | "over"
  | "likely-within"
  | "near-limit"
  | "likely-over";

export type ExactOutputSizeEstimate =
  | ExactStaticSizeEstimate
  | ExactCandidateSizeEstimate;

export function isExactEstimate(
  estimate: OutputSizeEstimate,
): estimate is ExactOutputSizeEstimate {
  return estimate.kind !== "range";
}

export function classifyEstimate(
  estimate: OutputSizeEstimate,
): EstimateLimitStatus {
  if (isExactEstimate(estimate)) {
    return estimate.bytes <= estimate.limitBytes ? "within" : "over";
  }

  if (estimate.upperBytes <= estimate.limitBytes) {
    return "likely-within";
  }

  if (estimate.lowerBytes > estimate.limitBytes) {
    return "likely-over";
  }

  return "near-limit";
}

export type OptimizerStopReason =
  | "best-ranked-within-limit"
  | "budget-exhausted"
  | "cancelled"
  | "failed";

export function normalizeOptimizerStopReason(
  reason: string | null,
): OptimizerStopReason {
  switch (reason) {
    case "best-ranked-within-limit":
    case "found-best-ranked-within-limit":
    case "first-fit-within-limit":
      return "best-ranked-within-limit";
    case "budget-exhausted":
    case "exhausted-ranked-candidates":
      return "budget-exhausted";
    case "cancelled":
      return "cancelled";
    default:
      return "failed";
  }
}
