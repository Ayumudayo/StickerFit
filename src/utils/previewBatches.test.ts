import { describe, expect, it, vi } from "vitest";

import type { FramePreviewsResult } from "../types/workflow";
import previewBatchesSource from "./previewBatches.ts?raw";
import {
  applyFramePreviewBatchResult,
  chunkFramePreviewIds,
  createPreviewBatchScheduler,
  filterFramePreviewDemand,
  markFramePreviewBatchLoading,
  prioritizeFramePreviewIds,
  retryFramePreviewEntry,
  type FramePreviewEntry,
  type PreviewBatchDescriptor,
  type PreviewEntryMap,
} from "./previewBatches";

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

function descriptor(currentSourceFrameId: number, sourceFrameIds: number[]): PreviewBatchDescriptor {
  return {
    fingerprint: "source-a",
    currentSourceFrameId,
    sourceFrameIds,
  };
}

describe("chunkFramePreviewIds", () => {
  it("stably deduplicates and chunks fifty IDs as 24, 24, and 2", () => {
    const ids = [...Array.from({ length: 50 }, (_, index) => index + 1), 1, 2];

    expect(chunkFramePreviewIds(ids)).toEqual([
      Array.from({ length: 24 }, (_, index) => index + 1),
      Array.from({ length: 24 }, (_, index) => index + 25),
      [49, 50],
    ]);
  });

  it("drops invalid IDs without reordering the remaining demand", () => {
    expect(chunkFramePreviewIds([3, 0, -1, 3, 2, Number.NaN, 1])).toEqual([[3, 2, 1]]);
  });
});

describe("prioritizeFramePreviewIds", () => {
  it("puts an offscreen current frame first, then edited-order lookahead and visible rows", () => {
    expect(
      prioritizeFramePreviewIds({
        currentSourceFrameId: 90,
        lookaheadSourceFrameIds: [4, 90, 4, 7],
        visibleSourceFrameIds: [1, 2, 4, 3],
      }),
    ).toEqual([90, 4, 7, 1, 2, 3]);
  });

  it("preserves edited timeline order rather than sorting numeric source IDs", () => {
    expect(
      prioritizeFramePreviewIds({
        currentSourceFrameId: 8,
        lookaheadSourceFrameIds: [3, 8, 20, 3],
        visibleSourceFrameIds: [50, 1, 20],
      }),
    ).toEqual([8, 3, 20, 50, 1]);
  });
});

describe("preview entry transitions", () => {
  const successfulResult = (sourceFrameIds: number[]): FramePreviewsResult => ({
    ok: true,
    previews: sourceFrameIds.map((sourceFrameId) => ({
      sourceFrameId,
      dataUrl: `data:image/png;base64,${sourceFrameId}`,
      width: 128,
      height: 64,
    })),
    errorCode: null,
    reasonCode: null,
    errorMessage: null,
  });

  it("marks active IDs loading and never marks an unpromoted pending descriptor", () => {
    const initial: PreviewEntryMap = new Map();
    const loading = markFramePreviewBatchLoading(initial, [1, 2], "batch-a");

    expect(loading.get(1)).toMatchObject({ status: "loading", batchToken: "batch-a" });
    expect(loading.get(2)).toMatchObject({ status: "loading", batchToken: "batch-a" });
    expect(loading.has(3)).toBe(false);
  });

  it("terminates missing success items as errors instead of stranding loading state", () => {
    const loading = markFramePreviewBatchLoading(new Map(), [1, 2, 3], "batch-a");
    const settled = applyFramePreviewBatchResult(
      loading,
      [1, 2, 3],
      successfulResult([1, 3]),
      "Preview unavailable",
    );

    expect(settled.get(1)).toMatchObject({ status: "ready" });
    expect(settled.get(2)).toEqual({ status: "error", message: "Preview unavailable" });
    expect(settled.get(3)).toMatchObject({ status: "ready" });
  });

  it("returns only an error entry to idle when retry is requested", () => {
    const entries: PreviewEntryMap = new Map<number, FramePreviewEntry>([
      [1, { status: "ready", dataUrl: "data:ready", width: 1, height: 1 }],
      [2, { status: "error", message: "failed" }],
    ]);

    const retried = retryFramePreviewEntry(entries, 2);
    expect(retried.get(1)).toEqual(entries.get(1));
    expect(retried.get(2)).toEqual({ status: "idle" });
  });

  it("preserves backend failure codes and returns a retried error to scheduler demand", () => {
    const loading = markFramePreviewBatchLoading(new Map(), [7], "batch-a");
    const backendFailure = {
      ok: false,
      previews: [],
      errorCode: "invalid-request",
      reasonCode: "frame-preview-decode-failed",
      errorMessage: "decoder failed",
    } satisfies FramePreviewsResult;
    const failed = applyFramePreviewBatchResult(
      loading,
      [7],
      backendFailure,
      "Preview unavailable",
    );

    expect(failed.get(7)).toEqual({
      status: "error",
      errorCode: "invalid-request",
      reasonCode: "frame-preview-decode-failed",
      message: "decoder failed",
    });
    expect(filterFramePreviewDemand([7], failed)).toEqual([]);
    expect(filterFramePreviewDemand([7], retryFramePreviewEntry(failed, 7))).toEqual([7]);
  });

  it("derives preview errors from the closed workflow result without casts", () => {
    expect(previewBatchesSource).toMatch(
      /type FramePreviewBatchResult\s*=\s*Pick<\s*FramePreviewsResult,/,
    );
    expect(previewBatchesSource).not.toMatch(/errorCode:\s*string\s*\|\s*null/);
    expect(previewBatchesSource).not.toMatch(/reasonCode:\s*string\s*\|\s*null/);
    const previewErrorEntrySource = previewBatchesSource.match(
      /function previewErrorEntry\([\s\S]*?\n\}/,
    )?.[0];
    expect(previewErrorEntrySource).toBeDefined();
    expect(previewErrorEntrySource).not.toMatch(/\bas\b/);
  });

  it("excludes ready, loading, and unretried error IDs from active demand", () => {
    const entries: PreviewEntryMap = new Map<number, FramePreviewEntry>([
      [1, { status: "ready", dataUrl: "data:ready", width: 1, height: 1 }],
      [2, { status: "loading", batchToken: "batch-a" }],
      [3, { status: "error", message: "failed" }],
      [4, { status: "idle" }],
    ]);

    expect(filterFramePreviewDemand([1, 2, 3, 4, 5], entries)).toEqual([4, 5]);
  });
});

