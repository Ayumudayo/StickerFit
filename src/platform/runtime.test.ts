import { describe, expect, it } from "vitest";

import mediaOperationCodes from "../types/media-operation-error-codes.json";
import type {
  MediaOperationErrorCode,
  MediaOperationReasonCode,
} from "../types/workflow";
import {
  MEDIA_OPERATION_MESSAGES,
  mediaOperationMessage,
} from "../locales/messages";
import {
  buildWebFileSourceRevision,
  normalizeLegacyMediaError,
  normalizeLegacyMediaResponse,
  normalizeLegacyOptimizerSearchResponse,
} from "./runtime";

function sorted(values: readonly string[]) {
  return [...values].sort();
}

describe("canonical media operation errors", () => {
  it("keeps the JSON fixture and every locale table exhaustive in both directions", () => {
    for (const locale of ["en", "ko"] as const) {
      expect(sorted(Object.keys(MEDIA_OPERATION_MESSAGES[locale].errorCodes))).toEqual(
        sorted(mediaOperationCodes.errorCodes),
      );
      expect(sorted(Object.keys(MEDIA_OPERATION_MESSAGES[locale].reasonCodes))).toEqual(
        sorted(mediaOperationCodes.reasonCodes),
      );
    }
  });

  it("passes every canonical category through unchanged", () => {
    for (const errorCode of mediaOperationCodes.errorCodes) {
      const normalized = normalizeLegacyMediaError({
        errorCode,
        reasonCode: null,
        errorMessage: "diagnostic detail",
      });

      expect(normalized).toEqual({
        errorCode: errorCode as MediaOperationErrorCode,
        reasonCode: null,
        diagnostics: "diagnostic detail",
      });
    }
  });

  it("folds every legacy domain code into invalid-request with the same reason", () => {
    for (const reasonCode of mediaOperationCodes.reasonCodes) {
      const normalized = normalizeLegacyMediaError(reasonCode);

      expect(normalized).toEqual({
        errorCode: "invalid-request",
        reasonCode: reasonCode as MediaOperationReasonCode,
        diagnostics: null,
      });
    }
  });

  it("normalizes the browser inspection legacy code", () => {
    expect(normalizeLegacyMediaError("browser_inspection_failed")).toEqual({
      errorCode: "malformed-media",
      reasonCode: "decode-failed",
      diagnostics: null,
    });
  });

  it("normalizes known desktop inspection legacy codes", () => {
    expect(normalizeLegacyMediaError("inspect-failed")).toEqual({
      errorCode: "malformed-media",
      reasonCode: "decode-failed",
      diagnostics: null,
    });
    expect(normalizeLegacyMediaError("tool-unavailable")).toEqual({
      errorCode: "tool-missing",
      reasonCode: null,
      diagnostics: null,
    });
  });

  it("keeps unknown raw values in diagnostics while using the internal category", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: "future-backend-code",
        errorMessage: "backend diagnostic",
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: "backend diagnostic (legacy code: future-backend-code)",
    });
  });

  it("does not let a known reason hide an unknown non-null error code", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: "future-backend-code",
        reasonCode: "decode-failed",
        errorMessage: "detail",
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: "detail (legacy code: future-backend-code)",
    });
  });

  it("preserves an unknown non-null error code when the reason is also unknown", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: "future-backend-code",
        reasonCode: "future-reason",
        errorMessage: "detail",
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics:
        "detail (legacy code: future-backend-code) (legacy reason code: future-reason)",
    });
  });

  it("does not silently accept an unknown reason without an error category", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: null,
        reasonCode: "future-reason",
        errorMessage: null,
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: "(legacy reason code: future-reason)",
    });
  });

  it("does not silently drop an unknown reason beside a canonical category", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: "invalid-request",
        reasonCode: "future-reason",
        errorMessage: "backend diagnostic",
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: "backend diagnostic (legacy reason code: future-reason)",
    });
  });

  it("uses reason translations ahead of category translations", () => {
    expect(
      mediaOperationMessage("en", "invalid-request", "invalid-crop"),
    ).toBe(MEDIA_OPERATION_MESSAGES.en.reasonCodes["invalid-crop"]);
    expect(mediaOperationMessage("ko", "tool-missing", null)).toBe(
      MEDIA_OPERATION_MESSAGES.ko.errorCodes["tool-missing"],
    );
  });

  it("normalizes resolved legacy response fields without replacing diagnostics", () => {
    expect(
      normalizeLegacyMediaResponse({
        ok: false,
        errorCode: "invalid-crop",
        errorMessage: "legacy crop details",
      }),
    ).toEqual({
      ok: false,
      errorCode: "invalid-request",
      reasonCode: "invalid-crop",
      errorMessage: "legacy crop details",
    });
  });

  it("keeps a valid ok-false search response ready and normalizes nested attempts", () => {
    const normalized = normalizeLegacyOptimizerSearchResponse({
      ok: false,
      errorCode: null,
      errorMessage: null,
      attempts: [
        {
          candidateId: "contain-candidate",
          errorCode: "invoke-failed",
          errorMessage: "ffmpeg diagnostic",
        },
      ],
    });

    expect(normalized.errorCode).toBeNull();
    expect(normalized.reasonCode).toBeNull();
    expect(normalized.attempts[0]).toMatchObject({
      errorCode: "invalid-request",
      reasonCode: "invoke-failed",
      errorMessage: "ffmpeg diagnostic",
    });
  });
});

describe("web source revision", () => {
  const baseFile = {
    name: "sticker.png",
    size: 1024,
    lastModified: 1_700_000_000_000,
    type: "image/png",
  };

  it("is stable for structurally equal File metadata", () => {
    expect(buildWebFileSourceRevision(baseFile)).toBe(
      buildWebFileSourceRevision({ ...baseFile }),
    );
  });

  it.each([
    ["name", "other.png"],
    ["size", 2048],
    ["lastModified", 1_700_000_000_001],
    ["type", "image/apng"],
  ] as const)("changes when %s changes", (field, value) => {
    expect(buildWebFileSourceRevision({ ...baseFile, [field]: value })).not.toBe(
      buildWebFileSourceRevision(baseFile),
    );
  });
});
