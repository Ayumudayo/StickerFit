import { useCallback, useEffect, useRef, useState } from "react";

import type { Locale } from "../../locales/messages";
import type { AppRuntime } from "../../platform/runtime";
import { releaseInspectionPreview, type RuntimeInputSource } from "../../platform/runtime";
import type { MediaInspection } from "../../types/workflow";

type UseMediaInputSelectorParams = {
  runtime: AppRuntime;
  locale: Locale;
  onResetForNewInspection: () => void;
  onCommitEditorSession: () => void;
};

export function useMediaInputSelector({
  runtime,
  locale,
  onResetForNewInspection,
  onCommitEditorSession,
}: UseMediaInputSelectorParams) {
  const [inspection, setInspection] = useState<MediaInspection | null>(null);
  const [inspectionLoading, setInspectionLoading] = useState(false);
  const [isDragging, setIsDragging] = useState(false);
  const [outputDirectory, setOutputDirectory] = useState<string | null>(null);

  const previousInspectionRef = useRef<MediaInspection | null>(null);

  const inspectSource = useCallback(
    async (source: RuntimeInputSource) => {
      setInspectionLoading(true);
      onResetForNewInspection();

      try {
        const result = await runtime.inspectInput(source, locale);
        setInspection(result);
        onCommitEditorSession();
      } finally {
        setInspectionLoading(false);
      }
    },
    [locale, onCommitEditorSession, onResetForNewInspection, runtime],
  );

  useEffect(() => {
    let dispose: (() => void) | null = null;
    let cancelled = false;

    void runtime
      .subscribeInputDrops({
        onDraggingChange: (dragging) => {
          setIsDragging(dragging);
        },
        onDrop: async (source) => {
          setIsDragging(false);
          await inspectSource(source);
        },
      })
      .then((nextDispose) => {
        if (cancelled) {
          nextDispose();
          return;
        }

        dispose = nextDispose;
      });

    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [inspectSource, runtime]);

  useEffect(() => {
    const previousInspection = previousInspectionRef.current;
    previousInspectionRef.current = inspection;
    if (previousInspection && previousInspection !== inspection) {
      releaseInspectionPreview(previousInspection);
    }
  }, [inspection]);

  useEffect(() => {
    return () => {
      releaseInspectionPreview(previousInspectionRef.current);
    };
  }, []);

  const pickInputFile = useCallback(async () => {
    const selected = await runtime.pickInputFile();
    if (!selected) {
      return;
    }

    await inspectSource(selected);
  }, [inspectSource, runtime]);

  const pickOutputDirectory = useCallback(async () => {
    const selected = await runtime.pickOutputDirectory();
    if (!selected) {
      return;
    }

    setOutputDirectory(selected);
  }, [runtime]);

  const openOutputFolder = useCallback(
    async (path?: string | null) => {
      await runtime.openOutputFolder(
        path,
        outputDirectory ?? inspection?.backendInputPath ?? null,
        locale,
      );
    },
    [inspection?.backendInputPath, locale, outputDirectory, runtime],
  );

  return {
    inspection,
    inspectionLoading,
    isDragging,
    outputDirectory,
    setOutputDirectory,
    pickInputFile,
    pickOutputDirectory,
    openOutputFolder,
  };
}
