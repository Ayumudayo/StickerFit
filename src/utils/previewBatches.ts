import type {
  FramePreviewLoadState,
  FramePreviewsResult,
} from "../types/workflow";

export const MAX_FRAME_PREVIEW_BATCH_SIZE = 24;

export type FramePreviewEntry = FramePreviewLoadState;

export type PreviewEntryMap = Map<number, FramePreviewEntry>;

type FramePreviewBatchResult = {
  ok: boolean;
  previews: FramePreviewsResult["previews"];
  errorCode: string | null;
  reasonCode: string | null;
  errorMessage: string | null;
};

export type PreviewBatchDescriptor = {
  fingerprint: string;
  currentSourceFrameId: number | null;
  sourceFrameIds: number[];
};

type PreviewPriorityParams = {
  currentSourceFrameId: number | null | undefined;
  lookaheadSourceFrameIds: readonly number[];
  visibleSourceFrameIds: readonly number[];
};

type PreviewBatchSchedulerOptions<
  TDescriptor extends PreviewBatchDescriptor,
  TResult,
> = {
  prepareBatch: (descriptor: TDescriptor) => TDescriptor | null;
  runBatch: (
    descriptor: TDescriptor,
    signal: AbortSignal,
  ) => Promise<TResult>;
  commitBatch?: (
    descriptor: TDescriptor,
    result: TResult,
  ) => void;
};

export type PreviewBatchSchedulerSnapshot = {
  activeCount: 0 | 1;
  pendingCount: 0 | 1;
  generation: number;
  fingerprint: string | null;
};

export type PreviewBatchScheduler<TDescriptor extends PreviewBatchDescriptor> = {
  schedule: (descriptor: TDescriptor) => void;
  reset: (fingerprint: string | null) => void;
  snapshot: () => PreviewBatchSchedulerSnapshot;
  whenIdle: () => Promise<void>;
};

function isValidSourceFrameId(value: number) {
  return Number.isSafeInteger(value) && value > 0;
}

function stableUniqueSourceFrameIds(values: readonly number[]) {
  const seen = new Set<number>();
  const result: number[] = [];

  for (const value of values) {
    if (!isValidSourceFrameId(value) || seen.has(value)) {
      continue;
    }

    seen.add(value);
    result.push(value);
  }

  return result;
}

export function chunkFramePreviewIds(values: readonly number[]) {
  const sourceFrameIds = stableUniqueSourceFrameIds(values);
  const chunks: number[][] = [];

  for (let index = 0; index < sourceFrameIds.length; index += MAX_FRAME_PREVIEW_BATCH_SIZE) {
    chunks.push(sourceFrameIds.slice(index, index + MAX_FRAME_PREVIEW_BATCH_SIZE));
  }

  return chunks;
}

export function prioritizeFramePreviewIds({
  currentSourceFrameId,
  lookaheadSourceFrameIds,
  visibleSourceFrameIds,
}: PreviewPriorityParams) {
  return stableUniqueSourceFrameIds([
    ...(currentSourceFrameId === null || currentSourceFrameId === undefined
      ? []
      : [currentSourceFrameId]),
    ...lookaheadSourceFrameIds,
    ...visibleSourceFrameIds,
  ]);
}

export function markFramePreviewBatchLoading(
  entries: ReadonlyMap<number, FramePreviewEntry>,
  sourceFrameIds: readonly number[],
  batchToken: string,
) {
  const nextEntries: PreviewEntryMap = new Map(entries);

  for (const sourceFrameId of stableUniqueSourceFrameIds(sourceFrameIds)) {
    nextEntries.set(sourceFrameId, { status: "loading", batchToken });
  }

  return nextEntries;
}

function previewErrorEntry(
  result: FramePreviewBatchResult,
  fallbackMessage: string,
): Extract<FramePreviewEntry, { status: "error" }> {
  const entry: Extract<FramePreviewEntry, { status: "error" }> = {
    status: "error",
    message: result.errorMessage?.trim() || fallbackMessage,
  };

  if (result.errorCode) {
    entry.errorCode = result.errorCode as NonNullable<typeof entry.errorCode>;
  }
  if (result.reasonCode) {
    entry.reasonCode = result.reasonCode as NonNullable<typeof entry.reasonCode>;
  }

  return entry;
}

export function applyFramePreviewBatchResult(
  entries: ReadonlyMap<number, FramePreviewEntry>,
  requestedSourceFrameIds: readonly number[],
  result: FramePreviewBatchResult | null,
  fallbackMessage: string,
) {
  const nextEntries: PreviewEntryMap = new Map(entries);
  const requestedIds = stableUniqueSourceFrameIds(requestedSourceFrameIds);

  if (!result) {
    for (const sourceFrameId of requestedIds) {
      nextEntries.set(sourceFrameId, { status: "error", message: fallbackMessage });
    }
    return nextEntries;
  }

  if (!result.ok) {
    for (const sourceFrameId of requestedIds) {
      nextEntries.set(sourceFrameId, previewErrorEntry(result, fallbackMessage));
    }
    return nextEntries;
  }

  const previewBySourceFrameId = new Map(
    result.previews.map((preview) => [preview.sourceFrameId, preview] as const),
  );

  for (const sourceFrameId of requestedIds) {
    const preview = previewBySourceFrameId.get(sourceFrameId);
    nextEntries.set(
      sourceFrameId,
      preview
        ? {
            status: "ready",
            dataUrl: preview.dataUrl,
            width: preview.width,
            height: preview.height,
          }
        : { status: "error", message: fallbackMessage },
    );
  }

  return nextEntries;
}

