import type { Locale } from "../locales/messages";
import type {
  MediaInspection,
  MediaInspectionFallbackReasonCode,
  MediaOperationErrorCode,
  MediaOperationErrorFields,
  MediaOperationReasonCode,
  OperationProgress,
  FramePreviewRequest,
  FramePreviewResult,
  FramePreviewsRequest,
  FramePreviewsResult,
  OptimizerPlanRequest,
  OptimizerPlanResponse,
  OptimizerSearchRequest,
  OptimizerSearchResponse,
  SearchAttemptResult,
  StaticImageConversionRequest,
  StaticImageConversionResult,
  ToolHealthReport,
} from "../types/workflow";

export type NormalizedMediaError = {
  errorCode: MediaOperationErrorCode | null;
  reasonCode: MediaOperationReasonCode | null;
  diagnostics: string | null;
};

type RawMediaErrorFields = {
  errorCode?: unknown;
  reasonCode?: unknown;
  errorMessage?: unknown;
};

const MEDIA_OPERATION_ERROR_CODES = {
  cancelled: true,
  "timed-out": true,
  "operation-conflict": true,
  "invalid-request": true,
  "source-changed": true,
  "media-input-too-large": true,
  "media-dimensions-too-large": true,
  "media-frame-limit": true,
  "decoded-byte-limit": true,
  "png-chunk-limit": true,
  "malformed-media": true,
  "malformed-process-output": true,
  "tool-missing": true,
  "process-failed": true,
  "output-conflict": true,
  "internal-task-failed": true,
} as const satisfies Record<MediaOperationErrorCode, true>;

const MEDIA_OPERATION_REASON_CODES = {
  "no-frames-selected": true,
  "invalid-frame-selection": true,
  "invalid-frame-duration": true,
  "duration-too-long": true,
  "invalid-crop": true,
  "invalid-output-directory": true,
  "unsupported-source-format": true,
  "unsupported-frame-preview": true,
  "frame-preview-decode-failed": true,
  "frame-preview-encode-failed": true,
  "decode-failed": true,
  "encode-failed": true,
  "missing-output": true,
  "plan-invalid": true,
  "invoke-failed": true,
} as const satisfies Record<MediaOperationReasonCode, true>;

function hasOwnCode(table: object, value: string) {
  return Object.prototype.hasOwnProperty.call(table, value);
}

function isMediaOperationErrorCode(
  value: unknown,
): value is MediaOperationErrorCode {
  return (
    typeof value === "string" &&
    value.length <= 64 &&
    hasOwnCode(MEDIA_OPERATION_ERROR_CODES, value)
  );
}

function isMediaOperationReasonCode(
  value: unknown,
): value is MediaOperationReasonCode {
  return (
    typeof value === "string" &&
    value.length <= 64 &&
    hasOwnCode(MEDIA_OPERATION_REASON_CODES, value)
  );
}

function containsPathSignal(value: string) {
  return (
    /[A-Za-z]:[\\/]/.test(value) ||
    /\\\\/.test(value) ||
    /(?:^|[^A-Za-z0-9])\/[^\s]/.test(value) ||
    /(?:^|[^A-Za-z0-9])\\[^\s]/.test(value)
  );
}

function boundedUnknownValue(value: unknown) {
  const valueType = typeof value;
  let serialized: string;

  if (valueType === "string") {
    const stringValue = value as string;
    return stringValue.length > 256
      ? `${stringValue.slice(0, 256)}…`
      : stringValue;
  } else if (value === null) {
    serialized = "null";
  } else if (
    valueType === "number" ||
    valueType === "boolean" ||
    valueType === "undefined"
  ) {
    serialized = String(value);
  } else {
    serialized = `[${valueType}]`;
  }

  return serialized;
}

