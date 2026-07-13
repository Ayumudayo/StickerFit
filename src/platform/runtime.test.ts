import { afterEach, describe, expect, it, vi } from "vitest";

import mediaOperationCodes from "../types/media-operation-error-codes.json";
import type {
  CandidateSizeProbeRequest,
  ExactCandidateSizeEstimate,
  MediaOperationErrorCode,
  MediaOperationProgressMessageCode,
  MediaOperationReasonCode,
  OperationProgress,
  OutputSizeEstimate,
  OptimizerCandidatePreview,
  OptimizerPlanRequest,
  OptimizerSizeEstimateRequest,
  StaticSizeEstimateRequest,
} from "../types/workflow";
import {
  MESSAGES,
  MEDIA_OPERATION_MESSAGES,
  MEDIA_OPERATION_PROGRESS_MESSAGES,
  mediaOperationMessage,
} from "../locales/messages";
import {
  buildWebFileSourceRevision,
  getAppRuntime,
  invokeDesktopEstimateOptimizerCandidates,
  invokeDesktopMediaOperation,
  invokeDesktopProbeOptimizerCandidateSize,
  invokeDesktopStaticSizeEstimate,
  normalizeLegacyMediaError,
  normalizeLegacyMediaResponse,
  normalizeLegacyOptimizerSearchResponse,
  normalizeInspectionFallbackReasonCode,
  normalizeInspectionSourceRevision,
} from "./runtime";
import runtimeSource from "./runtime.ts?raw";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

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

function hasExhaustiveRuntimeMembershipTable(
  source: string,
  tableName: string,
  codeType: string,
) {
  const closedRecord = new RegExp(
    `const\\s+${tableName}\\s*=\\s*\\{[\\s\\S]*?\\}\\s*as\\s+const\\s+satisfies\\s+Record<\\s*${codeType}\\s*,\\s*true\\s*>`,
  );
  const closedTuple = new RegExp(
    `const\\s+${tableName}\\s*=\\s*\\[[\\s\\S]*?\\]\\s*as\\s+const\\s+satisfies\\s+readonly\\s+${codeType}\\[\\]`,
  );
  const tupleCompleteness = new RegExp(
    `Exclude<\\s*${codeType}\\s*,\\s*\\(typeof\\s+${tableName}\\)\\[number\\]\\s*>\\s+extends\\s+never`,
  );
  return (
    closedRecord.test(source) ||
    (closedTuple.test(source) && tupleCompleteness.test(source))
  );
}

