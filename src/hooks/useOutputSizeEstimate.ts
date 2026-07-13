import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { CropRegion } from "../components/MediaSelectionPreview";
import type { Locale } from "../locales/messages";
import { createMediaOperationId } from "../platform/mediaOperationId";
import {
  normalizeLegacyMediaError,
  type AppRuntime,
} from "../platform/runtime";
import type {
  CandidateSizeProbeRequest,
  MediaInspection,
  OptimizerPlanRequest,
  OptimizerPlanResponse,
  OptimizerSizeEstimateRequest,
  OutputSizeEstimate,
  StaticSizeEstimateRequest,
} from "../types/workflow";
import {
  createOutputSizeEstimateCoordinator,
  exactEstimateCacheKey,
  sampleSeedFromFingerprint,
  selectCandidatesForEstimate,
  type OutputEstimateCoordinatorState,
  type OutputSizeEstimateCoordinator,
} from "./outputSizeEstimateCoordinator";

export type UseOutputSizeEstimateParams = {
  runtime: AppRuntime;
  inspection: MediaInspection | null;
  plan: OptimizerPlanResponse | null;
  optimizerPlanRequest: OptimizerPlanRequest | null;
  encodingFingerprint: string;
  cropRegion: CropRegion;
  locale: Locale;
};

export type UseOutputSizeEstimateResult = {
  state: OutputEstimateCoordinatorState;
  estimates: readonly OutputSizeEstimate[];
  estimateByCandidateId: ReadonlyMap<string, OutputSizeEstimate>;
  selectedCandidateIds: readonly string[];
  retryEstimate(): void;
  probeCandidate(candidateId: string): void;
  cancelProbe(): void;
};

function initialCoordinatorState(
  fingerprint: string,
): OutputEstimateCoordinatorState {
  return {
    estimate: { status: "idle", revision: 0, fingerprint },
    probe: { status: "idle", revision: 0, fingerprint },
    exactEstimateCache: new Map(),
  };
}

function inspectedBackendSource(inspection: MediaInspection | null) {
  if (
    !inspection?.ok ||
    !inspection.backendInputPath ||
    !inspection.sourceRevision?.trim()
  ) {
    return null;
  }
  return {
    inputPath: inspection.backendInputPath,
    sourceRevision: inspection.sourceRevision,
  };
}

function validateStaticEstimate(value: OutputSizeEstimate) {
  if (value.kind !== "exact-static" || value.candidateId !== null) {
    throw new Error("Static size estimate returned an invalid result kind.");
  }
  return value;
}

function validateCandidateEstimates(
  values: OutputSizeEstimate[],
  candidateIds: readonly string[],
) {
  if (values.length !== candidateIds.length) {
    throw new Error(
      "Candidate size estimate returned an invalid result count.",
    );
  }
  for (let index = 0; index < candidateIds.length; index += 1) {
    if (values[index]?.candidateId !== candidateIds[index]) {
      throw new Error(
        "Candidate size estimate returned results out of request order.",
      );
    }
  }
  return values;
}

function isNearLimitSampledEstimate(estimate: OutputSizeEstimate | undefined) {
  return (
    estimate?.kind === "range" &&
    estimate.lowerBytes <= estimate.limitBytes &&
    estimate.upperBytes > estimate.limitBytes
  );
}