function diagnosticMessage(value: unknown) {
  if (typeof value !== "string") {
    return null;
  }

  const sample = value.length > 513 ? value.slice(0, 513) : value;
  const trimmed = sample.trim();
  if (containsPathSignal(trimmed)) {
    return "[path redacted]";
  }
  const escapedControls = Array.from(trimmed, (character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint < 0x20 || (codePoint >= 0x7f && codePoint <= 0x9f)
      ? `\\u{${codePoint.toString(16).padStart(4, "0")}}`
      : character;
  }).join("");
  if (!escapedControls) {
    return null;
  }
  return escapedControls.length > 512 || sample.length < value.length
    ? `${escapedControls.slice(0, 512)}…`
    : escapedControls;
}

function safeLegacyValue(value: string) {
  const sample = value.length > 129 ? value.slice(0, 129) : value;
  if (containsPathSignal(sample)) {
    return "[path-redacted]";
  }
  const escaped = Array.from(sample, (character) =>
    /^[A-Za-z0-9._\[\]-]$/.test(character)
      ? character
      : `\\u{${(character.codePointAt(0) ?? 0)
          .toString(16)
          .padStart(4, "0")}}`,
  ).join("");
  return escaped.length > 128 || sample.length < value.length
    ? `${escaped.slice(0, 128)}…`
    : escaped;
}

function withLegacyCodeDiagnostic(message: string | null, code: string) {
  const suffix = `(legacy code: ${safeLegacyValue(code)})`;
  return message ? `${message} ${suffix}` : suffix;
}

function withLegacyReasonDiagnostic(message: string | null, reason: string) {
  const suffix = `(legacy reason code: ${safeLegacyValue(reason)})`;
  return message ? `${message} ${suffix}` : suffix;
}

export function normalizeLegacyMediaError(raw: unknown): NormalizedMediaError {
  if (raw === null || raw === undefined) {
    return { errorCode: null, reasonCode: null, diagnostics: null };
  }

  let rawCode: unknown;
  let rawReasonCode: unknown;
  let rawErrorMessage: unknown;
  try {
    if (typeof raw === "object" && raw !== null) {
      const fields = raw as RawMediaErrorFields;
      const isError = raw instanceof Error;
      rawCode = fields.errorCode;
      rawReasonCode = fields.reasonCode;
      rawErrorMessage = isError
        ? (raw as Error).message
        : fields.errorMessage;
    } else {
      rawCode = raw;
      rawReasonCode = undefined;
      rawErrorMessage = undefined;
    }
  } catch {
    return {
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: null,
    };
  }
  const diagnostics = diagnosticMessage(rawErrorMessage);
  const rawCodeIsKnown =
    isMediaOperationErrorCode(rawCode) ||
    isMediaOperationReasonCode(rawCode) ||
    rawCode === "browser_inspection_failed" ||
    rawCode === "inspect-failed" ||
    rawCode === "tool-unavailable";

  if (rawCode !== null && rawCode !== undefined && !rawCodeIsKnown) {
    return {
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: withLegacyCodeDiagnostic(
        null,
        boundedUnknownValue(rawCode),
      ),
    };
  }

  if (
    rawReasonCode !== null &&
    rawReasonCode !== undefined &&
    !isMediaOperationReasonCode(rawReasonCode)
  ) {
    return {
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics: withLegacyReasonDiagnostic(
        null,
        boundedUnknownValue(rawReasonCode),
      ),
    };
  }

  if (isMediaOperationErrorCode(rawCode)) {
    if (
      isMediaOperationReasonCode(rawReasonCode) &&
      rawCode !== "invalid-request" &&
      !(rawCode === "malformed-media" && rawReasonCode === "decode-failed")
    ) {
      return {
        errorCode: "internal-task-failed",
        reasonCode: null,
        diagnostics: withLegacyReasonDiagnostic(
          withLegacyCodeDiagnostic(diagnostics, rawCode),
          rawReasonCode,
        ),
      };
    }
    return {
      errorCode: rawCode,
      reasonCode: isMediaOperationReasonCode(rawReasonCode)
        ? rawReasonCode
        : null,
      diagnostics,
    };
  }

  if (
    (rawCode === null || rawCode === undefined) &&
    isMediaOperationReasonCode(rawReasonCode)
  ) {
    return {
      errorCode: "invalid-request",
      reasonCode: rawReasonCode,
      diagnostics,
    };
  }

  if (isMediaOperationReasonCode(rawCode)) {
    return {
      errorCode: "invalid-request",
      reasonCode: rawCode,
      diagnostics,
    };
  }

  if (rawCode === "browser_inspection_failed" || rawCode === "inspect-failed") {
    return {
      errorCode: "malformed-media",
      reasonCode: "decode-failed",
      diagnostics,
    };
  }

  if (rawCode === "tool-unavailable") {
    return {
      errorCode: "tool-missing",
      reasonCode: null,
      diagnostics,
    };
  }

  if (rawCode === null || rawCode === undefined) {
    if (diagnostics === null) {
      return { errorCode: null, reasonCode: null, diagnostics: null };
    }

    return {
      errorCode: "internal-task-failed",
      reasonCode: null,
      diagnostics,
    };
  }

  const legacyCode = boundedUnknownValue(rawCode);
  return {
    errorCode: "internal-task-failed",
    reasonCode: null,
    diagnostics: withLegacyCodeDiagnostic(diagnostics, legacyCode),
  };
}

