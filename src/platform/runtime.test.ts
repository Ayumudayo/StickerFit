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
  normalizeInspectionSourceRevision,
} from "./runtime";

const EXPECTED_ERROR_CODES = [
  "cancelled",
  "timed-out",
  "operation-conflict",
  "invalid-request",
  "source-changed",
  "media-input-too-large",
  "media-dimensions-too-large",
  "media-frame-limit",
  "decoded-byte-limit",
  "png-chunk-limit",
  "malformed-media",
  "malformed-process-output",
  "tool-missing",
  "process-failed",
  "output-conflict",
  "internal-task-failed",
] as const satisfies readonly MediaOperationErrorCode[];

const EXPECTED_REASON_CODES = [
  "no-frames-selected",
  "invalid-frame-selection",
  "invalid-frame-duration",
  "duration-too-long",
  "invalid-crop",
  "invalid-output-directory",
  "unsupported-source-format",
  "unsupported-frame-preview",
  "frame-preview-decode-failed",
  "frame-preview-encode-failed",
  "decode-failed",
  "encode-failed",
  "missing-output",
  "plan-invalid",
  "invoke-failed",
] as const satisfies readonly MediaOperationReasonCode[];

const ERROR_CODES_ARE_COMPLETE: Exclude<
  MediaOperationErrorCode,
  (typeof EXPECTED_ERROR_CODES)[number]
> extends never
  ? true
  : false = true;
const REASON_CODES_ARE_COMPLETE: Exclude<
  MediaOperationReasonCode,
  (typeof EXPECTED_REASON_CODES)[number]
> extends never
  ? true
  : false = true;

function sorted(values: readonly string[]) {
  return [...values].sort();
}

