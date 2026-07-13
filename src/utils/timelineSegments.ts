import type { TimelineFrameView } from "../types/editor";

export const MAX_TIMELINE_SEGMENT_BUCKETS = 200;

export type TimelineSegmentBucket = {
  startUs: number;
  durationUs: number;
  firstFrameIndex: number;
  lastFrameIndex: number;
  containsCurrent: boolean;
  containsSelected: boolean;
};

type BuildTimelineSegmentBucketsInput = {
  frames: readonly TimelineFrameView[];
  currentTimeUs: number;
  selectedInstanceIds: ReadonlySet<string>;
  maxBuckets?: number;
};

function normalizeBucketLimit(maxBuckets: number | undefined) {
  if (maxBuckets === undefined || !Number.isFinite(maxBuckets)) {
    return MAX_TIMELINE_SEGMENT_BUCKETS;
  }

  return Math.min(
    MAX_TIMELINE_SEGMENT_BUCKETS,
    Math.max(1, Math.floor(maxBuckets)),
  );
}

function frameContainsTime(frame: TimelineFrameView, currentTimeUs: number) {
  return (
    currentTimeUs >= frame.startTimeUs &&
    currentTimeUs < frame.startTimeUs + frame.durationUs
  );
}

function buildBucket(
  frames: readonly TimelineFrameView[],
  firstFrameIndex: number,
  endFrameIndex: number,
  currentTimeUs: number,
  selectedInstanceIds: ReadonlySet<string>,
): TimelineSegmentBucket {
  let durationUs = 0;
  let containsCurrent = false;
  let containsSelected = false;

  for (let index = firstFrameIndex; index < endFrameIndex; index += 1) {
    const frame = frames[index];
    durationUs += frame.durationUs;
    containsCurrent ||= frameContainsTime(frame, currentTimeUs);
    containsSelected ||= selectedInstanceIds.has(frame.instanceId);
  }

  return {
    startUs: frames[firstFrameIndex].startTimeUs,
    durationUs,
    firstFrameIndex,
    lastFrameIndex: endFrameIndex - 1,
    containsCurrent,
    containsSelected,
  };
}

function countWeightedEndIndex(
  frameCount: number,
  bucketIndex: number,
  bucketCount: number,
) {
  return Math.ceil(((bucketIndex + 1) * frameCount) / bucketCount);
}

function durationWeightedEndIndex(
  frames: readonly TimelineFrameView[],
  firstFrameIndex: number,
  bucketIndex: number,
  bucketCount: number,
  totalDurationUs: number,
  durationBeforeBucketUs: number,
) {
  const remainingBucketCount = bucketCount - bucketIndex - 1;
  const maximumEndIndex = frames.length - remainingBucketCount;
  if (remainingBucketCount === 0) {
    return frames.length;
  }

  const targetEndUs = (totalDurationUs * (bucketIndex + 1)) / bucketCount;
  let endFrameIndex = firstFrameIndex + 1;
  let candidateEndUs =
    durationBeforeBucketUs + frames[firstFrameIndex].durationUs;

  while (endFrameIndex < maximumEndIndex) {
    const nextEndUs = candidateEndUs + frames[endFrameIndex].durationUs;
    const candidateDistance = Math.abs(candidateEndUs - targetEndUs);
    const nextDistance = Math.abs(nextEndUs - targetEndUs);
    if (nextDistance > candidateDistance) {
      break;
    }

    candidateEndUs = nextEndUs;
    endFrameIndex += 1;
  }

  return endFrameIndex;
}

export function buildTimelineSegmentBuckets({
  frames,
  currentTimeUs,
  selectedInstanceIds,
  maxBuckets,
}: BuildTimelineSegmentBucketsInput): TimelineSegmentBucket[] {
  if (frames.length === 0) {
    return [];
  }

  const bucketCount = Math.min(frames.length, normalizeBucketLimit(maxBuckets));
  const totalDurationUs = frames.reduce(
    (sum, frame) => sum + frame.durationUs,
    0,
  );
  const useDurationWeights =
    Number.isFinite(totalDurationUs) && totalDurationUs > 0;
  const buckets: TimelineSegmentBucket[] = [];
  let firstFrameIndex = 0;
  let durationBeforeBucketUs = 0;

  for (let bucketIndex = 0; bucketIndex < bucketCount; bucketIndex += 1) {
    const endFrameIndex = useDurationWeights
      ? durationWeightedEndIndex(
          frames,
          firstFrameIndex,
          bucketIndex,
          bucketCount,
          totalDurationUs,
          durationBeforeBucketUs,
        )
      : countWeightedEndIndex(frames.length, bucketIndex, bucketCount);
    const bucket = buildBucket(
      frames,
      firstFrameIndex,
      endFrameIndex,
      currentTimeUs,
      selectedInstanceIds,
    );

    buckets.push(bucket);
    durationBeforeBucketUs += bucket.durationUs;
    firstFrameIndex = endFrameIndex;
  }

  return buckets;
}