describe("createPreviewBatchScheduler", () => {
  it("keeps one active and one replace-only pending batch across 150 rapid current ticks", async () => {
    const runs: PreviewBatchDescriptor[] = [];
    const operations: string[] = [];
    const first = deferred<void>();
    const second = deferred<void>();
    let active = 0;
    let maxActive = 0;
    const scheduler = createPreviewBatchScheduler({
      prepareBatch: (value) => ({ ...value, sourceFrameIds: value.sourceFrameIds.slice(0, 24) }),
      runBatch: async (value) => {
        operations.push(`operation-${operations.length + 1}`);
        runs.push(value);
        active += 1;
        maxActive = Math.max(maxActive, active);
        try {
          await (runs.length === 1 ? first.promise : second.promise);
        } finally {
          active -= 1;
        }
      },
    });

    scheduler.schedule(descriptor(1, [1, 2, 3]));
    for (let current = 2; current <= 150; current += 1) {
      scheduler.schedule(descriptor(current, [current, 1, 2, 3]));
      expect(scheduler.snapshot().activeCount).toBeLessThanOrEqual(1);
      expect(scheduler.snapshot().pendingCount).toBeLessThanOrEqual(1);
    }

    expect(runs).toHaveLength(1);
    expect(operations).toHaveLength(1);
    first.resolve();
    await vi.waitFor(() => expect(runs).toHaveLength(2));

    expect(runs[1]?.sourceFrameIds[0]).toBe(150);
    expect(operations).toHaveLength(2);
    expect(maxActive).toBe(1);
    second.resolve();
    await scheduler.whenIdle();
  });

  it("re-filters the latest pending descriptor only when it is promoted", async () => {
    const blocked = new Set<number>();
    const first = deferred<void>();
    const runs: number[][] = [];
    const scheduler = createPreviewBatchScheduler({
      prepareBatch: (value) => {
        const sourceFrameIds = value.sourceFrameIds.filter((id) => !blocked.has(id)).slice(0, 24);
        return sourceFrameIds.length > 0 ? { ...value, sourceFrameIds } : null;
      },
      runBatch: async (value) => {
        runs.push(value.sourceFrameIds);
        if (runs.length === 1) await first.promise;
      },
    });

    scheduler.schedule(descriptor(1, [1]));
    scheduler.schedule(descriptor(3, [3, 2]));
    blocked.add(2);
    first.resolve();
    await scheduler.whenIdle();

    expect(runs).toEqual([[1], [3]]);
  });

  it("drains fifty desired IDs in sequential 24, 24, and 2 batches", async () => {
    const ready = new Set<number>();
    const runs: number[][] = [];
    const scheduler = createPreviewBatchScheduler({
      prepareBatch: (value) => {
        const sourceFrameIds = value.sourceFrameIds.filter((id) => !ready.has(id)).slice(0, 24);
        return sourceFrameIds.length > 0 ? { ...value, sourceFrameIds } : null;
      },
      runBatch: async (value) => {
        runs.push(value.sourceFrameIds);
        value.sourceFrameIds.forEach((id) => ready.add(id));
      },
    });

    scheduler.schedule(descriptor(1, Array.from({ length: 50 }, (_, index) => index + 1)));
    await scheduler.whenIdle();

    expect(runs.map((ids) => ids.length)).toEqual([24, 24, 2]);
    expect(runs.flat()).toEqual(Array.from({ length: 50 }, (_, index) => index + 1));
  });

  it("aborts active work and blocks stale commit without clearing a fresh active generation", async () => {
    const stale = deferred<string>();
    const fresh = deferred<string>();
    const runs: string[] = [];
    const commits: string[] = [];
    const activeSignals: AbortSignal[] = [];
    const scheduler = createPreviewBatchScheduler({
      prepareBatch: (value) => value,
      runBatch: async (value, signal) => {
        activeSignals.push(signal);
        runs.push(value.fingerprint);
        return value.fingerprint === "source-a" ? stale.promise : fresh.promise;
      },
      commitBatch: (_value, result) => commits.push(result),
    });

    scheduler.schedule(descriptor(1, [1]));
    scheduler.schedule(descriptor(2, [2]));
    scheduler.reset("source-b");
    expect(activeSignals[0]?.aborted).toBe(true);
    expect(scheduler.snapshot()).toMatchObject({ activeCount: 0, pendingCount: 0 });
    scheduler.schedule({ fingerprint: "source-b", currentSourceFrameId: 9, sourceFrameIds: [9] });

    stale.resolve("stale-a");
    await vi.waitFor(() => expect(runs).toEqual(["source-a", "source-b"]));
    expect(commits).toEqual([]);
    expect(scheduler.snapshot().activeCount).toBe(1);

    fresh.resolve("fresh-b");
    await scheduler.whenIdle();
    expect(commits).toEqual(["fresh-b"]);
  });
});