function ownMembershipHelperName(source: string) {
  return (
    source.match(
      /function\s+([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*\{[\s\S]*?(?:Object\.prototype\.hasOwnProperty\.call|Object\.hasOwn)\([\s\S]*?\)[\s\S]*?\}/,
    )?.[1] ?? null
  );
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

  it("uses compile-time closed runtime membership instead of trusting the JSON fixture", () => {
    expect(runtimeSource).not.toContain("media-operation-error-codes.json");
    expect(runtimeSource).not.toMatch(
      /new\s+Set(?:<[^>]+>)?\s*\(\s*mediaOperationCodes\.(?:errorCodes|reasonCodes)/,
    );
    expect(
      hasExhaustiveRuntimeMembershipTable(
        runtimeSource,
        "MEDIA_OPERATION_ERROR_CODES",
        "MediaOperationErrorCode",
      ),
    ).toBe(true);
    expect(
      hasExhaustiveRuntimeMembershipTable(
        runtimeSource,
        "MEDIA_OPERATION_REASON_CODES",
        "MediaOperationReasonCode",
      ),
    ).toBe(true);
    const helperName = ownMembershipHelperName(runtimeSource);
    expect(helperName).not.toBeNull();
    const helperCall = helperName ?? "missingOwnMembershipHelper";
    expect(runtimeSource).toMatch(
      new RegExp(`${helperCall}\\(\\s*MEDIA_OPERATION_ERROR_CODES\\s*,`),
    );
    expect(runtimeSource).toMatch(
      new RegExp(`${helperCall}\\(\\s*MEDIA_OPERATION_REASON_CODES\\s*,`),
    );
  });

  it.each(mediaOperationCodes.wireCases)(
    "round-trips the shared Rust wire case $name through the runtime decoder",
    ({ wire, expected }) => {
      expect(normalizeLegacyMediaResponse(wire)).toEqual(expected);
    },
  );

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

  it("keeps only the bounded raw code marker for an unknown error", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: "future-backend-code",
        errorMessage: "backend diagnostic",
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: "(legacy code: future-backend-code)",
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
      diagnostics: "(legacy code: future-backend-code)",
    });
  });

  it("drops an unknown companion reason when the error code is already unknown", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: "future-backend-code",
        reasonCode: "future-reason",
        errorMessage: "detail",
      }),
    ).toEqual({
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: "(legacy code: future-backend-code)",
    });
  });

  it("does not silently accept an unknown reason without an error category", () => {
    expect(
      normalizeLegacyMediaError({
        errorCode: null,
        reasonCode: "future-reason",
        errorMessage: "untrusted backend diagnostic",
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
      diagnostics: "(legacy reason code: future-reason)",
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

  it("bounds and redacts raw unknown markers without retaining companion diagnostics", () => {
    const normalized = normalizeLegacyMediaError({
      errorCode: "future\ncode/C:\\Users\\Alice\\secret-input.mp4",
      reasonCode: "future\u001b\u009b-reason",
      errorMessage: `failed at C:\\Users\\Alice\\secret-input.mp4\n${"x".repeat(600)}`,
    });

    expect(normalized.errorCode).toBe("internal-task-failed");
    expect(normalized.reasonCode).toBeNull();
    expect(normalized.diagnostics).toBe("(legacy code: [path-redacted])");
    expect(normalized.diagnostics).not.toContain("legacy reason code:");
    expect(normalized.diagnostics).not.toContain("failed at");
    expect(normalized.diagnostics).not.toContain("Users");
    expect(normalized.diagnostics).not.toContain("Alice");
    expect(normalized.diagnostics).not.toContain("secret-input.mp4");
    expect(normalized.diagnostics).not.toContain("\u001b");
    expect(normalized.diagnostics).not.toContain("\u009b");
    expect(normalized.diagnostics!.length).toBeLessThan(300);

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
      diagnostics: "(legacy code: future-code)",
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
    expect(largeStringResult.diagnostics!.length).toBeLessThan(300);
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

describe("inspection fallback provenance contract", () => {
  it("accepts only the exact Media Foundation fallback code", () => {
    expect(
      normalizeInspectionFallbackReasonCode({
        fallbackReasonCode: "media-foundation-failed",
      }),
    ).toBe("media-foundation-failed");
  });

  it.each([
    {},
    { fallbackReasonCode: null },
    { fallbackReasonCode: "media_foundation_failed" },
    { fallbackReasonCode: "future-fallback" },
    { fallbackReasonCode: { value: "media-foundation-failed" } },
    { fallbackReason: "media-foundation-failed" },
    { toolDetail: "media-foundation-failed" },
  ])("normalizes missing, legacy, malformed, and unknown provenance to null", (raw) => {
    expect(normalizeInspectionFallbackReasonCode(raw)).toBeNull();
  });

  it("never coerces hostile fallback values", () => {
    expect(
      normalizeInspectionFallbackReasonCode({
        fallbackReasonCode: {
          toString() {
            throw new Error("must not coerce fallback provenance");
          },
        },
      }),
    ).toBeNull();
  });

  it("ships nonempty English and Korean fallback copy", () => {
    expect(MESSAGES.en.mediaFoundationFallbackWarning.trim()).not.toBe("");
    expect(MESSAGES.ko.mediaFoundationFallbackWarning.trim()).not.toBe("");
  });
});

const EXPECTED_PROGRESS_MESSAGE_CODES = [
  "media-operation-queued",
  "media-operation-inspecting",
  "media-operation-decoding",
  "media-operation-estimating",
  "media-operation-encoding",
  "media-operation-finalizing",
] as const satisfies readonly MediaOperationProgressMessageCode[];

const PROGRESS_CODES_ARE_COMPLETE: Exclude<
  MediaOperationProgressMessageCode,
  (typeof EXPECTED_PROGRESS_MESSAGE_CODES)[number]
> extends never
  ? true
  : false = true;

const PROGRESS_MESSAGE_CODE_BY_STAGE: Record<
  OperationProgress["stage"],
  MediaOperationProgressMessageCode
> = {
  queued: "media-operation-queued",
  inspecting: "media-operation-inspecting",
  decoding: "media-operation-decoding",
  estimating: "media-operation-estimating",
  encoding: "media-operation-encoding",
  finalizing: "media-operation-finalizing",
};

type OutputSizeEstimateContractKey = OutputSizeEstimate extends infer Estimate
  ? Estimate extends {
      kind: infer Kind extends string;
      basis: infer Basis extends string;
    }
    ? `${Kind}:${Basis}`
    : never
  : never;

const EXPECTED_OUTPUT_SIZE_ESTIMATE_CONTRACT = [
  "exact-static:exact-static",
  "exact-candidate:exact-full-sequence",
  "exact-candidate:probe",
  "range:sampled",
] as const satisfies readonly OutputSizeEstimateContractKey[];

const OUTPUT_SIZE_ESTIMATE_CONTRACT_IS_COMPLETE: Exclude<
  OutputSizeEstimateContractKey,
  (typeof EXPECTED_OUTPUT_SIZE_ESTIMATE_CONTRACT)[number]
> extends never
  ? true
  : false = true;

function operationProgress(
  operationId: string,
  stage: OperationProgress["stage"] = "encoding",
): OperationProgress {
  const progress = {
    operationId,
    stage,
    completed: stage === "encoding" ? 2 : 0,
    total: stage === "encoding" ? 5 : null,
    messageCode: PROGRESS_MESSAGE_CODE_BY_STAGE[stage],
  } satisfies OperationProgress;
  return progress;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

function optimizerPlanRequest(): OptimizerPlanRequest {
  return {
    locale: "en",
    sourceDurationSeconds: 2,
    inputWidth: 320,
    inputHeight: 320,
    avgFps: 12,
    presetStrategy: "auto",
    optimizerGoal: "balanced",
    qualityFrameDropInterval: 1,
    searchDepth: "standard",
    cropRegion: null,
  };
}

describe("operation progress localization", () => {
  it("keeps the six closed progress codes exhaustive and localized", () => {
    expect(PROGRESS_CODES_ARE_COMPLETE).toBe(true);
    for (const locale of ["en", "ko"] as const) {
      expect(sorted(Object.keys(MEDIA_OPERATION_PROGRESS_MESSAGES[locale]))).toEqual(
        sorted(EXPECTED_PROGRESS_MESSAGE_CODES),
      );
      for (const code of EXPECTED_PROGRESS_MESSAGE_CODES) {
        expect(MEDIA_OPERATION_PROGRESS_MESSAGES[locale][code]).not.toBe("");
      }
    }
  });
});

describe("desktop media operation adapter", () => {
  it("keeps the output-size estimate union closed across every kind and basis", () => {
    expect(OUTPUT_SIZE_ESTIMATE_CONTRACT_IS_COMPLETE).toBe(true);
    expect(EXPECTED_OUTPUT_SIZE_ESTIMATE_CONTRACT).toEqual([
      "exact-static:exact-static",
      "exact-candidate:exact-full-sequence",
      "exact-candidate:probe",
      "range:sampled",
    ]);
  });

  it("keeps the backend relative-size ordering hint on candidate previews", () => {
    const candidate = {
      id: "candidate-balanced",
      rank: 1,
      durationSeconds: 2,
      fps: 12,
      contentScale: 0.8,
      preset: "default",
      score: 0.91,
      relativeSizeFactor: 0.64,
      sourceSimilarityScore: 0.95,
      summary: "Balanced candidate",
    } satisfies OptimizerCandidatePreview;

    expect(candidate.relativeSizeFactor).toBe(0.64);
  });

  it("sends the static estimate request unchanged through the managed command envelope", async () => {
    const request = {
      inputPath: "C:/media/sticker.png",
      sourceRevision: "source-revision-1",
      locale: "ko",
      cropRegion: { x: 0.1, y: 0.2, width: 0.7, height: 0.6 },
    } satisfies StaticSizeEstimateRequest;
    const estimate = {
      kind: "exact-static",
      basis: "exact-static",
      bytes: 12_345,
      candidateId: null,
      limitBytes: 512 * 1_024,
      outputFrameCount: 1,
    } satisfies OutputSizeEstimate;
    const channel = { serialized: "__TAURI_CHANNEL__" };
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

    await expect(
      invokeDesktopStaticSizeEstimate(
        request,
        { operationId: "operation-static-estimate" },
        {
          createChannel: () => channel,
          invoke: async <T>(command: string, args?: Record<string, unknown>) => {
            calls.push({ command, args });
            return estimate as unknown as T;
          },
        },
      ),
    ).resolves.toBe(estimate);

    expect(calls).toEqual([
      {
        command: "estimate_static_output_size",
        args: {
          request,
          operationId: "operation-static-estimate",
          onProgress: channel,
        },
      },
    ]);
    expect(request).not.toHaveProperty("operationId");
  });

  it("preserves a closed backend estimate rejection for one caller-side normalization", async () => {
    const rejection = {
      errorCode: "source-changed",
      reasonCode: null,
      errorMessage: "Source changed before estimate publication.",
    };

    await expect(
      invokeDesktopStaticSizeEstimate(
        {
          inputPath: "C:/media/sticker.png",
          sourceRevision: "stale-revision",
          locale: "en",
          cropRegion: null,
        },
        { operationId: "operation-static-estimate-rejected" },
        {
          createChannel: () => ({}),
          invoke: () => Promise.reject(rejection),
        },
      ),
    ).rejects.toBe(rejection);
  });

  it("sends candidate estimate requests unchanged through the direct managed envelope", async () => {
    const request = {
      ...optimizerPlanRequest(),
      inputPath: "C:/media/sticker.gif",
      sourceRevision: "source-revision-2",
      sampleSeed: "0123456789abcdef",
      candidateIds: ["candidate-balanced", "candidate-small"],
    } satisfies OptimizerSizeEstimateRequest;
    const estimates = [
      {
        kind: "range",
        basis: "sampled",
        lowerBytes: 220_000,
        predictedBytes: 260_000,
        upperBytes: 310_000,
        confidence: "high",
        candidateId: "candidate-balanced",
        limitBytes: 512 * 1_024,
        measuredContributionCount: 12,
        outputFrameCount: 150,
      },
    ] satisfies OutputSizeEstimate[];
    const channel = { serialized: "__TAURI_CHANNEL__" };
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

    await expect(
      invokeDesktopEstimateOptimizerCandidates(
        request,
        { operationId: "operation-candidate-estimates" },
        {
          createChannel: () => channel,
          invoke: async <T>(command: string, args?: Record<string, unknown>) => {
            calls.push({ command, args });
            return estimates as unknown as T;
          },
        },
      ),
    ).resolves.toBe(estimates);

    expect(calls).toEqual([
      {
        command: "estimate_optimizer_candidates",
        args: {
          request,
          operationId: "operation-candidate-estimates",
          onProgress: channel,
        },
      },
    ]);
    expect(request).not.toHaveProperty("operationId");
  });

  it("sends an exact candidate probe through the direct managed envelope", async () => {
    const request = {
      ...optimizerPlanRequest(),
      inputPath: "C:/media/sticker.gif",
      sourceRevision: "source-revision-3",
      candidateId: "candidate-balanced",
    } satisfies CandidateSizeProbeRequest;
    const estimate = {
      kind: "exact-candidate",
      basis: "probe",
      bytes: 271_828,
      candidateId: "candidate-balanced",
      limitBytes: 512 * 1_024,
      outputFrameCount: 150,
    } satisfies ExactCandidateSizeEstimate;
    const channel = { serialized: "__TAURI_CHANNEL__" };
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

    await expect(
      invokeDesktopProbeOptimizerCandidateSize(
        request,
        { operationId: "operation-candidate-probe" },
        {
          createChannel: () => channel,
          invoke: async <T>(command: string, args?: Record<string, unknown>) => {
            calls.push({ command, args });
            return estimate as unknown as T;
          },
        },
      ),
    ).resolves.toBe(estimate);

    expect(calls).toEqual([
      {
        command: "probe_optimizer_candidate_size",
        args: {
          request,
          operationId: "operation-candidate-probe",
          onProgress: channel,
        },
      },
    ]);
    expect(request).not.toHaveProperty("operationId");
  });

  it("preserves candidate estimate and probe backend rejections unchanged", async () => {
    const rejection = {
      errorCode: "invalid-request",
      reasonCode: null,
      errorMessage: "Candidate request no longer matches the current plan.",
    };
    const bridge = {
      createChannel: () => ({}),
      invoke: () => Promise.reject(rejection),
    };

    await expect(
      invokeDesktopEstimateOptimizerCandidates(
        {
          ...optimizerPlanRequest(),
          inputPath: "C:/media/sticker.gif",
          sourceRevision: "source-revision-4",
          sampleSeed: "fedcba9876543210",
          candidateIds: ["missing-candidate"],
        },
        { operationId: "operation-candidate-estimates-rejected" },
        bridge,
      ),
    ).rejects.toBe(rejection);
    await expect(
      invokeDesktopProbeOptimizerCandidateSize(
        {
          ...optimizerPlanRequest(),
          inputPath: "C:/media/sticker.gif",
          sourceRevision: "source-revision-4",
          candidateId: "missing-candidate",
        },
        { operationId: "operation-candidate-probe-rejected" },
        bridge,
      ),
    ).rejects.toBe(rejection);
  });

  it("reuses the managed AbortSignal cancellation bridge for candidate estimates", async () => {
    const controller = new AbortController();
    const estimates = deferred<OutputSizeEstimate[]>();
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
    const operation = invokeDesktopEstimateOptimizerCandidates(
      {
        ...optimizerPlanRequest(),
        inputPath: "C:/media/sticker.gif",
        sourceRevision: "source-revision-5",
        sampleSeed: "0011223344556677",
        candidateIds: ["candidate-balanced"],
      },
      {
        operationId: "operation-candidate-estimates-aborted",
        signal: controller.signal,
      },
      {
        createChannel: () => ({}),
        invoke: <T>(command: string, args?: Record<string, unknown>) => {
          calls.push({ command, args });
          return command === "cancel_media_operation"
            ? Promise.resolve(true as unknown as T)
            : (estimates.promise as unknown as Promise<T>);
        },
      },
    );

    controller.abort();
    await Promise.resolve();
    estimates.resolve([]);
    await operation;

    expect(
      calls.filter(({ command }) => command === "cancel_media_operation"),
    ).toEqual([
      {
        command: "cancel_media_operation",
        args: { operationId: "operation-candidate-estimates-aborted" },
      },
    ]);
  });

  it("keeps the DTO unchanged and adds operationId plus the Channel at top level", async () => {
    const request = { inputPath: "C:/media/input.gif", locale: "en" };
    const channel = { serialized: "__TAURI_CHANNEL__" };
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

    await invokeDesktopMediaOperation(
      "build_optimizer_plan",
      { request },
      { operationId: "operation-1" },
      {
        createChannel: () => channel,
        invoke: async <T>(command: string, args?: Record<string, unknown>) => {
          calls.push({ command, args });
          return { ok: true } as unknown as T;
        },
      },
    );

    expect(request).toEqual({ inputPath: "C:/media/input.gif", locale: "en" });
    expect(request).not.toHaveProperty("operationId");
    expect(calls).toEqual([
      {
        command: "build_optimizer_plan",
        args: {
          request,
          operationId: "operation-1",
          onProgress: channel,
        },
      },
    ]);
  });

  it("preserves flat command arguments while adding operation metadata", async () => {
    const channel = { serialized: "__TAURI_CHANNEL__" };
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

    await invokeDesktopMediaOperation(
      "inspect_input_media",
      { inputPath: "C:/media/input.gif", locale: "ko" },
      { operationId: "operation-flat" },
      {
        createChannel: () => channel,
        invoke: async <T>(command: string, args?: Record<string, unknown>) => {
          calls.push({ command, args });
          return { ok: true } as unknown as T;
        },
      },
    );

    expect(calls).toEqual([
      {
        command: "inspect_input_media",
        args: {
          inputPath: "C:/media/input.gif",
          locale: "ko",
          operationId: "operation-flat",
          onProgress: channel,
        },
      },
    ]);
  });

  it("forwards matching progress and suppresses a different operation ID", async () => {
    const forwarded: OperationProgress[] = [];

    await invokeDesktopMediaOperation(
      "inspect_input_media",
      { inputPath: "C:/media/input.gif", locale: "en" },
      {
        operationId: "operation-current",
        onProgress: (progress) => forwarded.push(progress),
      },
      {
        createChannel: (onMessage) => ({ onMessage }),
        invoke: async <T>(_command: string, args?: Record<string, unknown>) => {
          const channel = args?.onProgress as {
            onMessage: (progress: OperationProgress) => void;
          };
          channel.onMessage(operationProgress("operation-stale"));
          channel.onMessage(operationProgress("operation-current"));
          return { ok: true } as unknown as T;
        },
      },
    );

    expect(forwarded).toEqual([operationProgress("operation-current")]);
  });

  it("suppresses progress delivered after abort", async () => {
    const controller = new AbortController();
    const forwarded: OperationProgress[] = [];

    await invokeDesktopMediaOperation(
      "extract_frame_preview",
      { inputPath: "C:/media/input.gif" },
      {
        operationId: "operation-aborted",
        signal: controller.signal,
        onProgress: (progress) => forwarded.push(progress),
      },
      {
        createChannel: (onMessage) => ({ onMessage }),
        invoke: async <T>(command: string, args?: Record<string, unknown>) => {
          if (command === "extract_frame_preview") {
            controller.abort();
            const channel = args?.onProgress as {
              onMessage: (progress: OperationProgress) => void;
            };
            channel.onMessage(operationProgress("operation-aborted"));
          }
          return { ok: true } as unknown as T;
        },
      },
    );

    expect(forwarded).toEqual([]);
  });

  it("removes the abort listener after the heavy invoke settles", async () => {
    const controller = new AbortController();
    const addListener = vi.spyOn(controller.signal, "addEventListener");
    const removeListener = vi.spyOn(controller.signal, "removeEventListener");

    await invokeDesktopMediaOperation(
      "build_optimizer_plan",
      { request: {} },
      { operationId: "operation-listener", signal: controller.signal },
      {
        createChannel: () => ({}),
        invoke: async <T>() => ({ ok: true } as unknown as T),
      },
    );

    expect(addListener).toHaveBeenCalledOnce();
    expect(removeListener).toHaveBeenCalledOnce();
    expect(removeListener.mock.calls[0]?.[1]).toBe(addListener.mock.calls[0]?.[1]);
  });

  it("removes the abort listener when the heavy invoke rejects", async () => {
    const controller = new AbortController();
    const removeListener = vi.spyOn(controller.signal, "removeEventListener");

    await expect(
      invokeDesktopMediaOperation(
        "build_optimizer_plan",
        { request: {} },
        { operationId: "operation-heavy-reject", signal: controller.signal },
        {
          createChannel: () => ({}),
          invoke: () => Promise.reject(new Error("heavy invoke failed")),
        },
      ),
    ).rejects.toThrow("heavy invoke failed");

    expect(removeListener).toHaveBeenCalledOnce();
  });

  it("sends live abort cancellation exactly once", async () => {
    const controller = new AbortController();
    const heavy = deferred<{ ok: boolean }>();
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
    const operation = invokeDesktopMediaOperation(
      "run_optimizer_search",
      { request: {} },
      { operationId: "operation-live-abort", signal: controller.signal },
      {
        createChannel: () => ({}),
        invoke: async <T>(command: string, args?: Record<string, unknown>) => {
          calls.push({ command, args });
          if (command === "cancel_media_operation") {
            return true as unknown as T;
          }
          return heavy.promise as unknown as Promise<T>;
        },
      },
    );

    controller.abort();
    controller.abort();
    await Promise.resolve();
    heavy.resolve({ ok: true });
    await operation;

    expect(
      calls.filter(({ command }) => command === "cancel_media_operation"),
    ).toEqual([
      {
        command: "cancel_media_operation",
        args: { operationId: "operation-live-abort" },
      },
    ]);
  });

  it("awaits already-aborted cancellation before invoking the heavy command", async () => {
    const controller = new AbortController();
    controller.abort();
    const cancel = deferred<boolean>();
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

    const operation = invokeDesktopMediaOperation(
      "convert_static_image_to_png",
      { request: {} },
      { operationId: "operation-pre-abort", signal: controller.signal },
      {
        createChannel: () => ({}),
        invoke: <T>(command: string, args?: Record<string, unknown>) => {
          calls.push({ command, args });
          return command === "cancel_media_operation"
            ? (cancel.promise as unknown as Promise<T>)
            : Promise.resolve({ ok: false } as unknown as T);
        },
      },
    );

    await Promise.resolve();
    expect(calls).toEqual([
      {
        command: "cancel_media_operation",
        args: { operationId: "operation-pre-abort" },
      },
    ]);
    cancel.resolve(true);
    await operation;
    expect(calls.map(({ command }) => command)).toEqual([
      "cancel_media_operation",
      "convert_static_image_to_png",
    ]);
  });

  it("closes the listener-installation abort race before starting the heavy command", async () => {
    const cancel = deferred<boolean>();
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
    let aborted = false;
    let installedListener: EventListener | null = null;
    const signal = {
      get aborted() {
        return aborted;
      },
      addEventListener: (_type: string, listener: EventListenerOrEventListenerObject) => {
        installedListener = listener as EventListener;
        aborted = true;
        installedListener(new Event("abort"));
      },
      removeEventListener: (_type: string, listener: EventListenerOrEventListenerObject) => {
        if (installedListener === listener) {
          installedListener = null;
        }
      },
    } as unknown as AbortSignal;

    const operation = invokeDesktopMediaOperation(
      "extract_frame_preview",
      { inputPath: "C:/media/input.gif" },
      { operationId: "operation-install-race", signal },
      {
        createChannel: () => ({}),
        invoke: <T>(command: string, args?: Record<string, unknown>) => {
          calls.push({ command, args });
          return command === "cancel_media_operation"
            ? (cancel.promise as unknown as Promise<T>)
            : Promise.resolve({ ok: false } as unknown as T);
        },
      },
    );

    await Promise.resolve();
    expect(calls).toEqual([
      {
        command: "cancel_media_operation",
        args: { operationId: "operation-install-race" },
      },
    ]);
    cancel.resolve(true);
    await operation;
    expect(calls.map(({ command }) => command)).toEqual([
      "cancel_media_operation",
      "extract_frame_preview",
    ]);
  });

  it("absorbs cancel invoke rejection without rejecting the heavy operation", async () => {
    const controller = new AbortController();
    const heavy = deferred<{ ok: boolean }>();
    const operation = invokeDesktopMediaOperation(
      "extract_frame_previews",
      { inputPath: "C:/media/input.gif" },
      { operationId: "operation-cancel-rejection", signal: controller.signal },
      {
        createChannel: () => ({}),
        invoke: <T>(command: string) =>
          command === "cancel_media_operation"
            ? Promise.reject(new Error("cancel IPC unavailable"))
            : (heavy.promise as unknown as Promise<T>),
      },
    );

    controller.abort();
    await Promise.resolve();
    heavy.resolve({ ok: true });

    await expect(operation).resolves.toEqual({ ok: true });
  });
});

describe("web media operation adapter contract", () => {
  it("accepts options for web-file inspection without changing its result contract", async () => {
    class FakeImage {
      decoding = "";
      naturalWidth = 64;
      naturalHeight = 32;
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;

      set src(_value: string) {
        queueMicrotask(() => this.onload?.());
      }
    }
    vi.stubGlobal("Image", FakeImage);
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:operation-web-file");
    const runtime = getAppRuntime();
    const file = {
      name: "sticker.png",
      size: 1_024,
      lastModified: 1_700_000_000_000,
      type: "image/png",
    } as unknown as File;

    const inspection = await runtime.inspectInput(
      { kind: "web-file", file },
      "en",
      { operationId: "web-inspection" },
    );

    expect(inspection).toMatchObject({
      ok: true,
      inputPath: "sticker.png",
      width: 64,
      height: 32,
      inputSourceKind: "file",
      fallbackReasonCode: null,
    });
  });

  it("keeps browser video fallback provenance null", async () => {
    class FakeVideo {
      preload = "";
      muted = false;
      playsInline = false;
      duration = 1;
      videoWidth = 320;
      videoHeight = 240;
      onloadedmetadata: (() => void) | null = null;
      onerror: (() => void) | null = null;

      set src(value: string) {
        if (value) {
          queueMicrotask(() => this.onloadedmetadata?.());
        }
      }
    }
    const createElement = document.createElement.bind(document);
    vi.spyOn(document, "createElement").mockImplementation((tagName) =>
      tagName === "video"
        ? (new FakeVideo() as unknown as HTMLVideoElement)
        : createElement(tagName),
    );
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:operation-web-video");
    const runtime = getAppRuntime();
    const file = {
      name: "clip.mp4",
      size: 2_048,
      lastModified: 1_700_000_000_001,
      type: "video/mp4",
    } as unknown as File;

    const inspection = await runtime.inspectInput(
      { kind: "web-file", file },
      "en",
      { operationId: "web-video-inspection" },
    );

    expect(inspection).toMatchObject({
      ok: true,
      toolSource: "browser",
      fallbackReasonCode: null,
    });
  });

  it("keeps browser inspection errors fallback provenance null", async () => {
    class FailingImage {
      decoding = "";
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;

      set src(_value: string) {
        queueMicrotask(() => this.onerror?.());
      }
    }
    vi.stubGlobal("Image", FailingImage);
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:operation-web-error");
    const runtime = getAppRuntime();
    const file = {
      name: "broken.png",
      size: 128,
      lastModified: 1_700_000_000_002,
      type: "image/png",
    } as unknown as File;

    const inspection = await runtime.inspectInput(
      { kind: "web-file", file },
      "en",
      { operationId: "web-error-inspection" },
    );

    expect(inspection).toMatchObject({
      ok: false,
      toolSource: "browser",
      fallbackReasonCode: null,
    });
  });

  it("preserves all desktop-only rejections and preview nulls", async () => {
    const runtime = getAppRuntime();
    const options = { operationId: "web-operation" };

    await expect(
      runtime.buildOptimizerPlan({} as OptimizerPlanRequest, options),
    ).rejects.toThrow("available only in the desktop app");
    await expect(
      runtime.runOptimizerSearch({} as never, options),
    ).rejects.toThrow("available only in the desktop app");
    await expect(
      runtime.convertStaticImageToPng({} as never, options),
    ).rejects.toThrow("available only in the desktop app");
    await expect(
      runtime.estimateStaticOutputSize({} as never, options),
    ).rejects.toThrow("available only in the desktop app");
    await expect(
      runtime.estimateOptimizerCandidates({} as never, options),
    ).rejects.toThrow("available only in the desktop app");
    await expect(
      runtime.probeOptimizerCandidateSize({} as never, options),
    ).rejects.toThrow("available only in the desktop app");
    await expect(
      runtime.extractFramePreview({} as never, options),
    ).resolves.toBeNull();
    await expect(
      runtime.extractFramePreviews({} as never, options),
    ).resolves.toBeNull();
  });
});