describe("canonical media operation errors", () => {
  it("matches the shared JSON fixture in canonical order", () => {
    expect(ERROR_CODES_ARE_COMPLETE).toBe(true);
    expect(REASON_CODES_ARE_COMPLETE).toBe(true);
    expect(mediaOperationCodes.errorCodes).toEqual(EXPECTED_ERROR_CODES);
    expect(mediaOperationCodes.reasonCodes).toEqual(EXPECTED_REASON_CODES);
  });
  it("keeps the JSON fixture and every locale table exhaustive in both directions", () => {
    for (const locale of ["en", "ko"] as const) {
      expect(sorted(Object.keys(MEDIA_OPERATION_MESSAGES[locale].errorCodes))).toEqual(
        sorted(mediaOperationCodes.errorCodes),
      );
      expect(sorted(Object.keys(MEDIA_OPERATION_MESSAGES[locale].reasonCodes))).toEqual(
        sorted(mediaOperationCodes.reasonCodes),
      );
    }

    expect(MEDIA_OPERATION_MESSAGES.en.errorCodes["png-chunk-limit"]).toBe(
      "A PNG chunk is too large to process safely.",
    );
    expect(MEDIA_OPERATION_MESSAGES.ko.errorCodes["png-chunk-limit"]).toBe(
      "PNG 청크 하나가 너무 커서 안전하게 처리할 수 없습니다.",
    );
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

  it("rejects a canonical but impossible category and reason pairing", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: "process-failed",
        reasonCode: "invalid-crop",
        errorMessage: "safe detail",
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics:
        "safe detail (legacy code: process-failed) (legacy reason code: invalid-crop)",
    });
    expect(
      normalizeLegacyMediaError({
        errorCode: "malformed-media",
        reasonCode: "decode-failed",
      }),
    ).toEqual({
      errorCode: "malformed-media",
      reasonCode: "decode-failed",
      diagnostics: null,
    });
  });

  it("bounds and safely escapes hostile diagnostics while preserving legacy evidence", () => {
    const normalized = normalizeLegacyMediaError({
      errorCode: "future\ncode/C:\\Users\\Alice\\secret-input.mp4",
      reasonCode: "future\u001b\u009b-reason",
      errorMessage: `failed at C:\\Users\\Alice\\secret-input.mp4\n${"x".repeat(600)}`,
    });

    expect(normalized.errorCode).toBe("internal-task-failed");
    expect(normalized.reasonCode).toBeNull();
    expect(normalized.diagnostics).toContain("[path redacted]");
    expect(normalized.diagnostics).toContain("legacy code: [path-redacted]");
    expect(normalized.diagnostics).toContain(
      "legacy reason code: future\\u{001b}\\u{009b}-reason",
    );
    expect(normalized.diagnostics).not.toContain("Users");
    expect(normalized.diagnostics).not.toContain("Alice");
    expect(normalized.diagnostics).not.toContain("secret-input.mp4");
    expect(normalized.diagnostics).not.toContain("\u001b");
    expect(normalized.diagnostics).not.toContain("\u009b");
    expect(normalized.diagnostics!.length).toBeLessThan(900);

    const pathReason = normalizeLegacyMediaError({
      errorCode: "invalid-request",
      reasonCode: "/home/alice/secret-reason",
    });
    expect(pathReason.diagnostics).toBe(
      "(legacy reason code: [path-redacted])",
    );
    expect(pathReason.diagnostics).not.toContain("home");
    expect(pathReason.diagnostics).not.toContain("alice");
    expect(pathReason.diagnostics).not.toContain("secret-reason");

    for (const errorMessage of [
      "failed at \\\\server\\private folder\\input.mp4",
      "failed at C:\\private folder\\input.mp4",
      "failed at \\private\\input.mp4",
      "failed at ..\\private\\input.mp4",
      "failed at /secret folder/input.mp4",
      "failed:/home/private/input.mp4",
      "source=[/private/input.mp4]",
      "remote=https://private.example/input.mp4",
    ]) {
      expect(normalizeLegacyMediaError({ errorMessage })).toEqual({
        errorCode: "internal-task-failed",
        reasonCode: null,
        diagnostics: "[path redacted]",
      });
    }
  });

  it("never invokes hostile unknown-code coercion", () => {
    const nullPrototypeCode = Object.assign(Object.create(null), {
      code: "future-code",
    });
    const hostileCode = {
      toString() {
        throw new Error("must not call toString");
      },
      toJSON() {
        throw new Error("must survive hostile JSON conversion");
      },
    };

    for (const errorCode of [nullPrototypeCode, hostileCode]) {
      const normalized = normalizeLegacyMediaError({ errorCode });
      expect(normalized.errorCode).toBe("internal-task-failed");
      expect(normalized.reasonCode).toBeNull();
      expect(normalized.diagnostics).toContain("legacy code:");
      expect(normalized.diagnostics!.length).toBeLessThan(300);
    }

    expect(
      normalizeLegacyMediaError({
        errorCode: "invalid-request",
        reasonCode: Object.create(null),
      }),
    ).toMatchObject({
      errorCode: "internal-task-failed",
      reasonCode: null,
    });
  });

  it("returns a detail-free internal error when field or Error inspection throws", () => {
    const expected = {
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: null,
    };
    const throwingAccessor = Object.defineProperty({}, "errorCode", {
      get() {
        throw new Error("secret getter detail");
      },
    });
    const throwingReasonAccessor = Object.defineProperty(
      { errorCode: "invalid-request" },
      "reasonCode",
      {
        get() {
          throw new Error("secret reason getter detail");
        },
      },
    );
    const throwingErrorMessageAccessor = Object.defineProperty(
      { errorCode: "future-code" },
      "errorMessage",
      {
        get() {
          throw new Error("secret error message getter detail");
        },
      },
    );
    const throwingPrototypeProxy = new Proxy(
      {},
      {
        getPrototypeOf() {
          throw new Error("secret proxy detail");
        },
      },
    );
    const throwingMessage = Object.defineProperty(
      new Error("unused"),
      "message",
      {
        get() {
          throw new Error("secret message detail");
        },
      },
    );

    for (const raw of [
      throwingAccessor,
      throwingReasonAccessor,
      throwingErrorMessageAccessor,
      throwingPrototypeProxy,
      throwingMessage,
    ]) {
      expect(normalizeLegacyMediaError(raw)).toEqual(expected);
    }
  });

  it("handles null-prototype, hostile coercion, Proxy, and very large unknown values with constant markers", () => {
    const nullPrototypeFields = Object.assign(Object.create(null), {
      errorCode: "future-code",
      errorMessage: "safe detail",
    });
    expect(normalizeLegacyMediaError(nullPrototypeFields)).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: "safe detail (legacy code: future-code)",
    });

    const hostileCoercion = {
      toString() {
        throw new Error("must not coerce");
      },
      valueOf() {
        throw new Error("must not coerce");
      },
      toJSON() {
        throw new Error("must not serialize");
      },
    };
    const hostileUnknownProxy = new Proxy(
      {},
      {
        get() {
          throw new Error("must not read unknown objects");
        },
        ownKeys() {
          throw new Error("must not enumerate unknown objects");
        },
      },
    );
    const veryLargeObject = { payload: "x".repeat(2_000_000) };

    for (const errorCode of [
      hostileCoercion,
      hostileUnknownProxy,
      veryLargeObject,
    ]) {
      expect(normalizeLegacyMediaError({ errorCode })).toEqual({
        errorCode: "internal-task-failed",
        reasonCode: null,
        diagnostics: "(legacy code: [object])",
      });
    }

    const veryLargeString = `future-${"x".repeat(2_000_000)}`;
    const largeStringResult = normalizeLegacyMediaError({
      errorCode: veryLargeString,
      errorMessage: veryLargeString,
    });
    expect(largeStringResult.errorCode).toBe("internal-task-failed");
    expect(largeStringResult.reasonCode).toBeNull();
    expect(largeStringResult.diagnostics!.length).toBeLessThan(700);
    expect(largeStringResult.diagnostics).not.toContain(
      veryLargeString.slice(-1_000),
    );
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

  it("does not expose source metadata in the revision", () => {
    const revision = buildWebFileSourceRevision(baseFile);

    expect(revision).toMatch(/^web-[0-9a-f]{16}$/);
    expect(revision).not.toContain(baseFile.name);
    expect(revision).not.toContain(baseFile.type);
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

describe("inspection source revision contract", () => {
  it("keeps a non-empty revision on successful inspection", () => {
    expect(
      normalizeInspectionSourceRevision({
        ok: true,
        sourceRevision: "opaque-revision-1",
      }),
    ).toBe("opaque-revision-1");
  });

  it.each([null, "", undefined])(
    "rejects %s as the revision of a successful inspection",
    (sourceRevision) => {
      expect(() =>
        normalizeInspectionSourceRevision({ ok: true, sourceRevision }),
      ).toThrow(
        "Successful media inspection did not include a source revision.",
      );
    },
  );

  it("uses explicit null for failed inspection", () => {
    expect(
      normalizeInspectionSourceRevision({
        ok: false,
        sourceRevision: "must-not-survive",
      }),
    ).toBeNull();
  });
});
