import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  MediaOperationOptions,
  NormalizedMediaError,
} from "../platform/runtime";
import type {
  ExactCandidateSizeEstimate,
  OperationProgress,
  OptimizerCandidatePreview,
  OutputSizeEstimate,
} from "../types/workflow";
import appSource from "../App.tsx?raw";
import hookSource from "./useOutputSizeEstimate.ts?raw";
import {
  createOutputSizeEstimateCoordinator,
  exactEstimateCacheKey,
  sampleSeedFromFingerprint,
  selectCandidatesForEstimate,
  type OutputEstimateCoordinatorState,
} from "./outputSizeEstimateCoordinator";

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

async function settleMicrotasks() {
  for (let index = 0; index < 6; index += 1) {
    await Promise.resolve();
  }
}

function normalizeError(error: unknown): NormalizedMediaError {
  if (typeof error === "object" && error !== null && "errorCode" in error) {
    const value = error as Partial<NormalizedMediaError>;
    return {
      errorCode: value.errorCode ?? "internal-task-failed",
      reasonCode: value.reasonCode ?? null,
      diagnostics: value.diagnostics ?? null,
    };
  }
  return {
    errorCode: "internal-task-failed",
    reasonCode: null,
    diagnostics: error instanceof Error ? error.message : null,
  };
}

function createHarness(initialFingerprint = "encoding-a") {
  const published: OutputEstimateCoordinatorState[] = [];
  let operationSequence = 0;
  const coordinator = createOutputSizeEstimateCoordinator({
    initialFingerprint,
    publish: (state) => published.push(state),
    createOperationId: () => `operation-${++operationSequence}`,
    normalizeError,
  });
  return { coordinator, published };
}

function candidate(
  id: string,
  rank: number,
  relativeSizeFactor: number,
): OptimizerCandidatePreview {
  return {
    id,
    rank,
    durationSeconds: 3,
    fps: 24,
    contentScale: 1,
    preset: id,
    score: 1 - rank / 100,
    relativeSizeFactor,
    sourceSimilarityScore: 1 - rank / 100,
    summary: id,
  };
}

function exactCandidate(
  candidateId: string,
  bytes = 400 * 1024,
): ExactCandidateSizeEstimate {
  return {
    kind: "exact-candidate",
    basis: "probe",
    bytes,
    candidateId,
    limitBytes: 512 * 1024,
    outputFrameCount: 30,
  };
}

function estimateList(candidateId: string): OutputSizeEstimate[] {
  return [exactCandidate(candidateId)];
}

function progress(operationId: string, completed: number): OperationProgress {
  return {
    operationId,
    stage: "estimating",
    completed,
    total: 10,
    messageCode: "media-operation-estimating",
  };
}