export function retryFramePreviewEntry(
  entries: ReadonlyMap<number, FramePreviewEntry>,
  sourceFrameId: number,
) {
  const nextEntries: PreviewEntryMap = new Map(entries);
  if (entries.get(sourceFrameId)?.status === "error") {
    nextEntries.set(sourceFrameId, { status: "idle" });
  }
  return nextEntries;
}

export function filterFramePreviewDemand(
  sourceFrameIds: readonly number[],
  entries: ReadonlyMap<number, FramePreviewEntry>,
) {
  return stableUniqueSourceFrameIds(sourceFrameIds).filter((sourceFrameId) => {
    const status = entries.get(sourceFrameId)?.status;
    return status === undefined || status === "idle";
  });
}

export function createPreviewBatchScheduler<
  TDescriptor extends PreviewBatchDescriptor,
  TResult = void,
>({
  prepareBatch,
  runBatch,
  commitBatch,
}: PreviewBatchSchedulerOptions<TDescriptor, TResult>): PreviewBatchScheduler<TDescriptor> {
  type DrainState = {
    descriptor: TDescriptor;
    consumedSourceFrameIds: Set<number>;
  };
  type ActiveBatch = {
    generation: number;
    drain: DrainState;
    descriptor: TDescriptor;
    controller: AbortController;
  };

  let generation = 0;
  let fingerprint: string | null = null;
  let active: ActiveBatch | null = null;
  let pending: TDescriptor | null = null;
  let drain: DrainState | null = null;
  let idleWaiters: Array<() => void> = [];

  const isIdle = () => active === null && pending === null && drain === null;

  const resolveIdleWaiters = () => {
    if (!isIdle()) {
      return;
    }

    const waiters = idleWaiters;
    idleWaiters = [];
    for (const resolve of waiters) {
      resolve();
    }
  };

  const descriptorWithSourceFrameIds = (
    descriptor: TDescriptor,
    sourceFrameIds: number[],
  ) => ({ ...descriptor, sourceFrameIds } as TDescriptor);

  const prepareNextBatch = (state: DrainState) => {
    const remainingSourceFrameIds = stableUniqueSourceFrameIds(
      state.descriptor.sourceFrameIds,
    ).filter((sourceFrameId) => !state.consumedSourceFrameIds.has(sourceFrameId));
    if (remainingSourceFrameIds.length === 0) {
      return null;
    }

    const prepared = prepareBatch(
      descriptorWithSourceFrameIds(state.descriptor, remainingSourceFrameIds),
    );
    if (!prepared) {
      return null;
    }

    const remainingSet = new Set(remainingSourceFrameIds);
    const batchSourceFrameIds = stableUniqueSourceFrameIds(prepared.sourceFrameIds)
      .filter((sourceFrameId) => remainingSet.has(sourceFrameId))
      .slice(0, MAX_FRAME_PREVIEW_BATCH_SIZE);
    return batchSourceFrameIds.length > 0
      ? descriptorWithSourceFrameIds(prepared, batchSourceFrameIds)
      : null;
  };

  const startDrain = (descriptor: TDescriptor) => {
    drain = {
      descriptor,
      consumedSourceFrameIds: new Set<number>(),
    };
  };

  const promote = () => {
    if (active || !drain) {
      return;
    }

    const prepared = prepareNextBatch(drain);
    if (!prepared) {
      drain = null;
      resolveIdleWaiters();
      return;
    }

    const activeBatch: ActiveBatch = {
      generation,
      drain,
      descriptor: prepared,
      controller: new AbortController(),
    };
    active = activeBatch;

    let operation: Promise<TResult>;
    try {
      operation = runBatch(prepared, activeBatch.controller.signal);
    } catch (error) {
      operation = Promise.reject(error);
    }

    const finish = () => {
      if (active !== activeBatch || generation !== activeBatch.generation) {
        return;
      }

      for (const sourceFrameId of activeBatch.descriptor.sourceFrameIds) {
        activeBatch.drain.consumedSourceFrameIds.add(sourceFrameId);
      }
      active = null;

      if (pending) {
        const nextDescriptor = pending;
        pending = null;
        startDrain(nextDescriptor);
      }

      promote();
    };

    void operation.then(
      (result) => {
        if (active !== activeBatch || generation !== activeBatch.generation) {
          return;
        }

        try {
          commitBatch?.(activeBatch.descriptor, result);
        } catch {
          // A commit failure must not strand scheduler ownership or start stale work.
        } finally {
          finish();
        }
      },
      () => finish(),
    );
  };

  return {
    schedule(descriptor) {
      if (fingerprint === null) {
        fingerprint = descriptor.fingerprint;
      }
      if (descriptor.fingerprint !== fingerprint) {
        return;
      }

      if (active) {
        pending = descriptor;
        return;
      }

      pending = null;
      startDrain(descriptor);
      promote();
    },

    reset(nextFingerprint) {
      generation += 1;
      fingerprint = nextFingerprint;
      active?.controller.abort();
      active = null;
      pending = null;
      drain = null;
      resolveIdleWaiters();
    },

    snapshot() {
      return {
        activeCount: active ? 1 : 0,
        pendingCount: pending ? 1 : 0,
        generation,
        fingerprint,
      };
    },

    whenIdle() {
      if (isIdle()) {
        return Promise.resolve();
      }
      return new Promise<void>((resolve) => {
        idleWaiters.push(resolve);
      });
    },
  };
}
