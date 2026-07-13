import { describe, expect, it } from "vitest";

import { FULL_CROP_REGION } from "../../components/MediaSelectionPreview";
import {
  buildWorkflowFingerprints,
  currentWorkflowState,
  isCurrentWorkflowRequest,
  latestActiveWorkflowState,
  workflowStateFromResult,
  type VersionedWorkflowState,
  type WorkflowFingerprintInput,
} from "./workflowFingerprint";

function input(
  overrides: Partial<WorkflowFingerprintInput> = {},
): WorkflowFingerprintInput {
  return {
    editorSessionKey: 3,
    locale: "en",
    inputPath: "C:/media/sticker.gif",
    sourceRevision: null,
    outputDirectory: "C:/output",
    sourceDurationSeconds: 2.5,
    inputWidth: 640,
    inputHeight: 320,
    avgFps: 24,
    optimizerPresetStrategy: "auto",
    optimizerGoal: "balanced",
    qualityFrameDropInterval: 3,
    optimizerSearchDepth: "standard",
    cropRegion: { ...FULL_CROP_REGION },
    baseFrameCount: 2,
    timelineFrames: [
      { sourceFrameId: 1, durationUs: 100_000 },
      { sourceFrameId: 2, durationUs: 200_000 },
      { sourceFrameId: 1, durationUs: 300_000 },
    ],
    ...overrides,
  };
}

describe("workflow fingerprints", () => {
  it("is stable for structurally equal inputs", () => {
    expect(buildWorkflowFingerprints(input())).toEqual(
      buildWorkflowFingerprints(
        input({
          cropRegion: { ...FULL_CROP_REGION },
          timelineFrames: input().timelineFrames?.map((frame) => ({
            ...frame,
          })),
        }),
      ),
    );
  });

  it("changes encoding when timeline order changes while preserving duplicates", () => {
    const original = input();
    const reordered = input({
      timelineFrames: [
        original.timelineFrames![1],
        original.timelineFrames![0],
        original.timelineFrames![2],
      ],
    });

    expect(buildWorkflowFingerprints(reordered).encoding).not.toBe(
      buildWorkflowFingerprints(original).encoding,
    );
  });

  it("changes encoding when a duplicate frame duration changes", () => {
    const changed = input({
      timelineFrames: [
        { sourceFrameId: 1, durationUs: 100_000 },
        { sourceFrameId: 2, durationUs: 200_000 },
        { sourceFrameId: 1, durationUs: 300_001 },
      ],
    });

    expect(buildWorkflowFingerprints(changed).encoding).not.toBe(
      buildWorkflowFingerprints(input()).encoding,
    );
  });

  it("changes only export when output directory changes", () => {
    const before = buildWorkflowFingerprints(input());
    const after = buildWorkflowFingerprints(
      input({ outputDirectory: "D:/other" }),
    );

    expect(after.encoding).toBe(before.encoding);
    expect(after.planner).toBe(before.planner);
    expect(after.export).not.toBe(before.export);
  });

  it("changes planner and export but not encoding when locale changes", () => {
    const before = buildWorkflowFingerprints(input());
    const after = buildWorkflowFingerprints(input({ locale: "ko" }));

    expect(after.encoding).toBe(before.encoding);
    expect(after.planner).not.toBe(before.planner);
    expect(after.export).not.toBe(before.export);
  });

  it("keeps a null source revision stable and changes encoding when it appears or changes", () => {
    const missing = buildWorkflowFingerprints(input({ sourceRevision: null }));
    const missingAgain = buildWorkflowFingerprints(
      input({ sourceRevision: null }),
    );
    const first = buildWorkflowFingerprints(
      input({ sourceRevision: "revision-1" }),
    );
    const second = buildWorkflowFingerprints(
      input({ sourceRevision: "revision-2" }),
    );

    expect(missingAgain.encoding).toBe(missing.encoding);
    expect(first.encoding).not.toBe(missing.encoding);
    expect(second.encoding).not.toBe(first.encoding);
  });

  it("ignores zoom and dock-only values", () => {
    const base = input();
    const withPresentationOnlyValues = {
      ...base,
      previewZoomScale: 2,
      activeDockPanel: "results",
    } as WorkflowFingerprintInput;

    expect(buildWorkflowFingerprints(withPresentationOnlyValues)).toEqual(
      buildWorkflowFingerprints(base),
    );
  });
});