export function normalizeLegacyMediaResponse<T extends RawMediaErrorFields>(
  response: T,
) {
  const normalized = normalizeLegacyMediaError(response);
  return {
    ...response,
    errorCode: normalized.errorCode,
    reasonCode: normalized.reasonCode,
    errorMessage: normalized.diagnostics,
  };
}

export function normalizeLegacyOptimizerSearchResponse<
  T extends RawMediaErrorFields & {
    attempts: readonly RawMediaErrorFields[];
  },
>(response: T) {
  return {
    ...normalizeLegacyMediaResponse(response),
    attempts: response.attempts.map((attempt) =>
      normalizeLegacyMediaResponse(attempt),
    ),
  };
}

export function buildWebFileSourceRevision(
  file: Pick<File, "name" | "size" | "lastModified" | "type">,
) {
  const identity = JSON.stringify([
    file.name,
    file.size,
    file.lastModified,
    file.type,
  ]);
  let hash = 0xcbf29ce484222325n;
  for (let index = 0; index < identity.length; index += 1) {
    hash ^= BigInt(identity.charCodeAt(index));
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }

  return `web-${hash.toString(16).padStart(16, "0")}`;
}

export function normalizeInspectionSourceRevision(input: {
  ok: boolean;
  sourceRevision?: unknown;
}): string | null {
  if (!input.ok) {
    return null;
  }

  if (
    typeof input.sourceRevision !== "string" ||
    !input.sourceRevision.trim()
  ) {
    throw new Error(
      "Successful media inspection did not include a source revision.",
    );
  }

  return input.sourceRevision;
}

type RawInspectionFallbackReasonFields = Readonly<{
  fallbackReasonCode?: unknown;
  fallbackReason?: unknown;
  toolDetail?: unknown;
}>;

export function normalizeInspectionFallbackReasonCode(
  input: RawInspectionFallbackReasonFields,
): MediaInspectionFallbackReasonCode | null {
  let fallbackReasonCode: unknown;
  try {
    fallbackReasonCode = input.fallbackReasonCode;
  } catch {
    return null;
  }

  return fallbackReasonCode === "media-foundation-failed"
    ? fallbackReasonCode
    : null;
}

type LegacyMediaResponse<T> = Omit<T, keyof MediaOperationErrorFields> &
  RawMediaErrorFields;

type RuntimeInspectionPayload = Omit<
  LegacyMediaResponse<MediaInspection>,
  | "backendInputPath"
  | "previewSrc"
  | "inputSourceKind"
  | "sourceRevision"
  | "fallbackReasonCode"
> & {
  sourceRevision: string | null;
  fallbackReasonCode?: unknown;
};

type LegacyOptimizerSearchResponse = LegacyMediaResponse<
  Omit<OptimizerSearchResponse, "attempts">
> & {
  attempts: Array<LegacyMediaResponse<SearchAttemptResult>>;
};

