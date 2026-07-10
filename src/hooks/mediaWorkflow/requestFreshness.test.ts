import { describe, expect, it } from "vitest";

import {
  createRequestFreshnessGuard,
  createRequestLifecycleCoordinator,
} from "./requestFreshness";

type TestValue = Readonly<{ id: string }>;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });

  return { promise, resolve };
}

function value(id: string): TestValue {
  return { id };
}

function createCoordinatorHarness() {
  const published: Array<TestValue | null> = [];
  const disposed: TestValue[] = [];
  const committed: TestValue[] = [];
  const loading: boolean[] = [];
  const coordinator = createRequestLifecycleCoordinator<TestValue>({
    publishCurrent: (nextValue) => published.push(nextValue),
    disposeValue: (staleValue) => disposed.push(staleValue),
  });

  function run(fingerprint: string, request: () => Promise<TestValue>) {
    return coordinator.run({
      fingerprint,
      request,
      onCommit: (currentValue) => committed.push(currentValue),
      onLoadingChange: (nextLoading) => loading.push(nextLoading),
    });
  }

  return {
    coordinator,
    run,
    published,
    disposed,
    committed,
    loading,
  };
}

describe("request freshness", () => {
  it("commits only B when same-fingerprint A resolves after B", async () => {
    const harness = createCoordinatorHarness();
    const requestA = deferred<TestValue>();
    const requestB = deferred<TestValue>();
    const resultA = harness.run("same-source", () => requestA.promise);
    const resultB = harness.run("same-source", () => requestB.promise);
    const valueA = value("result-a");
    const valueB = value("result-b");

    requestB.resolve(valueB);
    await expect(resultB).resolves.toBe(valueB);

    requestA.resolve(valueA);
    await expect(resultA).resolves.toBeNull();

    expect(harness.published[harness.published.length - 1]).toBe(valueB);
    expect(harness.committed).toEqual([valueB]);
    expect(harness.disposed).toEqual([valueA]);
  });

  it("keeps loading active when stale A settles before current B", async () => {
    const harness = createCoordinatorHarness();
    const requestA = deferred<TestValue>();
    const requestB = deferred<TestValue>();
    const resultA = harness.run("source-a", () => requestA.promise);
    const resultB = harness.run("source-b", () => requestB.promise);

    requestA.resolve(value("result-a"));
    await resultA;

    expect(harness.loading).toEqual([true, true]);

    requestB.resolve(value("result-b"));
    await resultB;

    expect(harness.loading).toEqual([true, true, false]);
  });

  it("clears and releases the previous preview exactly once at request begin", async () => {
    const harness = createCoordinatorHarness();
    const firstValue = value("first-result");
    const secondValue = value("second-result");

    await harness.run("same-source", () => Promise.resolve(firstValue));
    const secondRequest = deferred<TestValue>();
    const secondResult = harness.run("same-source", () => secondRequest.promise);

    expect(harness.published[harness.published.length - 1]).toBeNull();
    expect(harness.disposed).toEqual([firstValue]);

    secondRequest.resolve(secondValue);
    await expect(secondResult).resolves.toBe(secondValue);

    expect(harness.disposed).toEqual([firstValue]);
    expect(harness.committed).toEqual([firstValue, secondValue]);
  });

  it("disposes a result invalidated after the resolver returns but before commit", async () => {
    const harness = createCoordinatorHarness();
    const request = deferred<TestValue>();
    const result = harness.run("same-source", () => request.promise);
    const lateValue = value("late-result");

    request.resolve(lateValue);
    queueMicrotask(() => harness.coordinator.invalidate("same-source"));

    await expect(result).resolves.toBeNull();
    expect(harness.committed).toEqual([]);
    expect(harness.disposed).toEqual([lateValue]);
  });

  it("disposes a late result exactly once after unmount invalidation", async () => {
    const harness = createCoordinatorHarness();
    const request = deferred<TestValue>();
    const result = harness.run("same-source", () => request.promise);
    const lateValue = value("unmounted-result");

    harness.coordinator.invalidate();
    request.resolve(lateValue);

    await expect(result).resolves.toBeNull();
    expect(harness.committed).toEqual([]);
    expect(harness.disposed).toEqual([lateValue]);
  });

  it("makes an existing ticket stale when the same fingerprint is invalidated", () => {
    const guard = createRequestFreshnessGuard("same-source");
    const ticket = guard.begin("same-source");

    guard.invalidate("same-source");

    expect(guard.isCurrent(ticket)).toBe(false);
  });

  it("treats repeated begins for the same fingerprint as separate requests", () => {
    const guard = createRequestFreshnessGuard();
    const firstTicket = guard.begin("same-source");
    const secondTicket = guard.begin("same-source");

    expect(guard.isCurrent(firstTicket)).toBe(false);
    expect(guard.isCurrent(secondTicket)).toBe(true);
  });

  it("releases the current preview exactly once when invalidated", async () => {
    const harness = createCoordinatorHarness();
    const currentValue = value("current-result");

    await harness.run("same-source", () => Promise.resolve(currentValue));
    harness.coordinator.invalidate("same-source");
    harness.coordinator.invalidate("same-source");

    expect(harness.published[harness.published.length - 1]).toBeNull();
    expect(harness.disposed).toEqual([currentValue]);
  });
});