describe("output-size candidate selection", () => {
  it("unions ranked top three with globally smallest compact candidates in stable order", () => {
    const candidates = [
      candidate("rank-4", 4, 0.7),
      candidate("compact-plus", 8, 0.1),
      candidate("rank-2", 2, 0.9),
      candidate("rank-1", 1, 1),
      candidate("compact", 7, 0.2),
      candidate("rank-3", 3, 0.8),
      candidate("rank-5", 5, 0.6),
    ];

    expect(
      selectCandidatesForEstimate(candidates).map((value) => value.id),
    ).toEqual(["rank-1", "rank-2", "rank-3", "compact-plus", "compact"]);
  });

  it("deduplicates overlap without backfilling beyond the frozen top-three plus-smallest-two union", () => {
    const candidates = [
      candidate("rank-1", 1, 0.1),
      candidate("rank-2", 2, 0.2),
      candidate("rank-3", 3, 0.9),
      candidate("rank-4", 4, 0.3),
      candidate("rank-5", 5, 0.4),
      candidate("rank-6", 6, 0.5),
    ];

    expect(
      selectCandidatesForEstimate(candidates).map((value) => value.id),
    ).toEqual(["rank-1", "rank-2", "rank-3"]);
    expect(selectCandidatesForEstimate(candidates)).toHaveLength(3);
  });

  it("uses deterministic tie breaks and never returns more than five candidates", () => {
    const candidates = Array.from({ length: 12 }, (_, index) =>
      candidate(
        `candidate-${String(index + 1).padStart(2, "0")}`,
        index + 1,
        0.5,
      ),
    );
    const selected = selectCandidatesForEstimate(candidates);

    expect(selected.map((value) => value.id)).toEqual([
      "candidate-01",
      "candidate-02",
      "candidate-03",
    ]);
    expect(selected.length).toBeLessThanOrEqual(5);
  });

  it("keeps a compact candidate beyond the six-item display projection eligible", () => {
    const fullPlanCandidates = Array.from({ length: 8 }, (_, index) =>
      candidate(
        index === 7 ? "rank-8-compact" : `rank-${index + 1}`,
        index + 1,
        index === 7 ? 0.05 : 1 - index / 20,
      ),
    );
    const displayedCandidates = fullPlanCandidates.slice(0, 6);

    expect(
      selectCandidatesForEstimate(fullPlanCandidates).map((value) => value.id),
    ).toContain("rank-8-compact");
    expect(
      selectCandidatesForEstimate(displayedCandidates).map((value) => value.id),
    ).not.toContain("rank-8-compact");
    expect(appSource).toContain(
      "candidates: fullPlan.candidates.slice(0, ADVANCED_PREVIEW_COUNT)",
    );
    expect(appSource).toMatch(
      /useOutputSizeEstimate\(\{\s*runtime,\s*inspection,\s*plan: fullPlan,/,
    );
  });
});

describe("sampleSeedFromFingerprint", () => {
  it("returns one stable valid lowercase hexadecimal seed per encoding fingerprint", () => {
    const first = sampleSeedFromFingerprint("encoding-a");
    expect(first).toMatch(/^[0-9a-f]{16}$/);
    expect(sampleSeedFromFingerprint("encoding-a")).toBe(first);
    expect(sampleSeedFromFingerprint("encoding-b")).not.toBe(first);
  });
});

describe("output-size estimate coordinator", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("waits 400 ms and updates a same-key pending run closure without restarting debounce", async () => {
    const { coordinator } = createHarness();
    const firstRun = vi.fn(async () => estimateList("first"));
    const currentLocaleRun = vi.fn(async () => estimateList("current-locale"));

    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "same-numerical-request",
      run: firstRun,
    });
    await vi.advanceTimersByTimeAsync(200);
    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "same-numerical-request",
      run: currentLocaleRun,
    });
    await vi.advanceTimersByTimeAsync(199);
    expect(firstRun).not.toHaveBeenCalled();
    expect(currentLocaleRun).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    await settleMicrotasks();
    expect(firstRun).not.toHaveBeenCalled();
    expect(currentLocaleRun).toHaveBeenCalledTimes(1);
    expect(coordinator.getState().estimate).toMatchObject({
      status: "ready",
      fingerprint: "encoding-a",
    });
  });

  it("keeps a ready numerical result for same-key locale/output-only scheduling", async () => {
    const { coordinator } = createHarness();
    const initialRun = vi.fn(async () => estimateList("candidate-a"));
    const localeOnlyRun = vi.fn(async () => estimateList("candidate-a"));

    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "candidate-set-a",
      run: initialRun,
    });
    await vi.advanceTimersByTimeAsync(400);
    await settleMicrotasks();
    const readyState = coordinator.getState().estimate;

    coordinator.invalidate("encoding-a");
    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "candidate-set-a",
      run: localeOnlyRun,
    });
    await vi.advanceTimersByTimeAsync(800);

    expect(initialRun).toHaveBeenCalledTimes(1);
    expect(localeOnlyRun).not.toHaveBeenCalled();
    expect(coordinator.getState().estimate).toBe(readyState);
  });

  it("aborts A and accepts only B when the request key changes", async () => {
    const { coordinator } = createHarness();
    const requestA = deferred<OutputSizeEstimate[]>();
    const requestB = deferred<OutputSizeEstimate[]>();
    let optionsA!: MediaOperationOptions;
    let optionsB!: MediaOperationOptions;

    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "candidate-set-a",
      run: (options) => {
        optionsA = options;
        return requestA.promise;
      },
    });
    await vi.advanceTimersByTimeAsync(400);
    await settleMicrotasks();

    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "candidate-set-b",
      run: (options) => {
        optionsB = options;
        return requestB.promise;
      },
    });
    expect(optionsA.signal?.aborted).toBe(true);
    requestA.resolve(estimateList("stale-a"));
    await settleMicrotasks();
    expect(coordinator.getState().estimate).toMatchObject({
      status: "loading",
    });

    await vi.advanceTimersByTimeAsync(400);
    await settleMicrotasks();
    expect(optionsB.signal?.aborted).toBe(false);
    requestB.resolve(estimateList("fresh-b"));
    await settleMicrotasks();
    expect(coordinator.getState().estimate).toMatchObject({
      status: "ready",
      value: estimateList("fresh-b"),
    });
  });

  it("aborts work and publishes idle immediately when the encoding fingerprint changes", async () => {
    const { coordinator } = createHarness();
    const request = deferred<OutputSizeEstimate[]>();
    let options!: MediaOperationOptions;

    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "candidate-set-a",
      run: (nextOptions) => {
        options = nextOptions;
        return request.promise;
      },
    });
    await vi.advanceTimersByTimeAsync(400);
    await settleMicrotasks();
    coordinator.invalidate("encoding-b");

    expect(options.signal?.aborted).toBe(true);
    expect(coordinator.getState()).toMatchObject({
      estimate: { status: "idle", fingerprint: "encoding-b" },
      probe: { status: "idle", fingerprint: "encoding-b" },
    });
    request.resolve(estimateList("stale-a"));
    await settleMicrotasks();
    expect(coordinator.getState().estimate).toMatchObject({
      status: "idle",
      fingerprint: "encoding-b",
    });
  });

  it("gates progress by the current request revision, key, and controller", async () => {
    const { coordinator } = createHarness();
    const requestA = deferred<OutputSizeEstimate[]>();
    const requestB = deferred<OutputSizeEstimate[]>();
    let optionsA!: MediaOperationOptions;
    let optionsB!: MediaOperationOptions;

    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "a",
      run: (options) => {
        optionsA = options;
        return requestA.promise;
      },
    });
    await vi.advanceTimersByTimeAsync(400);
    await settleMicrotasks();
    coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "b",
      run: (options) => {
        optionsB = options;
        return requestB.promise;
      },
    });
    await vi.advanceTimersByTimeAsync(400);
    await settleMicrotasks();

    optionsA.onProgress?.(progress(optionsA.operationId, 9));
    expect(coordinator.getState().estimate).toMatchObject({
      status: "loading",
      progress: null,
    });
    const currentProgress = progress(optionsB.operationId, 2);
    optionsB.onProgress?.(currentProgress);
    expect(coordinator.getState().estimate).toMatchObject({
      status: "loading",
      progress: currentProgress,
    });
  });

  it("cancels and retries the same candidate without accepting the first probe late", async () => {
    const { coordinator } = createHarness();
    const first = deferred<ExactCandidateSizeEstimate>();
    const retry = deferred<ExactCandidateSizeEstimate>();
    let firstOptions!: MediaOperationOptions;
    let retryOptions!: MediaOperationOptions;

    coordinator.startProbe({
      fingerprint: "encoding-a",
      candidateId: "candidate-a",
      run: (options) => {
        firstOptions = options;
        return first.promise;
      },
    });
    await settleMicrotasks();
    const firstRevision = coordinator.getState().probe.revision;
    coordinator.cancelProbe();
    expect(firstOptions.signal?.aborted).toBe(true);
    expect(coordinator.getState().probe).toMatchObject({
      status: "cancelled",
      candidateId: "candidate-a",
    });

    coordinator.startProbe({
      fingerprint: "encoding-a",
      candidateId: "candidate-a",
      run: (options) => {
        retryOptions = options;
        return retry.promise;
      },
    });
    await settleMicrotasks();
    expect(coordinator.getState().probe.revision).toBeGreaterThan(
      firstRevision,
    );
    first.resolve(exactCandidate("candidate-a", 500 * 1024));
    await settleMicrotasks();
    expect(coordinator.getState().probe).toMatchObject({ status: "loading" });

    retry.resolve(exactCandidate("candidate-a", 480 * 1024));
    await settleMicrotasks();
    expect(retryOptions.signal?.aborted).toBe(false);
    expect(coordinator.getState().probe).toMatchObject({
      status: "ready",
      value: { candidateId: "candidate-a", bytes: 480 * 1024 },
    });
  });

  it("rejects a probe response for a different candidate", async () => {
    const { coordinator } = createHarness();
    coordinator.startProbe({
      fingerprint: "encoding-a",
      candidateId: "candidate-a",
      run: async () => exactCandidate("candidate-b"),
    });
    await settleMicrotasks();

    expect(coordinator.getState().probe).toMatchObject({
      status: "error",
      candidateId: "candidate-a",
      code: "internal-task-failed",
    });
  });

  it("caches exact probes by fingerprint and candidate while another probe runs", async () => {
    const { coordinator } = createHarness();
    const candidateB = deferred<ExactCandidateSizeEstimate>();
    const exactA = exactCandidate("candidate-a", 470 * 1024);

    coordinator.startProbe({
      fingerprint: "encoding-a",
      candidateId: "candidate-a",
      run: async () => exactA,
    });
    await settleMicrotasks();

    const keyA = exactEstimateCacheKey("encoding-a", "candidate-a");
    expect(coordinator.getState().exactEstimateCache.get(keyA)).toBe(exactA);

    coordinator.startProbe({
      fingerprint: "encoding-a",
      candidateId: "candidate-b",
      run: () => candidateB.promise,
    });
    await settleMicrotasks();

    expect(coordinator.getState().probe).toMatchObject({
      status: "loading",
      candidateId: "candidate-b",
    });
    expect(coordinator.getState().exactEstimateCache.get(keyA)).toBe(exactA);

    const exactB = exactCandidate("candidate-b", 480 * 1024);
    candidateB.resolve(exactB);
    await settleMicrotasks();

    expect(
      coordinator
        .getState()
        .exactEstimateCache.get(
          exactEstimateCacheKey("encoding-a", "candidate-b"),
        ),
    ).toBe(exactB);
    expect(
      coordinator
        .getState()
        .exactEstimateCache.get(
          exactEstimateCacheKey("encoding-b", "candidate-a"),
        ),
    ).toBeUndefined();
  });

  it("bounds exact probe history and evicts the least recently cached entry", async () => {
    const { coordinator } = createHarness();

    for (let index = 0; index < 24; index += 1) {
      const fingerprint = `encoding-${index}`;
      coordinator.invalidate(fingerprint);
      coordinator.startProbe({
        fingerprint,
        candidateId: "candidate-a",
        run: async () => exactCandidate("candidate-a", (index + 1) * 1024),
      });
      await settleMicrotasks();
    }

    const cache = coordinator.getState().exactEstimateCache;
    expect(cache.size).toBe(20);
    expect(
      cache.get(exactEstimateCacheKey("encoding-3", "candidate-a")),
    ).toBeUndefined();
    expect(
      cache.get(exactEstimateCacheKey("encoding-4", "candidate-a")),
    ).toMatchObject({ bytes: 5 * 1024 });
    expect(
      cache.get(exactEstimateCacheKey("encoding-23", "candidate-a")),
    ).toMatchObject({ bytes: 24 * 1024 });
  });

  it("clears pending timers and aborts active estimate and probe work on dispose", async () => {
    const pending = createHarness();
    const pendingRun = vi.fn(async () => estimateList("pending"));
    pending.coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "pending",
      run: pendingRun,
    });
    pending.coordinator.dispose();
    await vi.advanceTimersByTimeAsync(400);
    expect(pendingRun).not.toHaveBeenCalled();

    const active = createHarness();
    const estimate = deferred<OutputSizeEstimate[]>();
    const probe = deferred<ExactCandidateSizeEstimate>();
    let estimateOptions!: MediaOperationOptions;
    let probeOptions!: MediaOperationOptions;
    active.coordinator.scheduleEstimate({
      fingerprint: "encoding-a",
      requestKey: "active",
      run: (options) => {
        estimateOptions = options;
        return estimate.promise;
      },
    });
    await vi.advanceTimersByTimeAsync(400);
    await settleMicrotasks();
    active.coordinator.startProbe({
      fingerprint: "encoding-a",
      candidateId: "candidate-a",
      run: (options) => {
        probeOptions = options;
        return probe.promise;
      },
    });
    await settleMicrotasks();
    const publishCount = active.published.length;
    active.coordinator.dispose();

    expect(estimateOptions.signal?.aborted).toBe(true);
    expect(probeOptions.signal?.aborted).toBe(true);
    estimate.resolve(estimateList("late"));
    probe.resolve(exactCandidate("candidate-a"));
    await settleMicrotasks();
    expect(active.published).toHaveLength(publishCount);
  });
});