export type RuntimeKind = "tauri" | "web";

export type RuntimeInputSource =
  | {
      kind: "tauri-path";
      path: string;
    }
  | {
      kind: "web-file";
      file: File;
    };

export type RuntimeDropHandlers = {
  onDraggingChange: (dragging: boolean) => void;
  onDrop: (source: RuntimeInputSource) => void | Promise<void>;
};

export type RuntimeCapabilities = {
  backendProcessing: boolean;
  outputDirectorySelection: boolean;
  openOutputFolder: boolean;
};

export type MediaOperationOptions = {
  operationId: string;
  signal?: AbortSignal;
  onProgress?: (progress: OperationProgress) => void;
};

export type AppRuntime = {
  kind: RuntimeKind;
  capabilities: RuntimeCapabilities;
  pickInputFile: () => Promise<RuntimeInputSource | null>;
  pickOutputDirectory: () => Promise<string | null>;
  openOutputFolder: (
    path: string | null | undefined,
    fallbackPath: string | null | undefined,
    locale: Locale,
  ) => Promise<void>;
  inspectInput: (
    source: RuntimeInputSource,
    locale: Locale,
    options: MediaOperationOptions,
  ) => Promise<MediaInspection>;
  checkToolHealth: (locale: Locale) => Promise<ToolHealthReport | null>;
  buildOptimizerPlan: (
    request: OptimizerPlanRequest,
    options: MediaOperationOptions,
  ) => Promise<OptimizerPlanResponse>;
  runOptimizerSearch: (
    request: OptimizerSearchRequest,
    options: MediaOperationOptions,
  ) => Promise<OptimizerSearchResponse>;
  convertStaticImageToPng: (
    request: StaticImageConversionRequest,
    options: MediaOperationOptions,
  ) => Promise<StaticImageConversionResult>;
  extractFramePreview: (
    request: FramePreviewRequest,
    options: MediaOperationOptions,
  ) => Promise<FramePreviewResult | null>;
  extractFramePreviews: (
    request: FramePreviewsRequest,
    options: MediaOperationOptions,
  ) => Promise<FramePreviewsResult | null>;
  subscribeInputDrops: (handlers: RuntimeDropHandlers) => Promise<() => void>;
};

type WindowWithTauri = Window & {
  __TAURI_INTERNALS__?: unknown;
};

const DEFAULT_BROWSER_VIDEO_FPS = 12;
const MAX_BROWSER_TIMELINE_FRAMES = 240;
const DESKTOP_ONLY_ERROR =
  "This feature is available only in the desktop app. Use web mode for preview and layout review.";

function isBrowserFileDrag(event: DragEvent) {
  const types = event.dataTransfer?.types;
  if (!types) {
    return false;
  }

  return Array.from(types).includes("Files");
}

function isTauriEnvironment() {
  if (typeof window === "undefined") {
    return false;
  }

  return Boolean((window as WindowWithTauri).__TAURI_INTERNALS__);
}

function revokeIfObjectUrl(value: string) {
  if (value.startsWith("blob:")) {
    URL.revokeObjectURL(value);
  }
}

function pickWebFile(accept: string) {
  return new Promise<File | null>((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.multiple = false;

    input.addEventListener(
      "change",
      () => {
        resolve(input.files?.[0] ?? null);
      },
      { once: true },
    );

    input.click();
  });
}

function createMediaErrorInspection(
  inputPath: string,
  previewSrc: string,
  sourceKind: MediaInspection["inputSourceKind"],
  error: unknown,
): MediaInspection {
  return {
    ok: false,
    inputPath,
    sourceRevision: null,
    backendInputPath: null,
    previewSrc,
    inputSourceKind: sourceKind,
    toolSource: "browser",
    toolCommand: null,
    toolDetail: "Browser metadata inspection failed.",
    fallbackReasonCode: null,
    formatName: null,
    durationSeconds: null,
    sizeBytes: null,
    width: null,
    height: null,
    codecName: null,
    pixelFormat: null,
    avgFps: null,
    frameRateLabel: null,
    estimatedFrames: null,
    frameDurationsSeconds: null,
    warnings: [],
    isStaticImage: true,
    canConvertToPng: false,
    errorCode: "malformed-media",
    reasonCode: "decode-failed",
    errorMessage:
      diagnosticMessage(
        error instanceof Error ? error.message : boundedUnknownValue(error),
      ) ??
      "Browser media inspection failed.",
  };
}

