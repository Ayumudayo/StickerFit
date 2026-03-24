import { describe, expect, it } from "vitest";

import { MESSAGES } from "../locales/messages";
import {
  formatSimilarityScore,
  selectionReasonLabel,
} from "./formatters";

describe("formatters", () => {
  it("formats source similarity as a percentage", () => {
    expect(formatSimilarityScore(0.934)).toBe("93%");
    expect(formatSimilarityScore(null)).toBe("-");
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
});