describe("currentWorkflowState", () => {
  it("hides a ready plan immediately after the crop fingerprint changes", () => {
    const before = buildWorkflowFingerprints(input());
    const after = buildWorkflowFingerprints(
      input({ cropRegion: { x: 0.1, y: 0.1, width: 0.8, height: 0.8 } }),
    );
    const state: VersionedWorkflowState<{ id: string }> = {
      status: "ready",
      revision: 4,
      fingerprint: before.planner,
      value: { id: "stale-plan" },
    };

    expect(currentWorkflowState(state, after.planner)).toBeNull();
  });

  it("preserves the loading progress generic without copying it", () => {
    type OperationProgress = Readonly<{
      phase: "encoding";
      completed: number;
      total: number;
    }>;
    const progress: OperationProgress = {
      phase: "encoding",
      completed: 2,
      total: 5,
    };
    const state: VersionedWorkflowState<never, OperationProgress> = {
      status: "loading",
      revision: 5,
      fingerprint: "current",
      progress,
    };
    const current = currentWorkflowState(state, "current");

    expect(current?.status).toBe("loading");
    if (current?.status === "loading") {
      expect(current.progress).toBe(progress);
    }
  });

  it("gates idle, loading, ready, error, and cancelled states by fingerprint", () => {
    const states: Array<
      VersionedWorkflowState<{ id: string }, { step: number }>
    > = [
      { status: "idle", revision: 1, fingerprint: "current" },
      {
        status: "loading",
        revision: 2,
        fingerprint: "current",
        progress: { step: 1 },
      },
      {
        status: "ready",
        revision: 3,
        fingerprint: "current",
        value: { id: "value" },
      },
      {
        status: "error",
        revision: 4,
        fingerprint: "current",
        code: "invalid-request",
        reasonCode: "invalid-crop",
        message: "diagnostics",
      },
      { status: "cancelled", revision: 5, fingerprint: "current" },
    ];

    for (const state of states) {
      expect(currentWorkflowState(state, "current")).toBe(state);
      expect(currentWorkflowState(state, "stale")).toBeNull();
    }
  });
});

describe("workflow request tickets", () => {
  it("rejects A after same-fingerprint B starts", () => {
    const requestA = { revision: 1, fingerprint: "same" };
    const requestB = { revision: 2, fingerprint: "same" };

    expect(isCurrentWorkflowRequest(requestA, 2, "same")).toBe(false);
    expect(isCurrentWorkflowRequest(requestB, 2, "same")).toBe(true);
  });

  it("requires both the ticket and the current fingerprint", () => {
    const ticket = { revision: 4, fingerprint: "captured" };

    expect(isCurrentWorkflowRequest(ticket, 4, "changed")).toBe(false);
    expect(isCurrentWorkflowRequest(ticket, 5, "captured")).toBe(false);
  });
});

describe("workflow result settlement", () => {
  it("keeps ok-false without an operation error as a ready result", () => {
    const value = {
      ok: false,
      errorCode: null,
      reasonCode: null,
      errorMessage: null,
    } as const;

    expect(workflowStateFromResult(value, 7, "export")).toEqual({
      status: "ready",
      revision: 7,
      fingerprint: "export",
      value,
    });
  });

  it("maps canonical cancellation to cancelled state", () => {
    expect(
      workflowStateFromResult(
        {
          errorCode: "cancelled",
          reasonCode: null,
          errorMessage: "cancel diagnostics",
        },
        8,
        "planner",
      ),
    ).toEqual({
      status: "cancelled",
      revision: 8,
      fingerprint: "planner",
    });
  });

  it("keeps canonical code, reason, and diagnostics in error state", () => {
    expect(
      workflowStateFromResult(
        {
          errorCode: "invalid-request",
          reasonCode: "invalid-crop",
          errorMessage: "crop diagnostics",
        },
        9,
        "planner",
      ),
    ).toEqual({
      status: "error",
      revision: 9,
      fingerprint: "planner",
      code: "invalid-request",
      reasonCode: "invalid-crop",
      message: "crop diagnostics",
    });
  });
});

describe("latestActiveWorkflowState", () => {
  it("does not let newer idle export states hide a current planner error", () => {
    const error: VersionedWorkflowState<unknown> = {
      status: "error",
      revision: 4,
      fingerprint: "planner",
      code: "invalid-request",
      reasonCode: "invalid-crop",
      message: "diagnostics",
    };
    const idle: VersionedWorkflowState<unknown> = {
      status: "idle",
      revision: 8,
      fingerprint: "export",
    };

    expect(latestActiveWorkflowState([error, idle])).toBe(error);
  });

  it("accepts loading states with typed operation progress", () => {
    const loading: VersionedWorkflowState<unknown, { stage: "encoding" }> = {
      status: "loading",
      revision: 6,
      fingerprint: "export",
      progress: { stage: "encoding" },
    };

    expect(latestActiveWorkflowState([loading])).toBe(loading);
  });

  it("selects a current cancellation as a neutral workflow state", () => {
    const ready: VersionedWorkflowState<{ id: string }> = {
      status: "ready",
      revision: 4,
      fingerprint: "planner",
      value: { id: "plan" },
    };
    const cancelled: VersionedWorkflowState<unknown> = {
      status: "cancelled",
      revision: 5,
      fingerprint: "export",
    };

    expect(latestActiveWorkflowState([ready, cancelled])).toBe(cancelled);
  });
});