function fileExtension(name: string) {
  const lastDot = name.lastIndexOf(".");
  return lastDot >= 0 ? name.slice(lastDot + 1).toLowerCase() : null;
}

function inferFormatName(file: File) {
  const extension = fileExtension(file.name);
  if (extension) {
    return extension;
  }

  const [, subtype] = file.type.split("/");
  return subtype || "file";
}

function inferCodecName(file: File) {
  const type = file.type.toLowerCase();
  if (!type) {
    return null;
  }

  if (type.startsWith("video/")) {
    return type.slice("video/".length);
  }

  if (type.startsWith("image/")) {
    return type.slice("image/".length);
  }

  return null;
}

function createBrowserVideoInspection(file: File, previewSrc: string, metadata: {
  durationSeconds: number;
  width: number;
  height: number;
}) {
  const estimatedFrames = Math.max(
    1,
    Math.min(
      MAX_BROWSER_TIMELINE_FRAMES,
      Math.round(metadata.durationSeconds * DEFAULT_BROWSER_VIDEO_FPS),
    ),
  );

  return {
    ok: true,
    inputPath: file.name,
    sourceRevision: buildWebFileSourceRevision(file),
    backendInputPath: null,
    previewSrc,
    inputSourceKind: "file",
    toolSource: "browser",
    toolCommand: null,
    toolDetail: `Browser preview mode uses estimated timeline data at ${DEFAULT_BROWSER_VIDEO_FPS} fps.`,
    fallbackReasonCode: null,
    formatName: inferFormatName(file),
    durationSeconds: metadata.durationSeconds,
    sizeBytes: file.size,
    width: metadata.width,
    height: metadata.height,
    codecName: inferCodecName(file),
    pixelFormat: null,
    avgFps: DEFAULT_BROWSER_VIDEO_FPS,
    frameRateLabel: `${DEFAULT_BROWSER_VIDEO_FPS}`,
    estimatedFrames,
    frameDurationsSeconds: null,
    warnings: [],
    isStaticImage: false,
    canConvertToPng: false,
    errorCode: null,
    reasonCode: null,
    errorMessage: null,
  } satisfies MediaInspection;
}

function createBrowserImageInspection(file: File, previewSrc: string, metadata: {
  width: number;
  height: number;
}) {
  return {
    ok: true,
    inputPath: file.name,
    sourceRevision: buildWebFileSourceRevision(file),
    backendInputPath: null,
    previewSrc,
    inputSourceKind: "file",
    toolSource: "browser",
    toolCommand: null,
    toolDetail: "Browser preview mode supports crop and layout review for local image files.",
    fallbackReasonCode: null,
    formatName: inferFormatName(file),
    durationSeconds: null,
    sizeBytes: file.size,
    width: metadata.width,
    height: metadata.height,
    codecName: inferCodecName(file),
    pixelFormat: null,
    avgFps: null,
    frameRateLabel: null,
    estimatedFrames: null,
    frameDurationsSeconds: null,
    warnings: [],
    isStaticImage: true,
    canConvertToPng: false,
    errorCode: null,
    reasonCode: null,
    errorMessage: null,
  } satisfies MediaInspection;
}

function loadVideoMetadata(previewSrc: string) {
  return new Promise<{ durationSeconds: number; width: number; height: number }>(
    (resolve, reject) => {
      const video = document.createElement("video");
      video.preload = "metadata";
      video.muted = true;
      video.playsInline = true;

      const cleanup = () => {
        video.src = "";
      };

      video.onloadedmetadata = () => {
        resolve({
          durationSeconds: Number.isFinite(video.duration) ? video.duration : 0,
          width: video.videoWidth,
          height: video.videoHeight,
        });
        cleanup();
      };
      video.onerror = () => {
        reject(new Error("Unable to read video metadata in the browser."));
        cleanup();
      };
      video.src = previewSrc;
    },
  );
}

