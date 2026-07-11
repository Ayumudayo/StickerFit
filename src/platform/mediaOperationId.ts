let fallbackSessionPrefix: string | null = null;
let fallbackCounter = 0n;

function cryptographicSessionPrefix() {
  if (fallbackSessionPrefix !== null) {
    return fallbackSessionPrefix;
  }
  if (typeof globalThis.crypto?.getRandomValues !== "function") {
    throw new Error("Cryptographic media operation IDs are unavailable.");
  }

  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  fallbackSessionPrefix = Array.from(bytes, (value) =>
    value.toString(16).padStart(2, "0"),
  ).join("");
  return fallbackSessionPrefix;
}

export function createMediaOperationId() {
  if (typeof globalThis.crypto?.randomUUID === "function") {
    return `media-${globalThis.crypto.randomUUID()}`;
  }

  fallbackCounter += 1n;
  return `media-${cryptographicSessionPrefix()}-${fallbackCounter}`;
}
