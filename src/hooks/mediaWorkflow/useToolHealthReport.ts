import { useEffect, useState } from "react";

import type { Locale } from "../../locales/messages";
import type { AppRuntime } from "../../platform/runtime";
import type { ToolHealthReport } from "../../types/workflow";

export function useToolHealthReport(runtime: AppRuntime, initialLocale: Locale) {
  const [toolReport, setToolReport] = useState<ToolHealthReport | null>(null);
  const [toolError, setToolError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function checkToolHealth() {
      try {
        const result = await runtime.checkToolHealth(initialLocale);
        if (!cancelled) {
          setToolReport(result);
          setToolError(null);
        }
      } catch (error) {
        if (!cancelled) {
          setToolError(error instanceof Error ? error.message : String(error));
        }
      }
    }

    void checkToolHealth();

    return () => {
      cancelled = true;
    };
  }, [initialLocale, runtime]);

  return {
    toolReport,
    toolError,
  };
}
