import { describe, expect, it } from "vitest";

import { MESSAGES } from "../locales/messages";
import {
  FULL_CROP_REGION,
  clampPreviewZoomScale,
  previewKindForPath,
  selectionSummary,
} from "./mediaSelectionMath";

describe("mediaSelectionMath", () => {
  it("clamps preview zoom scale to the supported range", () => {
    expect(clampPreviewZoomScale(0)).toBe(0.001);
    expect(clampPreviewZoomScale(1.5)).toBe(1.5);
    expect(clampPreviewZoomScale(7)).toBe(4);
  });

  it("summarizes full-frame and cropped selections", () => {
    expect(selectionSummary(FULL_CROP_REGION, MESSAGES.en)).toBe("Full frame");
    expect(
      selectionSummary({ x: 0.1, y: 0.2, width: 0.5, height: 0.75 }, MESSAGES.en),
    ).toBe("50% x 75%");
    expect(selectionSummary(FULL_CROP_REGION, MESSAGES.en, 200, 200)).toBe("Full frame");
    expect(
      selectionSummary(
        { x: 0.1, y: 0.2, width: 0.5, height: 0.75 },
        MESSAGES.en,
        200,
        200,
      ),
    ).toBe("100 x 150 px");
  });

  it("infers preview kind from the input path", () => {
    expect(previewKindForPath("sample.mp4")).toBe("video");
    expect(previewKindForPath("sample.PNG")).toBe("image");
    expect(previewKindForPath(null)).toBe("image");
  });
});
