import { describe, expect, it, vi } from "vitest";

import {
  CSP_DIAGNOSTIC_RECORD_LIMIT,
  createCspDiagnosticsStore,
} from "./cspDiagnostics";

type SyntheticCspEvent = Readonly<{
  effectiveDirective?: unknown;
  violatedDirective?: unknown;
  blockedURI?: unknown;
  sourceFile?: unknown;
  sample?: unknown;
  originalPolicy?: unknown;
}>;

function createSyntheticTarget() {
  let listener: EventListenerOrEventListenerObject | null = null;
  const addEventListener = vi.fn(
    (
      type: string,
      nextListener: EventListenerOrEventListenerObject | null,
    ) => {
      expect(type).toBe("securitypolicyviolation");
      listener = nextListener;
    },
  );

  return {
    target: { addEventListener },
    dispatch(event: SyntheticCspEvent) {
      if (!listener) {
        throw new Error("CSP diagnostics listener was not installed.");
      }
      if (typeof listener === "function") {
        listener(event as unknown as Event);
      } else {
        listener.handleEvent(event as unknown as Event);
      }
    },
  };
}

describe("CSP diagnostics", () => {
  it("installs once and counts repeated synthetic violations", () => {
    const store = createCspDiagnosticsStore();
    const synthetic = createSyntheticTarget();
    const subscriber = vi.fn();
    const unsubscribe = store.subscribe(subscriber);

    store.install(synthetic.target);
    store.install(synthetic.target);
    synthetic.dispatch({
      effectiveDirective: "script-src-elem",
      blockedURI: "https://cdn.example.test/private/app.js?token=secret#entry",
    });
    synthetic.dispatch({
      effectiveDirective: "script-src-elem",
      blockedURI: "https://cdn.example.test/another.js",
    });

    expect(synthetic.target.addEventListener).toHaveBeenCalledTimes(1);
    expect(subscriber).toHaveBeenCalledTimes(2);
    expect(store.getSnapshot()).toEqual({
      violationCount: 2,
      records: [
        {
          directive: "script-src-elem",
          blockedOrigin: "https://cdn.example.test",
          count: 2,
        },
      ],
    });

    unsubscribe();
    synthetic.dispatch({ effectiveDirective: "style-src", blockedURI: "inline" });
    expect(subscriber).toHaveBeenCalledTimes(2);
  });

  it("retains only sanitized origins or safe schemes", () => {
    const store = createCspDiagnosticsStore();
    const secret = "C:/Users/Alice/private-input.mp4?token=secret#frame";

    store.record({
      effectiveDirective: "media-src",
      blockedURI: `asset://localhost/${secret}`,
      sourceFile: `file:///${secret}`,
      sample: `fetch('${secret}')`,
      originalPolicy: `connect-src https://private.example/${secret}`,
    } as SyntheticCspEvent);
    store.record({ effectiveDirective: "img-src", blockedURI: `blob:${secret}` });
    store.record({ effectiveDirective: "img-src", blockedURI: `data:${secret}` });
    store.record({
      effectiveDirective: "connect-src",
      blockedURI: "wss://socket.example.test/private?token=secret#stream",
    });
    store.record({
      effectiveDirective: "connect-src",
      blockedURI: `https://bounded.example.test/${"private/".repeat(600)}`,
    });

    const serialized = JSON.stringify(store.getSnapshot());
    expect(store.getSnapshot().records).toEqual([
      { directive: "media-src", blockedOrigin: "asset:", count: 1 },
      { directive: "img-src", blockedOrigin: "blob:", count: 1 },
      { directive: "img-src", blockedOrigin: "data:", count: 1 },
      {
        directive: "connect-src",
        blockedOrigin: "wss://socket.example.test",
        count: 1,
      },
      { directive: "connect-src", blockedOrigin: "https:", count: 1 },
    ]);
    expect(serialized).not.toContain("Alice");
    expect(serialized).not.toContain("private-input");
    expect(serialized).not.toContain("token");
    expect(serialized).not.toContain("frame");
    expect(serialized).not.toContain("fetch");
    expect(serialized).not.toContain("originalPolicy");
  });

  it("sanitizes malformed and hostile fields without retaining their values", () => {
    const store = createCspDiagnosticsStore();
    const hostileEvent = Object.defineProperties(
      {},
      {
        effectiveDirective: {
          get() {
            throw new Error("private directive");
          },
        },
        violatedDirective: { value: "script-src 'self'" },
        blockedURI: { value: "custom-private://host/private/path" },
      },
    );

    store.record(hostileEvent);

    expect(store.getSnapshot().records).toEqual([
      {
        directive: "unknown-directive",
        blockedOrigin: "other:",
        count: 1,
      },
    ]);
    expect(JSON.stringify(store.getSnapshot())).not.toContain("private/path");
  });

  it("bounds unique records at 50 while keeping the total violation count", () => {
    const store = createCspDiagnosticsStore();

    for (let index = 0; index <= CSP_DIAGNOSTIC_RECORD_LIMIT; index += 1) {
      store.record({
        effectiveDirective: `test-${index}`,
        blockedURI: `https://host-${index}.example.test/private/path`,
      });
    }

    const snapshot = store.getSnapshot();
    expect(snapshot.violationCount).toBe(CSP_DIAGNOSTIC_RECORD_LIMIT + 1);
    expect(snapshot.records).toHaveLength(CSP_DIAGNOSTIC_RECORD_LIMIT);
    expect(snapshot.records[0]).toEqual({
      directive: "test-1",
      blockedOrigin: "https://host-1.example.test",
      count: 1,
    });
    expect(snapshot.records[snapshot.records.length - 1]).toEqual({
      directive: `test-${CSP_DIAGNOSTIC_RECORD_LIMIT}`,
      blockedOrigin: `https://host-${CSP_DIAGNOSTIC_RECORD_LIMIT}.example.test`,
      count: 1,
    });
  });
});