export function useOutputSizeEstimate({
  runtime,
  inspection,
  plan,
  optimizerPlanRequest,
  encodingFingerprint,
  cropRegion,
  locale,
}: UseOutputSizeEstimateParams): UseOutputSizeEstimateResult {
  const [state, setState] = useState<OutputEstimateCoordinatorState>(() =>
    initialCoordinatorState(encodingFingerprint),
  );
  const stateRef = useRef(state);
  stateRef.current = state;
  const coordinatorRef = useRef<OutputSizeEstimateCoordinator | null>(null);
  const candidateSelectionRef = useRef<{
    fingerprint: string;
    candidateIds: readonly string[];
  }>({ fingerprint: encodingFingerprint, candidateIds: [] });
  const backendSource = inspectedBackendSource(inspection);
  const selectedCandidates = useMemo(
    () =>
      inspection?.ok && !inspection.isStaticImage && plan
        ? selectCandidatesForEstimate(plan.candidates)
        : [],
    [inspection?.isStaticImage, inspection?.ok, plan],
  );
  const plannedCandidateIds = useMemo(
    () => selectedCandidates.map((candidate) => candidate.id),
    [selectedCandidates],
  );
  if (candidateSelectionRef.current.fingerprint !== encodingFingerprint) {
    candidateSelectionRef.current = {
      fingerprint: encodingFingerprint,
      candidateIds: [],
    };
  }
  if (plannedCandidateIds.length > 0) {
    candidateSelectionRef.current = {
      fingerprint: encodingFingerprint,
      candidateIds: plannedCandidateIds,
    };
  }
  const selectedCandidateIds = candidateSelectionRef.current.candidateIds;

  useEffect(() => {
    const coordinator = createOutputSizeEstimateCoordinator({
      initialFingerprint: encodingFingerprint,
      createOperationId: createMediaOperationId,
      normalizeError: normalizeLegacyMediaError,
      publish: (nextState) => {
        stateRef.current = nextState;
        setState(nextState);
      },
    });
    coordinatorRef.current = coordinator;
    const nextState = coordinator.getState();
    stateRef.current = nextState;
    setState(nextState);

    return () => {
      coordinator.dispose();
      if (coordinatorRef.current === coordinator) {
        coordinatorRef.current = null;
      }
    };
  }, [runtime]);

  useEffect(() => {
    const coordinator = coordinatorRef.current;
    if (coordinator === null) return;
    coordinator.invalidate(encodingFingerprint);

    if (
      runtime.kind === "web" ||
      !runtime.capabilities.backendProcessing ||
      backendSource === null ||
      !inspection?.ok
    ) {
      return;
    }

    if (inspection.isStaticImage) {
      const request: StaticSizeEstimateRequest = {
        ...backendSource,
        locale,
        cropRegion,
      };
      coordinator.scheduleEstimate({
        fingerprint: encodingFingerprint,
        requestKey: `static:${encodingFingerprint}`,
        run: async (options) => [
          validateStaticEstimate(
            await runtime.estimateStaticOutputSize(request, options),
          ),
        ],
      });
      return;
    }

    if (optimizerPlanRequest === null || selectedCandidateIds.length === 0) {
      return;
    }

    const candidateIds = [...selectedCandidateIds];
    const request: OptimizerSizeEstimateRequest = {
      ...optimizerPlanRequest,
      ...backendSource,
      locale,
      sampleSeed: sampleSeedFromFingerprint(encodingFingerprint),
      candidateIds,
    };
    coordinator.scheduleEstimate({
      fingerprint: encodingFingerprint,
      requestKey: `optimizer:${encodingFingerprint}:${JSON.stringify(candidateIds)}`,
      run: async (options) =>
        validateCandidateEstimates(
          await runtime.estimateOptimizerCandidates(request, options),
          candidateIds,
        ),
    });
  }, [
    backendSource?.inputPath,
    backendSource?.sourceRevision,
    cropRegion,
    encodingFingerprint,
    inspection?.isStaticImage,
    inspection?.ok,
    locale,
    optimizerPlanRequest,
    plan,
    runtime,
    selectedCandidateIds,
  ]);

  const estimates = useMemo(() => {
    const estimateState = state.estimate;
    if (
      estimateState.fingerprint !== encodingFingerprint ||
      estimateState.status !== "ready"
    ) {
      return [];
    }
    return estimateState.value.map((estimate) => {
      if (estimate.candidateId === null) return estimate;
      return (
        state.exactEstimateCache.get(
          exactEstimateCacheKey(encodingFingerprint, estimate.candidateId),
        ) ?? estimate
      );
    });
  }, [encodingFingerprint, state.estimate, state.exactEstimateCache]);

  const estimateByCandidateId = useMemo(() => {
    const byCandidateId = new Map<string, OutputSizeEstimate>();
    for (const estimate of estimates) {
      if (estimate.candidateId !== null) {
        byCandidateId.set(estimate.candidateId, estimate);
      }
    }
    return byCandidateId;
  }, [estimates]);

  const retryEstimate = useCallback(() => {
    coordinatorRef.current?.retryEstimate();
  }, []);

  const probeCandidate = useCallback(
    (candidateId: string) => {
      const coordinator = coordinatorRef.current;
      const current = stateRef.current;
      if (
        coordinator === null ||
        runtime.kind === "web" ||
        !runtime.capabilities.backendProcessing ||
        backendSource === null ||
        optimizerPlanRequest === null ||
        current.estimate.fingerprint !== encodingFingerprint ||
        current.estimate.status !== "ready" ||
        !selectedCandidateIds.includes(candidateId)
      ) {
        return;
      }

      const estimate = current.estimate.value.find(
        (value) => value.candidateId === candidateId,
      );
      if (!isNearLimitSampledEstimate(estimate)) return;

      const request: CandidateSizeProbeRequest = {
        ...optimizerPlanRequest,
        ...backendSource,
        locale,
        candidateId,
      };
      coordinator.startProbe({
        fingerprint: encodingFingerprint,
        candidateId,
        run: async (options) => {
          const value = await runtime.probeOptimizerCandidateSize(
            request,
            options,
          );
          if (value.candidateId !== candidateId) {
            throw new Error(
              "Candidate probe returned a mismatched candidate ID.",
            );
          }
          return value;
        },
      });
    },
    [
      backendSource,
      encodingFingerprint,
      locale,
      optimizerPlanRequest,
      runtime,
      selectedCandidateIds,
    ],
  );

  const cancelProbe = useCallback(() => {
    coordinatorRef.current?.cancelProbe();
  }, []);

  return {
    state,
    estimates,
    estimateByCandidateId,
    selectedCandidateIds,
    retryEstimate,
    probeCandidate,
    cancelProbe,
  };
}
