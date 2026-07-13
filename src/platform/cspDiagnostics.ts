import { useSyncExternalStore } from "react";

export const CSP_DIAGNOSTIC_RECORD_LIMIT = 50;

const CSP_DIAGNOSTIC_URI_INSPECTION_LIMIT = 4_096;
const CSP_DIAGNOSTIC_ORIGIN_LENGTH_LIMIT = 256;

export type CspViolationRecord = Readonly<{
  directive: string;
  blockedOrigin: string;
  count: number;
}>;

export type CspDiagnosticsSnapshot = Readonly<{
  violationCount: number;
  records: readonly CspViolationRecord[];
}>;

type CspViolationEventInput = Readonly<{
  effectiveDirective?: unknown;
  violatedDirective?: unknown;
  blockedURI?: unknown;
}>;

type CspViolationEventTarget = Pick<EventTarget, "addEventListener">;

type CspDiagnosticsStore = Readonly<{
  getSnapshot: () => CspDiagnosticsSnapshot;
  install: (target: CspViolationEventTarget | null) => void;
  record: (event: CspViolationEventInput) => void;
  subscribe: (listener: () => void) => () => void;
}>;

const EMPTY_SNAPSHOT: CspDiagnosticsSnapshot = Object.freeze({
  violationCount: 0,
  records: Object.freeze([]) as readonly CspViolationRecord[],
});

const SAFE_BLOCKED_TOKENS = new Set(["eval", "inline", "self"]);
const ORIGIN_SCHEMES = new Set(["http", "https", "ws", "wss"]);
const SAFE_SCHEMES = new Set([
  "asset",
  "blob",
  "data",
  "file",
  "ipc",
  "tauri",
]);

function readString(
  event: CspViolationEventInput,
  key: keyof CspViolationEventInput,
) {
  try {
    const value = event[key];
    return typeof value === "string" ? value : "";
  } catch {
    return "";
  }
}

function sanitizeDirective(event: CspViolationEventInput) {
  const value =
    readString(event, "effectiveDirective") ||
    readString(event, "violatedDirective");
  if (value.length > 64) {
    return "unknown-directive";
  }
  const normalized = value.trim().toLowerCase();

  return /^[a-z][a-z0-9-]{0,63}$/.test(normalized)
    ? normalized
    : "unknown-directive";
}

function sanitizeBlockedOrigin(event: CspViolationEventInput) {
  const rawValue = readString(event, "blockedURI");
  const inputWasTruncated =
    rawValue.length > CSP_DIAGNOSTIC_URI_INSPECTION_LIMIT;
  const value = rawValue
    .slice(0, CSP_DIAGNOSTIC_URI_INSPECTION_LIMIT)
    .trim();
  const normalizedToken = value.toLowerCase();

  if (!value) {
    return "unknown";
  }

  if (SAFE_BLOCKED_TOKENS.has(normalizedToken)) {
    return normalizedToken;
  }

  const schemeMatch = /^([a-z][a-z0-9+.-]{0,31}):/i.exec(value);
  if (!schemeMatch) {
    return "other";
  }

  const scheme = schemeMatch[1].toLowerCase();
  if (ORIGIN_SCHEMES.has(scheme)) {
    if (inputWasTruncated) {
      return `${scheme}:`;
    }

    try {
      const origin = new URL(value).origin;
      if (
        origin === "null" ||
        origin.length > CSP_DIAGNOSTIC_ORIGIN_LENGTH_LIMIT
      ) {
        return `${scheme}:`;
      }
      return origin;
    } catch {
      return `${scheme}:`;
    }
  }

  return SAFE_SCHEMES.has(scheme) ? `${scheme}:` : "other:";
}

function incrementSafely(value: number) {
  return value < Number.MAX_SAFE_INTEGER ? value + 1 : value;
}

export function createCspDiagnosticsStore(): CspDiagnosticsStore {
  let snapshot = EMPTY_SNAPSHOT;
  const listeners = new Set<() => void>();
  const installedTargets = new WeakSet<object>();

  const getSnapshot = () => snapshot;

  const subscribe = (listener: () => void) => {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  };

  const record = (event: CspViolationEventInput) => {
    const directive = sanitizeDirective(event);
    const blockedOrigin = sanitizeBlockedOrigin(event);
    const matchingIndex = snapshot.records.findIndex(
      (recorded) =>
        recorded.directive === directive &&
        recorded.blockedOrigin === blockedOrigin,
    );
    const records = [...snapshot.records];

    if (matchingIndex >= 0) {
      const matchingRecord = records[matchingIndex];
      records[matchingIndex] = Object.freeze({
        ...matchingRecord,
        count: incrementSafely(matchingRecord.count),
      });
    } else {
      records.push(Object.freeze({ directive, blockedOrigin, count: 1 }));
      if (records.length > CSP_DIAGNOSTIC_RECORD_LIMIT) {
        records.shift();
      }
    }

    snapshot = Object.freeze({
      violationCount: incrementSafely(snapshot.violationCount),
      records: Object.freeze(records),
    });
    listeners.forEach((listener) => listener());
  };

  const install = (target: CspViolationEventTarget | null) => {
    if (!target || installedTargets.has(target)) {
      return;
    }

    const listener: EventListener = (event) => {
      record(event as unknown as CspViolationEventInput);
    };
    target.addEventListener("securitypolicyviolation", listener);
    installedTargets.add(target);
  };

  return Object.freeze({ getSnapshot, install, record, subscribe });
}

const diagnosticsStore = createCspDiagnosticsStore();

export function installCspDiagnostics(
  target: CspViolationEventTarget | null =
    typeof document === "undefined" ? null : document,
) {
  diagnosticsStore.install(target);
}

export function getCspDiagnosticsSnapshot() {
  return diagnosticsStore.getSnapshot();
}

export function subscribeToCspDiagnostics(listener: () => void) {
  return diagnosticsStore.subscribe(listener);
}

export function useCspDiagnostics() {
  return useSyncExternalStore(
    subscribeToCspDiagnostics,
    getCspDiagnosticsSnapshot,
    getCspDiagnosticsSnapshot,
  );
}
