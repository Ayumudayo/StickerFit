import type {
  FramePreviewRequest,
  FramePreviewsRequest,
  MediaInspection,
  OptimizerSearchRequest,
  StaticImageConversionRequest,
} from "../../types/workflow";

type PathBackedInspection = Pick<
  MediaInspection,
  "ok" | "backendInputPath" | "sourceRevision"
>;

type OptimizerSearchFields = Omit<
  OptimizerSearchRequest,
  "inputPath" | "sourceRevision"
>;
type StaticImageConversionFields = Omit<
  StaticImageConversionRequest,
  "inputPath" | "sourceRevision"
>;
type FramePreviewFields = Omit<
  FramePreviewRequest,
  "inputPath" | "sourceRevision"
>;
type FramePreviewsFields = Omit<
  FramePreviewsRequest,
  "inputPath" | "sourceRevision"
>;

function sourceIdentity(inspection: PathBackedInspection | null | undefined) {
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

export function buildOptimizerSearchRequest(
  inspection: PathBackedInspection | null | undefined,
  fields: OptimizerSearchFields,
): OptimizerSearchRequest | null {
  const source = sourceIdentity(inspection);
  return source ? { ...fields, ...source } : null;
}

export function buildStaticImageConversionRequest(
  inspection: PathBackedInspection | null | undefined,
  fields: StaticImageConversionFields,
): StaticImageConversionRequest | null {
  const source = sourceIdentity(inspection);
  return source ? { ...fields, ...source } : null;
}

export function buildFramePreviewRequest(
  inspection: PathBackedInspection | null | undefined,
  fields: FramePreviewFields,
): FramePreviewRequest | null {
  const source = sourceIdentity(inspection);
  return source ? { ...fields, ...source } : null;
}

export function buildFramePreviewsRequest(
  inspection: PathBackedInspection | null | undefined,
  fields: FramePreviewsFields,
): FramePreviewsRequest | null {
  const source = sourceIdentity(inspection);
  return source ? { ...fields, ...source } : null;
}
