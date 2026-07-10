export type RequestTicket = Readonly<{
  revision: number;
  fingerprint: string;
}>;

export type RequestFreshnessGuard = {
  begin(fingerprint: string): RequestTicket;
  invalidate(fingerprint: string): void;
  isCurrent(ticket: RequestTicket): boolean;
};

export type RequestLifecycleRunParams<T> = {
  fingerprint: string;
  request: () => Promise<T>;
  onBegin?: () => void;
  onCommit?: (value: T) => void;
  onLoadingChange?: (loading: boolean) => void;
};

export type RequestLifecycleCoordinator<T> = {
  run(params: RequestLifecycleRunParams<T>): Promise<T | null>;
  invalidate(fingerprint?: string): void;
};

export function createRequestFreshnessGuard(
  initialFingerprint = "",
): RequestFreshnessGuard {
  let revision = 0;
  let fingerprint = initialFingerprint;

  return {
    begin(nextFingerprint: string) {
      revision += 1;
      fingerprint = nextFingerprint;
      return { revision, fingerprint };
    },
    invalidate(nextFingerprint: string) {
      revision += 1;
      fingerprint = nextFingerprint;
    },
    isCurrent(ticket: RequestTicket) {
      return ticket.revision === revision && ticket.fingerprint === fingerprint;
    },
  };
}

export async function resolveCurrentRequest<T>({
  guard,
  ticket,
  request,
  disposeStale,
}: {
  guard: RequestFreshnessGuard;
  ticket: RequestTicket;
  request: () => Promise<T>;
  disposeStale?: (value: T) => void;
}): Promise<T | null> {
  const value = await request();
  if (guard.isCurrent(ticket)) {
    return value;
  }

  disposeStale?.(value);
  return null;
}

export function createRequestLifecycleCoordinator<T>({
  initialFingerprint = "",
  publishCurrent,
  disposeValue,
}: {
  initialFingerprint?: string;
  publishCurrent: (value: T | null) => void;
  disposeValue: (value: T) => void;
}): RequestLifecycleCoordinator<T> {
  const guard = createRequestFreshnessGuard(initialFingerprint);
  let currentValue: T | null = null;

  function replaceCurrent(nextValue: T | null) {
    const previousValue = currentValue;
    currentValue = nextValue;
    if (previousValue !== null && previousValue !== nextValue) {
      disposeValue(previousValue);
    }
    publishCurrent(nextValue);
  }

  return {
    async run({
      fingerprint,
      request,
      onBegin,
      onCommit,
      onLoadingChange,
    }) {
      const ticket = guard.begin(fingerprint);
      onLoadingChange?.(true);
      replaceCurrent(null);

      try {
        onBegin?.();
        const result = await resolveCurrentRequest({
          guard,
          ticket,
          request,
          disposeStale: disposeValue,
        });
        if (result === null) {
          return null;
        }

        if (!guard.isCurrent(ticket)) {
          disposeValue(result);
          return null;
        }

        replaceCurrent(result);
        onCommit?.(result);
        return result;
      } finally {
        if (guard.isCurrent(ticket)) {
          onLoadingChange?.(false);
        }
      }
    },
    invalidate(fingerprint = "") {
      guard.invalidate(fingerprint);
      replaceCurrent(null);
    },
  };
}
