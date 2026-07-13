import { describe, expect, it } from "vitest";

import { MEDIA_OPERATION_MESSAGES } from "../../locales/messages";
import { canonicalToolHealthFailureMessage } from "./useToolHealthReport";

describe("canonicalToolHealthFailureMessage", () => {
  it("uses the initial locale and never exposes rejection detail", () => {
    const hostileRejections: unknown[] = [
      new Error("C:\\Users\\private\\ffmpeg.exe failed\u001b[31m"),
      "transport rejected: /home/private/input.mp4",
      { detail: "x".repeat(2_000_000) },
      new Proxy(
        {},
        {
          get() {
            throw new Error("must not inspect rejection values");
          },
        },
      ),
    ];

    for (const rejection of hostileRejections) {
      expect(canonicalToolHealthFailureMessage("en", rejection)).toBe(
        MEDIA_OPERATION_MESSAGES.en.errorCodes["internal-task-failed"],
      );
      expect(canonicalToolHealthFailureMessage("ko", rejection)).toBe(
        MEDIA_OPERATION_MESSAGES.ko.errorCodes["internal-task-failed"],
      );
    }
  });
});
