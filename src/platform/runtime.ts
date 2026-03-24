import type { Locale } from "../locales/messages";
import type {
  MediaInspection,
  OptimizerPlanRequest,
  OptimizerPlanResponse,
  OptimizerSearchRequest,
  OptimizerSearchResponse,
  StaticImageConversionRequest,
  StaticImageConversionResult,
  ToolHealthReport,
} from "../types/workflow";

type RuntimeInspectionPayload = Omit<
  MediaInspection,
  "backendInputPath" | "previewSrc" | "inputSourceKind"
>;

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
  inspectInput: (source: RuntimeInputSource, locale: Locale) => Promise<MediaInspection>;
  checkToolHealth: (locale: Locale) => Promise<ToolHealthReport | null>;
  buildOptimizerPlan: (request: OptimizerPlanRequest) => Promise<OptimizerPlanResponse>;
  runOptimizerSearch: (request: OptimizerSearchRequest) => Promise<OptimizerSearchResponse>;
  convertStaticImageToPng: (
    request: StaticImageConversionRequest,
  ) => Promise<StaticImageConversionResult>;
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
    backendInputPath: null,
    previewSrc,
    inputSourceKind: sourceKind,
    toolSource: "browser",
    toolCommand: null,
    toolDetail: "Browser metadata inspection failed.",
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
    isStaticImage: true,
    canConvertToPng: false,
    errorCode: "browser_inspection_failed",
    errorMessage: error instanceof Error ? error.message : String(error),
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
    backendInputPath: null,
    previewSrc,
    inputSourceKind: "file",
    toolSource: "browser",
    toolCommand: null,
    toolDetail: `Browser preview mode uses estimated timeline data at ${DEFAULT_BROWSER_VIDEO_FPS} fps.`,
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
    isStaticImage: false,
    canConvertToPng: false,
    errorCode: null,
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
    backendInputPath: null,
    previewSrc,
    inputSourceKind: "file",
    toolSource: "browser",
    toolCommand: null,
    toolDetail: "Browser preview mode supports crop and layout review for local image files.",
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
    isStaticImage: true,
    canConvertToPng: false,
    errorCode: null,
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
  async inspectInput(source, locale) {
    if (source.kind !== "tauri-path") {
      throw new Error("Expected a desktop file path.");
    }

    const [{ invoke, convertFileSrc }] = await Promise.all([loadTauriCore()]);
    const result = await invoke<RuntimeInspectionPayload>("inspect_input_media", {
      inputPath: source.path,
      locale,
    });

    return {
      ...result,
      inputPath: source.path,
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
  async buildOptimizerPlan(request) {
    const { invoke } = await loadTauriCore();
    return invoke<OptimizerPlanResponse>("build_optimizer_plan", {
      request,
    });
  },
  async runOptimizerSearch(request) {
    const { invoke } = await loadTauriCore();
    return invoke<OptimizerSearchResponse>("run_optimizer_search", {
      request,
    });
  },
  async convertStaticImageToPng(request) {
    const { invoke } = await loadTauriCore();
    return invoke<StaticImageConversionResult>("convert_static_image_to_png", {
      request,
    });
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
  async inspectInput(source, _locale) {
    if (source.kind !== "web-file") {
      throw new Error("Expected a browser File object.");
    }

    return inspectWebFile(source.file);
  },
  async checkToolHealth(_locale) {
    return null;
  },
  async buildOptimizerPlan(_request) {
    throw new Error(DESKTOP_ONLY_ERROR);
  },
  async runOptimizerSearch(_request) {
    throw new Error(DESKTOP_ONLY_ERROR);
  },
  async convertStaticImageToPng(_request) {
    throw new Error(DESKTOP_ONLY_ERROR);
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
