import { describe, expect, it } from "vitest";

import { compactPathLabel } from "./pathLabels";

describe("compactPathLabel", () => {
  it("keeps short paths unchanged", () => {
    expect(compactPathLabel("C:\\tmp\\sticker.png", 28)).toBe("C:\\tmp\\sticker.png");
  });

  it("compresses long paths around the extension", () => {
    const compactLabel = compactPathLabel(
      "E:\\very\\long\\folder\\structure\\sticker-preview-name.png",
      28,
    );

    expect(compactLabel).toContain("...");
    expect(compactLabel.endsWith("name.png")).toBe(true);
    expect(compactLabel.length).toBeLessThanOrEqual(28);
  });
});