function loadImageMetadata(previewSrc: string) {
  return new Promise<{ width: number; height: number }>((resolve, reject) => {
    const image = new Image();
    image.decoding = "async";
    image.onload = () => {
      resolve({
        width: image.naturalWidth,
        height: image.naturalHeight,
      });
    };
    image.onerror = () => reject(new Error("Unable to read image metadata in the browser."));
    image.src = previewSrc;
  });
}

async function inspectWebFile(file: File) {
  const previewSrc = URL.createObjectURL(file);
  const isVideo = file.type.startsWith("video/");

  try {
    if (isVideo) {
      const metadata = await loadVideoMetadata(previewSrc);
      return createBrowserVideoInspection(file, previewSrc, metadata);
    }

    const metadata = await loadImageMetadata(previewSrc);
    return createBrowserImageInspection(file, previewSrc, metadata);
  } catch (error) {
    return createMediaErrorInspection(file.name, previewSrc, "file", error);
  }
}

async function loadTauriCore() {
  return import("@tauri-apps/api/core");
}

type DesktopMediaOperationBridge = {
  invoke: <T>(
    command: string,
    args?: Record<string, unknown>,
  ) => Promise<T>;
  createChannel: (
    onMessage: (progress: OperationProgress) => void,
  ) => unknown;
};

export async function invokeDesktopMediaOperation<T>(
  command: string,
  args: Record<string, unknown>,
  options: MediaOperationOptions,
  bridge?: DesktopMediaOperationBridge,
) {
  const resolvedBridge =
    bridge ??
    (await (async (): Promise<DesktopMediaOperationBridge> => {
      const { Channel, invoke } = await loadTauriCore();
      return {
        invoke: <TResult>(
          nextCommand: string,
          nextArgs?: Record<string, unknown>,
        ) => invoke<TResult>(nextCommand, nextArgs),
        createChannel: (onMessage) =>
          new Channel<OperationProgress>(onMessage),
      };
    })());
  const channel = resolvedBridge.createChannel((progress) => {
    if (
      options.signal?.aborted ||
      progress.operationId !== options.operationId
    ) {
      return;
    }
    options.onProgress?.(progress);
  });
  let cancellation: Promise<void> | null = null;
  const requestCancellation = () => {
    if (cancellation === null) {
      cancellation = resolvedBridge
        .invoke<unknown>("cancel_media_operation", {
          operationId: options.operationId,
        })
        .then(
          () => undefined,
          () => undefined,
        );
    }
    return cancellation;
  };
  const handleAbort = () => {
    void requestCancellation();
  };

  options.signal?.addEventListener("abort", handleAbort, { once: true });
  try {
    if (options.signal?.aborted) {
      await requestCancellation();
    }
    return await resolvedBridge.invoke<T>(command, {
      ...args,
      operationId: options.operationId,
      onProgress: channel,
    });
  } finally {
    options.signal?.removeEventListener("abort", handleAbort);
  }
}

async function loadTauriDialog() {
  return import("@tauri-apps/plugin-dialog");
}

async function loadTauriWindow() {
  return import("@tauri-apps/api/window");
}

