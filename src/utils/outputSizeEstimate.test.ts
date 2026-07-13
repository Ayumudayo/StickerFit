import { describe, expect, it } from "vitest";

import { MESSAGES } from "../locales/messages";
import type { OutputSizeEstimate } from "../types/workflow";
import {
  classifyEstimate,
  isExactEstimate,
  normalizeOptimizerStopReason,
} from "./outputSizeEstimate";

const LIMIT_BYTES = 512 * 1024;

function exactEstimate(bytes: number): OutputSizeEstimate {
  return {
    kind: "exact-static",
    basis: "exact-static",
    bytes,
    candidateId: null,
    limitBytes: LIMIT_BYTES,
    outputFrameCount: 1,
  };
}

function sampledEstimate(
  lowerBytes: number,
  upperBytes: number,
): OutputSizeEstimate {
  return {
    kind: "range",
    basis: "sampled",
    lowerBytes,
    predictedBytes: Math.round((lowerBytes + upperBytes) / 2),
    upperBytes,
    confidence: "high",
    candidateId: "candidate-1",
    limitBytes: LIMIT_BYTES,
    measuredContributionCount: 12,
    outputFrameCount: 24,
  };
}

describe("output-size estimate classification", () => {
  it("classifies exact estimates under, at, and over the byte limit", () => {
    expect(classifyEstimate(exactEstimate(LIMIT_BYTES - 1))).toBe("within");
    expect(classifyEstimate(exactEstimate(LIMIT_BYTES))).toBe("within");
    expect(classifyEstimate(exactEstimate(LIMIT_BYTES + 1))).toBe("over");
  });

  it("classifies a sampled range entirely below the limit as likely within", () => {
    expect(
      classifyEstimate(sampledEstimate(LIMIT_BYTES - 2048, LIMIT_BYTES)),
    ).toBe("likely-within");
  });

  it("classifies a sampled range crossing the limit as near the limit", () => {
    expect(
      classifyEstimate(sampledEstimate(LIMIT_BYTES - 1, LIMIT_BYTES + 1)),
    ).toBe("near-limit");
  });

  it("classifies a sampled range entirely above the limit as likely over", () => {
    expect(
      classifyEstimate(sampledEstimate(LIMIT_BYTES + 1, LIMIT_BYTES + 2048)),
    ).toBe("likely-over");
  });

  it("distinguishes exact/probe values from sampled ranges", () => {
    expect(isExactEstimate(exactEstimate(LIMIT_BYTES))).toBe(true);
    expect(
      isExactEstimate(sampledEstimate(LIMIT_BYTES - 1, LIMIT_BYTES + 1)),
    ).toBe(false);
  });
});

describe("optimizer stop-reason normalization", () => {
  it("maps the backend best-ranked result to truthful UI copy", () => {
    expect(
      normalizeOptimizerStopReason("found-best-ranked-within-limit"),
    ).toBe("best-ranked-within-limit");
    expect(normalizeOptimizerStopReason("first-fit-within-limit")).toBe(
      "best-ranked-within-limit",
    );
  });

  it("normalizes exhausted, cancelled, and failure reasons", () => {
    expect(normalizeOptimizerStopReason("exhausted-ranked-candidates")).toBe(
      "budget-exhausted",
    );
    expect(normalizeOptimizerStopReason("cancelled")).toBe("cancelled");
    expect(normalizeOptimizerStopReason("no-successful-encodes")).toBe(
      "failed",
    );
    expect(normalizeOptimizerStopReason(null)).toBe("failed");
  });
});

describe("output-size locale parity", () => {
  it.each(["en", "ko"] as const)(
    "provides non-empty %s estimate, probe, progress, and result copy",
    (locale) => {
      const copy = MESSAGES[locale];
      const keys = [
        "outputSizeEstimate",
        "recommendedCandidateEstimate",
        "estimateDesktopOnly",
        "estimateWaitingForPlan",
        "estimateCalculating",
        "estimateRetry",
        "estimateExactLabel",
        "estimateConfidenceHigh",
        "estimateConfidenceMedium",
        "estimateConfidenceLow",
        "estimateWithin",
        "estimateOver",
        "estimateLikelyWithin",
        "estimateNearLimit",
        "estimateLikelyOver",
        "estimateCompressionBasis",
        "checkExactCandidateSize",
        "checkingExactCandidateSize",
        "cancelExactProbe",
        "exactProbeNoOutput",
        "estimateCancelled",
        "operationCancelled",
        "estimateSettingsHint",
        "actualOutputSize",
        "elapsedTime",
        "representativeError",
        "candidateIdLabel",
        "statusBestRanked",
        "statusExhausted",
        "statusCancelled",
        "statusFailed",
        "estimateProgressQueued",
        "estimateProgressDecoding",
        "estimateProgressEstimating",
        "estimateProgressEncoding",
        "estimateProgressFinalizing",
      ] as const;

      for (const key of keys) {
        expect(copy[key].trim(), key).not.toBe("");
      }
      expect(copy.estimateExactSummary("512.0 KiB").trim()).not.toBe("");
      expect(
        copy.estimateRangeSummary("380.0–470.0 KiB", copy.estimateConfidenceHigh).trim(),
      ).not.toBe("");
    },
  );
});
