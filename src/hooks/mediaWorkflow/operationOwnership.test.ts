/// <reference types="vite/client" />

import { describe, expect, it } from "vitest";

import appSource from "../../App.tsx?raw";
import runtimeSource from "../../platform/runtime.ts?raw";
import controllerSource from "../useMediaWorkflowController.ts?raw";
import selectorSource from "./useMediaInputSelector.ts?raw";

function sourceBlock(source: string, startMarker: string, endMarker: string) {
  const start = source.indexOf(startMarker);
  expect(start, `missing start marker: ${startMarker}`).toBeGreaterThanOrEqual(
    0,
  );
  const end = source.indexOf(endMarker, start + startMarker.length);
  expect(end, `missing end marker: ${endMarker}`).toBeGreaterThan(start);
  return source.slice(start, end);
}

describe("media operation AbortController ownership", () => {
  it("keeps separate plan, search, and conversion owners in the workflow controller", () => {
    expect(controllerSource).toContain("planAbortControllerRef");
    expect(controllerSource).toContain("searchAbortControllerRef");
    expect(controllerSource).toContain("conversionAbortControllerRef");
    expect(controllerSource).not.toContain("operationIdRef");
  });

  it("replaces same-kind plan ownership and aborts the search invalidated by a plan", () => {
    const block = sourceBlock(
      controllerSource,
      "const buildPlan = useCallback",
      "const runBoundedSearch = useCallback",
    );
    expect(block).toContain("planAbortControllerRef.current?.abort()");
    expect(
      block.indexOf("planAbortControllerRef.current?.abort()"),
    ).toBeLessThan(block.indexOf("const controller = new AbortController()"));
    expect(block).toContain("searchAbortControllerRef.current?.abort()");
    expect(block).toContain("const operationId = createMediaOperationId()");
    expect(block).toMatch(
      /buildOptimizerPlan\(request,\s*\{[\s\S]*?operationId,[\s\S]*?signal: controller\.signal/,
    );
    expect(block).toContain("onProgress:");
    expect(block).toContain("isCurrentProgressUpdate");
    expect(block).toMatch(
      /if \(planAbortControllerRef\.current === controller\) \{\s*planAbortControllerRef\.current = null;\s*\}/,
    );
  });

  it("replaces same-kind search ownership and gates progress before exact-owner cleanup", () => {
    const block = sourceBlock(
      controllerSource,
      "const runBoundedSearch = useCallback",
      "const cancelOptimizerSearch = useCallback",
    );
    expect(block).toContain("searchAbortControllerRef.current?.abort()");
    expect(block).toContain("planAbortControllerRef.current?.abort()");
    expect(block).toContain("planTicketRef.current += 1");
    expect(
      block.indexOf("planAbortControllerRef.current?.abort()"),
    ).toBeLessThan(block.indexOf("const controller = new AbortController()"));
    expect(
      block.indexOf("searchAbortControllerRef.current?.abort()"),
    ).toBeLessThan(block.indexOf("const controller = new AbortController()"));
    expect(block).toContain("const operationId = createMediaOperationId()");
    expect(block).toMatch(
      /runOptimizerSearch\(request,\s*\{[\s\S]*?operationId,[\s\S]*?signal: controller\.signal/,
    );
    expect(block).toContain("onProgress:");
    expect(block).toContain("isCurrentProgressUpdate");
    expect(block).toMatch(
      /if \(searchAbortControllerRef\.current === controller\) \{\s*searchAbortControllerRef\.current = null;\s*\}/,
    );
  });

  it("replaces same-kind conversion ownership and cleans only the exact owner", () => {
    const block = sourceBlock(
      controllerSource,
      "const convertStaticImageToPng = useCallback",
      "    runtime,",
    );
    expect(block).toContain("conversionAbortControllerRef.current?.abort()");
    expect(
      block.indexOf("conversionAbortControllerRef.current?.abort()"),
    ).toBeLessThan(block.indexOf("const controller = new AbortController()"));
    expect(block).toContain("const operationId = createMediaOperationId()");
    expect(block).toMatch(
      /convertStaticImageToPng\(request,\s*\{[\s\S]*?operationId,[\s\S]*?signal: controller\.signal/,
    );
    expect(block).toContain("onProgress:");
    expect(block).toContain("isCurrentProgressUpdate");
    expect(block).toMatch(
      /if \(conversionAbortControllerRef\.current === controller\) \{\s*conversionAbortControllerRef\.current = null;\s*\}/,
    );
  });

  it("gates progress by owner, ticket, fingerprint, mount, and loading revision", () => {
    const block = sourceBlock(
      controllerSource,
      "const isCurrentProgressUpdate = useCallback",
      "const resetForNewInspection = useCallback",
    );
    expect(block).toContain("mountedRef.current");
    expect(block).toContain("controllerRef.current === controller");
    expect(block).toContain("isCurrentWorkflowRequest(");
    expect(block).toContain("ticket");
    expect(block).toContain("currentTicket");
    expect(block).toContain("getCurrentWorkflowFingerprints()[kind]");

    for (const [start, end, stateName] of [
      [
        "const buildPlan = useCallback",
        "const runBoundedSearch = useCallback",
        "setPlanState",
      ],
      [
        "const runBoundedSearch = useCallback",
        "const cancelOptimizerSearch = useCallback",
        "setSearchState",
      ],
      [
        "const convertStaticImageToPng = useCallback",
        "    runtime,",
        "setConversionState",
      ],
    ] as const) {
      const operation = sourceBlock(controllerSource, start, end);
      const callbackStart = operation.indexOf(`${stateName}((current) =>`);
      expect(
        callbackStart,
        `${stateName} progress callback`,
      ).toBeGreaterThanOrEqual(0);
      const callback = operation.slice(callbackStart);
      expect(callback).toContain('current.status === "loading"');
      expect(callback).toContain("current.revision === stateRevision");
      expect(callback).toContain("current.fingerprint === fingerprint");
      expect(callback).toMatch(
        /\?\s*\{\s*\.\.\.current,\s*progress,?\s*\}\s*:\s*current/,
      );
    }
  });

  it("aborts matching refs on fingerprint invalidation, forced reset, and unmount", () => {
    const invalidation = sourceBlock(
      controllerSource,
      "const invalidateWorkflowResults = useCallback",
      "const isCurrentOperation = useCallback",
    );
    expect(invalidation).toContain("planAbortControllerRef.current?.abort()");
    expect(invalidation).toContain("searchAbortControllerRef.current?.abort()");
    expect(invalidation).toContain(
      "conversionAbortControllerRef.current?.abort()",
    );

    const reset = sourceBlock(
      controllerSource,
      "const resetForNewInspection = useCallback",
      "const { toolReport",
    );
    expect(reset).toContain("invalidateWorkflowResults(");
    expect(reset).toContain("true");

    const unmount = sourceBlock(
      controllerSource,
      "mountedRef.current = true",
      "const buildPlan = useCallback",
    );
    expect(unmount).toContain("mountedRef.current = false");
    expect(unmount).toContain("planAbortControllerRef.current?.abort()");
    expect(unmount).toContain("searchAbortControllerRef.current?.abort()");
    expect(unmount).toContain("conversionAbortControllerRef.current?.abort()");
  });

  it("aborts search without advancing its freshness ticket before canonical settlement", () => {
    const block = sourceBlock(
      controllerSource,
      "const cancelOptimizerSearch = useCallback",
      "const convertStaticImageToPng = useCallback",
    );
    expect(block).toContain("searchAbortControllerRef.current?.abort()");
    expect(block).not.toContain("searchTicketRef.current += 1");
  });

  it("owns inspection replacement, options, exact cleanup, and unmount invalidation", () => {
    const inspect = sourceBlock(
      selectorSource,
      "const inspectSource = useCallback",
      "useEffect(() =>",
    );
    expect(inspect).toContain("inspectionAbortControllerRef.current?.abort()");
    expect(
      inspect.indexOf("inspectionAbortControllerRef.current?.abort()"),
    ).toBeLessThan(inspect.indexOf("const controller = new AbortController()"));
    expect(inspect).toContain("const operationId = createMediaOperationId()");
    expect(inspect).toMatch(
      /runtime\.inspectInput\(source, locale, \{[\s\S]*?operationId,[\s\S]*?signal: controller\.signal/,
    );
    expect(inspect).toMatch(
      /if \(inspectionAbortControllerRef\.current === controller\) \{\s*inspectionAbortControllerRef\.current = null;\s*\}/,
    );

    const invalidateIndex = selectorSource.indexOf(
      "requestLifecycle.invalidate()",
    );
    expect(invalidateIndex).toBeGreaterThanOrEqual(0);
    const unmount = selectorSource.slice(
      Math.max(0, invalidateIndex - 200),
      selectorSource.indexOf("const pickInputFile", invalidateIndex),
    );
    expect(unmount).toContain("inspectionAbortControllerRef.current?.abort()");
  });

  it("keeps the editor-session commit callback stable across ordinary App renders", () => {
    const callback = sourceBlock(
      appSource,
      "const handleCommitEditorSession = useCallback",
      "const mediaWorkflow = useMediaWorkflowController",
    );
    expect(callback).toContain("setEditorSessionKey((current) => current + 1)");
    expect(callback).toMatch(/\}, \[\]\);\s*$/);

    const wiring = sourceBlock(
      appSource,
      "const mediaWorkflow = useMediaWorkflowController",
      "const {",
    );
    expect(wiring).toContain(
      "onCommitEditorSession: handleCommitEditorSession",
    );
    expect(wiring).not.toMatch(/onCommitEditorSession:\s*\(\)\s*=>/);
    expect(selectorSource).toContain("}, [inspectSource, runtime]);");
  });

  it("keeps App preview request IDs and effect-owned operation cancellation", () => {
    const effect = sourceBlock(
      appSource,
      "const request = buildFramePreviewsRequest",
      "const backendInputPath = inspection?.backendInputPath",
    );
    expect(effect).toContain("framePreviewRequestIdRef");
    expect(effect).toContain("const controller = new AbortController()");
    expect(effect).toContain("const operationId = createMediaOperationId()");
    expect(effect).toMatch(
      /extractFramePreviews\(request, \{[\s\S]*?operationId,[\s\S]*?signal: controller\.signal/,
    );
    expect(effect).toContain("window.clearTimeout(timeoutId)");
    expect(effect).toContain("controller.abort()");
  });

  it("routes every desktop media method through the Channel adapter without minting IDs", () => {
    const adapter = sourceBlock(
      runtimeSource,
      "export async function invokeDesktopMediaOperation",
      "const tauriRuntime: AppRuntime",
    );
    expect(adapter).toContain("new Channel<OperationProgress>(onMessage)");
    expect(adapter).toContain("onProgress: channel");

    const desktop = sourceBlock(
      runtimeSource,
      "const tauriRuntime: AppRuntime",
      "const webRuntime: AppRuntime",
    );
    expect(desktop.match(/invokeDesktopMediaOperation/g)).toHaveLength(6);

    const expectCentralCommand = (block: string, command: string) => {
      expect(block.match(/invokeDesktopMediaOperation/g)).toHaveLength(1);
      expect(block).toContain(`"${command}"`);
      expect(block).not.toMatch(/\binvoke</);
    };

    const inspect = sourceBlock(
      desktop,
      "async inspectInput(",
      "async checkToolHealth(",
    );
    expectCentralCommand(inspect, "inspect_input_media");
    expect(inspect).toMatch(
      /"inspect_input_media",\s*\{\s*inputPath: source\.path,\s*locale,\s*\},\s*options,?\s*\)/,
    );

    const plan = sourceBlock(
      desktop,
      "async buildOptimizerPlan(",
      "async runOptimizerSearch(",
    );
    expectCentralCommand(plan, "build_optimizer_plan");
    expect(plan).toMatch(
      /"build_optimizer_plan",\s*\{ request \},\s*options,?\s*\)/,
    );

    const search = sourceBlock(
      desktop,
      "async runOptimizerSearch(",
      "async convertStaticImageToPng(",
    );
    expectCentralCommand(search, "run_optimizer_search");
    expect(search).toMatch(
      /"run_optimizer_search",\s*\{ request \},\s*options,?\s*\)/,
    );

    const conversion = sourceBlock(
      desktop,
      "async convertStaticImageToPng(",
      "async extractFramePreview(",
    );
    expectCentralCommand(conversion, "convert_static_image_to_png");
    expect(conversion).toMatch(
      /"convert_static_image_to_png",\s*\{ request \},\s*options,?\s*\)/,
    );

    const preview = sourceBlock(
      desktop,
      "async extractFramePreview(",
      "async extractFramePreviews(",
    );
    expectCentralCommand(preview, "extract_frame_preview");
    expect(preview).toMatch(
      /"extract_frame_preview",\s*\{\s*inputPath: request\.inputPath,\s*sourceRevision: request\.sourceRevision,\s*sourceFrameId: request\.sourceFrameId,\s*sourceWidth: request\.sourceWidth,\s*sourceHeight: request\.sourceHeight,\s*locale: request\.locale,\s*\},\s*options,?\s*\)/,
    );

    const previews = sourceBlock(
      desktop,
      "async extractFramePreviews(",
      "async subscribeInputDrops(",
    );
    expectCentralCommand(previews, "extract_frame_previews");
    expect(previews).toMatch(
      /"extract_frame_previews",\s*\{\s*inputPath: request\.inputPath,\s*sourceRevision: request\.sourceRevision,\s*sourceFrameIds: request\.sourceFrameIds,\s*sourceWidth: request\.sourceWidth,\s*sourceHeight: request\.sourceHeight,\s*locale: request\.locale,\s*\},\s*options,?\s*\)/,
    );

    const folder = sourceBlock(
      desktop,
      "async openOutputFolder(",
      "async inspectInput(",
    );
    const health = sourceBlock(
      desktop,
      "async checkToolHealth(",
      "async buildOptimizerPlan(",
    );
    expect(folder).not.toContain("invokeDesktopMediaOperation");
    expect(health).not.toContain("invokeDesktopMediaOperation");
    expect(runtimeSource).not.toContain("createMediaOperationId");
  });

  it("keeps cancelled workflow state out of the red planner error channel", () => {
    const plannerError = sourceBlock(
      appSource,
      "const plannerError =",
      "useEffect(() =>",
    );
    expect(plannerError).toContain('latestWorkflowState?.status === "error"');
    expect(plannerError).not.toContain('status === "cancelled"');
    expect(plannerError).not.toContain(
      'mediaOperationMessage(locale, "cancelled"',
    );
  });

  it("returns result envelopes only for canonical ready settlements", () => {
    expect(
      controllerSource.match(/return nextState\.status === "ready"/g),
    ).toHaveLength(3);
    expect(controllerSource).not.toContain(
      'return nextState.status === "cancelled"',
    );
  });
});