const tauriRuntime: AppRuntime = {
  kind: "tauri",
  capabilities: {
    backendProcessing: true,
    outputDirectorySelection: true,
    openOutputFolder: true,
  },
  async pickInputFile() {
    const { open } = await loadTauriDialog();
    const selected = await open({
      directory: false,
      multiple: false,
      filters: [
        {
          name: "Media",
          extensions: [
            "mp4",
            "gif",
            "webm",
            "mov",
            "m4v",
            "apng",
            "png",
            "jpg",
            "jpeg",
            "bmp",
          ],
        },
      ],
    });

    if (!selected || Array.isArray(selected)) {
      return null;
    }

    return {
      kind: "tauri-path",
      path: selected,
    } satisfies RuntimeInputSource;
  },
  async pickOutputDirectory() {
    const { open } = await loadTauriDialog();
    const selected = await open({ directory: true, multiple: false });
    if (!selected || Array.isArray(selected)) {
      return null;
    }

    return selected;
  },
  async openOutputFolder(path, fallbackPath, locale) {
    const { invoke } = await loadTauriCore();
    await invoke("open_folder_path", {
      path: path ?? fallbackPath ?? null,
      locale,
    });
  },
  async inspectInput(source, locale, options) {
    if (source.kind !== "tauri-path") {
      throw new Error("Expected a desktop file path.");
    }

    const { convertFileSrc } = await loadTauriCore();
    const result = await invokeDesktopMediaOperation<RuntimeInspectionPayload>(
      "inspect_input_media",
      {
        inputPath: source.path,
        locale,
      },
      options,
    );
    const normalized = normalizeLegacyMediaResponse(result);

    return {
      ...normalized,
      inputPath: source.path,
      sourceRevision: normalizeInspectionSourceRevision(result),
      fallbackReasonCode: normalizeInspectionFallbackReasonCode(result),
      backendInputPath: source.path,
      previewSrc: convertFileSrc(source.path),
      inputSourceKind: "path",
    };
  },
  async checkToolHealth(locale) {
    const { invoke } = await loadTauriCore();
    return invoke<ToolHealthReport>("check_media_tools", {
      locale,
    });
  },
  async buildOptimizerPlan(request, options) {
    const result = await invokeDesktopMediaOperation<
      LegacyMediaResponse<OptimizerPlanResponse>
    >(
      "build_optimizer_plan",
      { request },
      options,
    );
    return normalizeLegacyMediaResponse(result);
  },
  async runOptimizerSearch(request, options) {
    const result = await invokeDesktopMediaOperation<LegacyOptimizerSearchResponse>(
      "run_optimizer_search",
      { request },
      options,
    );
    return normalizeLegacyOptimizerSearchResponse(
      result,
    ) as OptimizerSearchResponse;
  },
  async convertStaticImageToPng(request, options) {
    const result = await invokeDesktopMediaOperation<
      LegacyMediaResponse<StaticImageConversionResult>
    >(
      "convert_static_image_to_png",
      { request },
      options,
    );
    return normalizeLegacyMediaResponse(result);
  },
  async extractFramePreview(request, options) {
    const result = await invokeDesktopMediaOperation<
      LegacyMediaResponse<FramePreviewResult>
    >(
      "extract_frame_preview",
      {
        inputPath: request.inputPath,
        sourceRevision: request.sourceRevision,
        sourceFrameId: request.sourceFrameId,
        sourceWidth: request.sourceWidth,
        sourceHeight: request.sourceHeight,
        locale: request.locale,
      },
      options,
    );
    return normalizeLegacyMediaResponse(result);
  },
  async extractFramePreviews(request, options) {
    const result = await invokeDesktopMediaOperation<
      LegacyMediaResponse<FramePreviewsResult>
    >(
      "extract_frame_previews",
      {
        inputPath: request.inputPath,
        sourceRevision: request.sourceRevision,
        sourceFrameIds: request.sourceFrameIds,
        sourceWidth: request.sourceWidth,
        sourceHeight: request.sourceHeight,
        locale: request.locale,
      },
      options,
    );
    return normalizeLegacyMediaResponse(result);
  },
  async subscribeInputDrops(handlers) {
    let isFileDragActive = false;
    const { getCurrentWindow } = await loadTauriWindow();
    const unlisten = await getCurrentWindow().onDragDropEvent(async (event) => {
      if (event.payload.type === "enter") {
        isFileDragActive = event.payload.paths.length > 0;
        handlers.onDraggingChange(isFileDragActive);
        return;
      }

      if (event.payload.type === "over") {
        if (isFileDragActive) {
          handlers.onDraggingChange(true);
        }
        return;
      }

      if (event.payload.type === "leave") {
        isFileDragActive = false;
        handlers.onDraggingChange(false);
        return;
      }

      if (event.payload.type === "drop") {
        isFileDragActive = false;
        handlers.onDraggingChange(false);
        const droppedPath = event.payload.paths[0];
        if (!droppedPath) {
          return;
        }

        await handlers.onDrop({
          kind: "tauri-path",
          path: droppedPath,
        });
      }
    });

    return () => {
      unlisten();
    };
  },
};

