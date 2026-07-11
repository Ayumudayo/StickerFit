import { afterEach, describe, expect, it, vi } from "vitest";

import mediaOperationCodes from "../types/media-operation-error-codes.json";
import type {
  MediaOperationErrorCode,
  MediaOperationProgressMessageCode,
  MediaOperationReasonCode,
  OperationProgress,
  OptimizerPlanRequest,
} from "../types/workflow";
import {
  MEDIA_OPERATION_MESSAGES,
  MEDIA_OPERATION_PROGRESS_MESSAGES,
  mediaOperationMessage,
} from "../locales/messages";
import {
  buildWebFileSourceRevision,
  getAppRuntime,
  invokeDesktopMediaOperation,
  normalizeLegacyMediaError,
  normalizeLegacyMediaResponse,
  normalizeLegacyOptimizerSearchResponse,
  normalizeInspectionSourceRevision,
} from "./runtime";

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
      runtime.extractFramePreview({} as never, options),
    ).resolves.toBeNull();
    await expect(
      runtime.extractFramePreviews({} as never, options),
    ).resolves.toBeNull();
  });
});
