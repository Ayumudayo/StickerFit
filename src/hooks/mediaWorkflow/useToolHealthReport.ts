import { useEffect, useState } from "react";

import { MEDIA_OPERATION_MESSAGES, type Locale } from "../../locales/messages";
import type { AppRuntime } from "../../platform/runtime";
import type { ToolHealthReport } from "../../types/workflow";

export function canonicalToolHealthFailureMessage(
  initialLocale: Locale,
  rejection: unknown,
) {
  void rejection;
  return MEDIA_OPERATION_MESSAGES[initialLocale].errorCodes[
    "internal-task-failed"
  ];
}

export function useToolHealthReport(
  runtime: AppRuntime,
  initialLocale: Locale,
) {
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
          setToolError(canonicalToolHealthFailureMessage(initialLocale, error));
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