const webRuntime: AppRuntime = {
  kind: "web",
  capabilities: {
    backendProcessing: false,
    outputDirectorySelection: false,
    openOutputFolder: false,
  },
  async pickInputFile() {
    const file = await pickWebFile(
      "video/mp4,video/webm,video/quicktime,image/png,image/apng,image/gif,image/jpeg,image/bmp",
    );
    if (!file) {
      return null;
    }

    return {
      kind: "web-file",
      file,
    } satisfies RuntimeInputSource;
  },
  async pickOutputDirectory() {
    return null;
  },
  async openOutputFolder(_path, _fallbackPath, _locale) {
    return;
  },
  async inspectInput(source, _locale, _options) {
    if (source.kind !== "web-file") {
      throw new Error("Expected a browser File object.");
    }

    return inspectWebFile(source.file);
  },
  async checkToolHealth(_locale) {
    return null;
  },
  async buildOptimizerPlan(_request, _options) {
    throw new Error(DESKTOP_ONLY_ERROR);
  },
  async runOptimizerSearch(_request, _options) {
    throw new Error(DESKTOP_ONLY_ERROR);
  },
  async convertStaticImageToPng(_request, _options) {
    throw new Error(DESKTOP_ONLY_ERROR);
  },
  async extractFramePreview(_request, _options) {
    return null;
  },
  async extractFramePreviews(_request, _options) {
    return null;
  },
  async subscribeInputDrops(handlers) {
    let dragDepth = 0;

    const handleDragEnter = (event: DragEvent) => {
      if (!isBrowserFileDrag(event)) {
        return;
      }
      event.preventDefault();
      dragDepth += 1;
      handlers.onDraggingChange(true);
    };
    const handleDragOver = (event: DragEvent) => {
      if (!isBrowserFileDrag(event)) {
        return;
      }
      event.preventDefault();
      handlers.onDraggingChange(true);
    };
    const handleDragLeave = (event: DragEvent) => {
      if (!isBrowserFileDrag(event)) {
        return;
      }
      event.preventDefault();
      dragDepth = Math.max(0, dragDepth - 1);
      if (dragDepth === 0) {
        handlers.onDraggingChange(false);
      }
    };
    const handleDrop = async (event: DragEvent) => {
      if (!isBrowserFileDrag(event)) {
        return;
      }
      event.preventDefault();
      dragDepth = 0;
      handlers.onDraggingChange(false);
      const file = event.dataTransfer?.files?.[0] ?? null;
      if (!file) {
        return;
      }

      await handlers.onDrop({
        kind: "web-file",
        file,
      });
    };

    window.addEventListener("dragenter", handleDragEnter);
    window.addEventListener("dragover", handleDragOver);
    window.addEventListener("dragleave", handleDragLeave);
    window.addEventListener("drop", handleDrop);

    return () => {
      window.removeEventListener("dragenter", handleDragEnter);
      window.removeEventListener("dragover", handleDragOver);
      window.removeEventListener("dragleave", handleDragLeave);
      window.removeEventListener("drop", handleDrop);
    };
  },
};

let cachedRuntime: AppRuntime | null = null;

export function getAppRuntime() {
  if (!cachedRuntime) {
    cachedRuntime = isTauriEnvironment() ? tauriRuntime : webRuntime;
  }

  return cachedRuntime;
}

export function releaseInspectionPreview(inspection: MediaInspection | null) {
  if (!inspection || inspection.inputSourceKind !== "file") {
    return;
  }

  revokeIfObjectUrl(inspection.previewSrc);
}