describe("useOutputSizeEstimate source contract", () => {
  it("does not invoke desktop estimate APIs in web runtime or synthesize input-size estimates", () => {
    const webGuard = hookSource.indexOf('runtime.kind === "web"');
    const staticInvoke = hookSource.indexOf("runtime.estimateStaticOutputSize");
    const candidateInvoke = hookSource.indexOf(
      "runtime.estimateOptimizerCandidates",
    );

    expect(webGuard).toBeGreaterThanOrEqual(0);
    expect(webGuard).toBeLessThan(staticInvoke);
    expect(webGuard).toBeLessThan(candidateInvoke);
    expect(hookSource).not.toContain("inspection.sizeBytes");
  });

  it("preserves same-fingerprint state when a plan is absent and disposes strict-effect owners", () => {
    expect(hookSource).toContain("coordinator.invalidate(encodingFingerprint)");
    expect(hookSource).toMatch(
      /if \(optimizerPlanRequest === null \|\| selectedCandidateIds\.length === 0\) \{\s*return;/,
    );
    expect(hookSource).toContain("coordinator.dispose()");
    expect(hookSource).toContain("coordinatorRef.current === coordinator");
    expect(hookSource).toContain("candidateSelectionRef.current.fingerprint");
    expect(hookSource).toContain("candidateIds: plannedCandidateIds");
    expect(hookSource).not.toContain("if (plan === null");
  });

  it("limits probes to current near-limit sampled candidates and overlays fingerprint-keyed exact results", () => {
    expect(hookSource).toContain('estimate?.kind === "range"');
    expect(hookSource).toContain("estimate.lowerBytes <= estimate.limitBytes");
    expect(hookSource).toContain("estimate.upperBytes > estimate.limitBytes");
    expect(hookSource).toContain("state.exactEstimateCache.get(");
    expect(hookSource).toContain(
      "exactEstimateCacheKey(encodingFingerprint, estimate.candidateId)",
    );
  });
});
