import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

async function importFreshGenerator() {
  vi.resetModules();
  return import("./mediaOperationId");
}

describe("createMediaOperationId", () => {
  it("uses randomUUID and produces 10,000 unique IDs", async () => {
    let nextUuid = 0;
    const randomUUID = vi.fn(
      () => `00000000-0000-4000-8000-${String(nextUuid++).padStart(12, "0")}`,
    );
    vi.stubGlobal("crypto", {
      randomUUID,
      getRandomValues: <T extends ArrayBufferView>(value: T) => value,
    });
    const { createMediaOperationId } = await importFreshGenerator();

    const ids = Array.from({ length: 10_000 }, () => createMediaOperationId());

    expect(new Set(ids).size).toBe(ids.length);
    expect(randomUUID).toHaveBeenCalledTimes(10_000);
  });

  it("uses a cryptographic session prefix and counter for 10,000 fallback IDs", async () => {
    let seed = 1;
    const getRandomValues = vi.fn(<T extends ArrayBufferView>(value: T) => {
      const bytes = new Uint8Array(
        value.buffer,
        value.byteOffset,
        value.byteLength,
      );
      bytes.fill(seed++);
      return value;
    });
    vi.stubGlobal("crypto", {
      getRandomValues,
    });
    const { createMediaOperationId } = await importFreshGenerator();

    const ids = Array.from({ length: 10_000 }, () => createMediaOperationId());
    const sessionPrefixes = ids.map((id) => id.slice(0, id.lastIndexOf("-")));

    expect(new Set(ids).size).toBe(ids.length);
    expect(new Set(sessionPrefixes).size).toBe(1);
    expect(getRandomValues).toHaveBeenCalledOnce();
  });

  it("does not reuse IDs across simulated hook or component remounts", async () => {
    vi.stubGlobal("crypto", {
      getRandomValues: <T extends ArrayBufferView>(value: T) => {
        new Uint8Array(value.buffer, value.byteOffset, value.byteLength).fill(
          0x5a,
        );
        return value;
      },
    });
    const { createMediaOperationId } = await importFreshGenerator();

    const firstMountIds = [createMediaOperationId(), createMediaOperationId()];
    const secondMountIds = [createMediaOperationId(), createMediaOperationId()];

    expect(new Set([...firstMountIds, ...secondMountIds]).size).toBe(4);
    expect(secondMountIds[0]).not.toBe(firstMountIds[0]);
  });

  it("uses a different fallback session prefix after a module reload", async () => {
    let seed = 0x10;
    const getRandomValues = vi.fn(<T extends ArrayBufferView>(value: T) => {
      new Uint8Array(value.buffer, value.byteOffset, value.byteLength).fill(
        seed++,
      );
      return value;
    });
    vi.stubGlobal("crypto", {
      getRandomValues,
    });

    const firstModule = await importFreshGenerator();
    const firstId = firstModule.createMediaOperationId();
    const secondModule = await importFreshGenerator();
    const secondId = secondModule.createMediaOperationId();

    expect(firstId.split("-").slice(0, -1).join("-")).not.toBe(
      secondId.split("-").slice(0, -1).join("-"),
    );
    expect(getRandomValues).toHaveBeenCalledTimes(2);
  });
});
