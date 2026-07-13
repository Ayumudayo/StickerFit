import { describe, expect, it } from "vitest";

import { MESSAGES } from "../locales/messages";
import {
  formatElapsedTime,
  formatKiB,
  formatKiBRange,
  formatSimilarityScore,
  selectionReasonLabel,
  stopReasonLabel,
} from "./formatters";

describe("formatters", () => {
  it("formats source similarity as a percentage", () => {
    expect(formatSimilarityScore(0.934)).toBe("93%");
    expect(formatSimilarityScore(null)).toBe("-");
  });

  it("formats exact byte counts in KiB at the 512 KiB boundary", () => {
    expect(formatKiB(512 * 1024)).toBe("512.0 KiB");
    expect(formatKiB(null)).toBe("-");
  });

  it("formats sampled byte ranges with one shared KiB unit", () => {
    expect(formatKiBRange(380 * 1024, 470 * 1024)).toBe(
      "380.0–470.0 KiB",
    );
    expect(formatKiBRange(null, 470 * 1024)).toBe("-");
    expect(formatKiBRange(380 * 1024, null)).toBe("-");
  });

  it("formats elapsed milliseconds as localized seconds", () => {
    expect(formatElapsedTime(1250, "en")).toBe("1.25 s");
    expect(formatElapsedTime(1250, "ko")).toBe("1.25초");
    expect(formatElapsedTime(null, "en")).toBe("-");
  });

  it("maps selection reasons to localized copy", () => {
    expect(selectionReasonLabel("best_within_limit", MESSAGES.en)).toBe(
      MESSAGES.en.selectionReasonBestWithinLimit,
    );
    expect(selectionReasonLabel("smallest_oversize", MESSAGES.ko)).toBe(
      MESSAGES.ko.selectionReasonSmallestOversize,
    );
    expect(selectionReasonLabel(null, MESSAGES.en)).toBe(
      MESSAGES.en.selectionReasonNoFitFound,
    );
  });

  it("uses normalized truthful stop-reason copy", () => {
    expect(
      stopReasonLabel("found-best-ranked-within-limit", MESSAGES.en),
    ).toBe(MESSAGES.en.statusBestRanked);
    expect(stopReasonLabel("exhausted-ranked-candidates", MESSAGES.ko)).toBe(
      MESSAGES.ko.statusExhausted,
    );
    expect(stopReasonLabel("cancelled", MESSAGES.en)).toBe(
      MESSAGES.en.statusCancelled,
    );
    expect(stopReasonLabel("plan-invalid", MESSAGES.ko)).toBe(
      MESSAGES.ko.statusFailed,
    );
  });
});
