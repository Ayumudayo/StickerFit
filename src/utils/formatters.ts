import type { Locale } from "../locales/messages";

type FitModeCopy = {
  cover: string;
  contain: string;
  fill: string;
};

type StopReasonCopy = {
  statusFirstFit: string;
  statusExhausted: string;
  statusNoOutput: string;
  statusPlanInvalid: string;
  statusInvokeFailed: string;
};

type AttemptStatusCopy = {
  skipped: string;
  fits: string;
  over: string;
  failed: string;
};

type SelectionReasonCopy = {
  selectionReasonBestWithinLimit: string;
  selectionReasonSmallestOversize: string;
  selectionReasonNoFitFound: string;
};

type AttemptStatusFields = {
  skipped: boolean;
  withinLimit: boolean;
  sizeBytes: number | null;
};

export function formatDuration(value: number | null, locale: Locale) {
  if (value === null) {
    return "-";
  }

  return locale === "ko" ? `${value.toFixed(2)}초` : `${value.toFixed(2)} s`;
}

export function formatSize(value: number | null) {
  if (value === null) {
    return "-";
  }

  return `${(value / 1024 / 1024).toFixed(2)} MB`;
}

export function formatFps(value: number | null, raw: string | null) {
  if (value === null && raw === null) {
    return "-";
  }

  if (value === null) {
    return raw ?? "-";
  }

  return raw ? `${value.toFixed(2)} (${raw})` : value.toFixed(2);
}

export function formatScale(value: number) {
  return `${Math.round(value * 100)}%`;
}

export function formatKiB(value: number | null) {
  if (value === null) {
    return "-";
  }

  return `${(value / 1024).toFixed(1)} KiB`;
}

export function formatSimilarityScore(value: number | null) {
  if (value === null) {
    return "-";
  }

  return `${Math.round(value * 100)}%`;
}

export function inputMediaLabel(inputPath: string | null, fallback: string) {
  if (!inputPath) {
    return fallback;
  }

  const lastDot = inputPath.lastIndexOf(".");
  const lastSlash = Math.max(inputPath.lastIndexOf("\\"), inputPath.lastIndexOf("/"));

  if (lastDot < 0 || lastDot < lastSlash) {
    return fallback;
  }

  return inputPath.slice(lastDot + 1).toLowerCase() || fallback;
}

export function presetLabel(preset: string, locale: Locale) {
  const labels = {
    standard: locale === "ko" ? "표준" : "Standard",
    compact: locale === "ko" ? "압축" : "Compact",
    compactPlus: locale === "ko" ? "압축+" : "Compact+",
  } as const;

  return labels[preset as keyof typeof labels] ?? preset;
}

export function fitModeLabel(fitMode: string, copy: FitModeCopy) {
  if (fitMode === "cover") {
    return copy.cover;
  }

  if (fitMode === "fill") {
    return copy.fill;
  }

  return copy.contain;
}

export function stopReasonLabel(reason: string | null, copy: StopReasonCopy) {
  switch (reason) {
    case "first-fit-within-limit":
      return copy.statusFirstFit;
    case "exhausted-ranked-candidates":
      return copy.statusExhausted;
    case "no-successful-encodes":
      return copy.statusNoOutput;
    case "plan-invalid":
      return copy.statusPlanInvalid;
    case "invoke-failed":
      return copy.statusInvokeFailed;
    case "internal-task-failed":
      return copy.statusInvokeFailed;
    default:
      return reason ?? "-";
  }
}

export function selectionReasonLabel(reason: string | null, copy: SelectionReasonCopy) {
  switch (reason) {
    case "best_within_limit":
      return copy.selectionReasonBestWithinLimit;
    case "smallest_oversize":
      return copy.selectionReasonSmallestOversize;
    case "no_fit_found":
      return copy.selectionReasonNoFitFound;
    default:
      return copy.selectionReasonNoFitFound;
  }
}

export function statusText(attempt: AttemptStatusFields, copy: AttemptStatusCopy) {
  if (attempt.skipped) {
    return copy.skipped;
  }

  if (attempt.withinLimit) {
    return copy.fits;
  }

  if (attempt.sizeBytes !== null) {
    return copy.over;
  }

  return copy.failed;
}

export function statusClassName(attempt: AttemptStatusFields) {
  if (attempt.skipped) {
    return "badge badgeNeutral";
  }

  if (attempt.withinLimit) {
    return "badge badgeOk";
  }

  return "badge badgeWarn";
}

