use std::mem::size_of;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::media_error::PipelineError;
use crate::media_limits::SourceIdentity;

pub(crate) const MAX_PREVIEW_CACHE_BYTES: usize = 32 * 1024 * 1024;

const ARC_ALLOCATION_HEADER_BYTES: usize = 2 * size_of::<usize>();

#[derive(Debug, Clone)]
pub(crate) struct CachedPreview {
    pub(crate) source_frame_id: u32,
    pub(crate) png_bytes: Arc<[u8]>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PreviewVariant {
    Native,
    Video { width: u32, height: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PreviewSourceKey {
    pub(crate) identity: SourceIdentity,
    pub(crate) source_revision: String,
    pub(crate) variant: PreviewVariant,
}

#[derive(Debug, Clone)]
pub(crate) struct PreviewCachePublication {
    marker: Arc<()>,
}

impl PreviewCachePublication {
    fn new() -> Self {
        Self {
            marker: Arc::new(()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct CompletePreviewLookup {
    pub(crate) previews: Vec<CachedPreview>,
    pub(crate) publication: Option<PreviewCachePublication>,
}

#[derive(Debug)]
struct PreviewSourceGroup {
    key: PreviewSourceKey,
    previews: Vec<CachedPreview>,
    complete: bool,
    publication: Arc<()>,
}

#[derive(Debug, Default)]
struct PreviewCacheInner {
    // Oldest entries are kept at the front and the most recently used entry at the back.
    entries: Vec<PreviewSourceGroup>,
}

#[derive(Debug)]
pub(crate) struct PreviewCache {
    inner: Mutex<PreviewCacheInner>,
    max_bytes: usize,
}

pub(crate) fn checked_preview_cache_add(left: usize, right: usize) -> usize {
    left.checked_add(right).unwrap_or(usize::MAX)
}

fn checked_preview_cache_mul(left: usize, right: usize) -> usize {
    left.checked_mul(right).unwrap_or(usize::MAX)
}

pub(crate) fn accounted_cached_preview_bytes(preview: &CachedPreview) -> usize {
    let metadata_and_header =
        checked_preview_cache_add(size_of::<CachedPreview>(), ARC_ALLOCATION_HEADER_BYTES);
    checked_preview_cache_add(metadata_and_header, preview.png_bytes.len())
}

pub(crate) fn accounted_source_group_bytes(
    key: &PreviewSourceKey,
    previews: &Vec<CachedPreview>,
) -> usize {
    let mut total = checked_preview_cache_add(
        checked_preview_cache_add(
            size_of::<PreviewSourceGroup>(),
            size_of::<Vec<CachedPreview>>(),
        ),
        ARC_ALLOCATION_HEADER_BYTES,
    );
    total = checked_preview_cache_add(total, key.identity.canonical_path.capacity());
    total = checked_preview_cache_add(total, key.source_revision.capacity());
    total = checked_preview_cache_add(
        total,
        checked_preview_cache_mul(previews.capacity(), size_of::<CachedPreview>()),
    );
    for preview in previews {
        total = checked_preview_cache_add(total, accounted_cached_preview_bytes(preview));
    }
    total
}

fn accounted_cache_bytes(inner: &PreviewCacheInner) -> usize {
    let mut total =
        checked_preview_cache_add(size_of::<PreviewCache>(), ARC_ALLOCATION_HEADER_BYTES);
    total = checked_preview_cache_add(
        total,
        checked_preview_cache_mul(inner.entries.capacity(), size_of::<PreviewSourceGroup>()),
    );
    for group in &inner.entries {
        total = checked_preview_cache_add(
            total,
            accounted_source_group_bytes(&group.key, &group.previews),
        );
    }
    total
}

fn minimum_cache_bytes_with_group(group: &PreviewSourceGroup) -> usize {
    let base = checked_preview_cache_add(size_of::<PreviewCache>(), ARC_ALLOCATION_HEADER_BYTES);
    let outer_slot = size_of::<PreviewSourceGroup>();
    checked_preview_cache_add(
        checked_preview_cache_add(base, outer_slot),
        accounted_source_group_bytes(&group.key, &group.previews),
    )
}

fn compact_entries(inner: &mut PreviewCacheInner) {
    let entries = std::mem::take(&mut inner.entries);
    inner.entries = entries.into_boxed_slice().into_vec();
}

fn invalid_frame_selection() -> PipelineError {
    PipelineError::InvalidRequest {
        reason: "invalid-frame-selection",
    }
}

fn merge_preview(previews: &mut Vec<CachedPreview>, incoming: CachedPreview) {
    if let Some(existing) = previews
        .iter_mut()
        .find(|preview| preview.source_frame_id == incoming.source_frame_id)
    {
        *existing = incoming;
    } else {
        previews.push(incoming);
    }
}

fn deduplicate_previews(previews: Vec<CachedPreview>) -> Vec<CachedPreview> {
    let mut deduplicated = Vec::with_capacity(previews.capacity());
    for preview in previews {
        merge_preview(&mut deduplicated, preview);
    }
    deduplicated
}

fn select_requested_previews(
    previews: &[CachedPreview],
    requested_source_frame_ids: &[u32],
) -> Option<Vec<CachedPreview>> {
    if requested_source_frame_ids.is_empty() {
        return None;
    }
    requested_source_frame_ids
        .iter()
        .map(|source_frame_id| {
            previews
                .iter()
                .find(|preview| preview.source_frame_id == *source_frame_id)
                .cloned()
        })
        .collect()
}

impl PreviewCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(PreviewCacheInner::default()),
            max_bytes,
        }
    }

    fn lock_inner(&self) -> MutexGuard<'_, PreviewCacheInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub(crate) fn accounted_bytes(&self) -> usize {
        accounted_cache_bytes(&self.lock_inner())
    }

    pub(crate) fn contains_key(&self, key: &PreviewSourceKey) -> bool {
        self.lock_inner()
            .entries
            .iter()
            .any(|group| &group.key == key)
    }

    pub(crate) fn get_requested(
        &self,
        key: &PreviewSourceKey,
        requested_source_frame_ids: &[u32],
    ) -> Option<Vec<CachedPreview>> {
        if requested_source_frame_ids.is_empty() {
            return None;
        }
        let selected = self.get_available(key, requested_source_frame_ids);
        (selected.len() == requested_source_frame_ids.len()).then_some(selected)
    }

    pub(crate) fn get_available(
        &self,
        key: &PreviewSourceKey,
        requested_source_frame_ids: &[u32],
    ) -> Vec<CachedPreview> {
        if requested_source_frame_ids.is_empty() {
            return Vec::new();
        }
        let mut inner = self.lock_inner();
        let Some(index) = inner.entries.iter().position(|group| &group.key == key) else {
            return Vec::new();
        };
        let group = inner.entries.remove(index);
        let selected = requested_source_frame_ids
            .iter()
            .filter_map(|source_frame_id| {
                group
                    .previews
                    .iter()
                    .find(|preview| preview.source_frame_id == *source_frame_id)
                    .cloned()
            })
            .collect();
        inner.entries.push(group);
        selected
    }

    pub(crate) fn publish_complete(
        &self,
        key: PreviewSourceKey,
        previews: Vec<CachedPreview>,
    ) -> bool {
        self.publish_complete_with_publication(key, previews)
            .is_some()
    }

    fn publish_complete_with_publication(
        &self,
        key: PreviewSourceKey,
        previews: Vec<CachedPreview>,
    ) -> Option<PreviewCachePublication> {
        let mut inner = self.lock_inner();
        let publication = PreviewCachePublication::new();
        let group = PreviewSourceGroup {
            key,
            previews: deduplicate_previews(previews),
            complete: true,
            publication: Arc::clone(&publication.marker),
        };
        self.publish_group_locked(&mut inner, group)
            .then_some(publication)
    }

    pub(crate) fn merge_partial(
        &self,
        key: PreviewSourceKey,
        previews: Vec<CachedPreview>,
    ) -> Option<PreviewCachePublication> {
        let mut inner = self.lock_inner();
        let publication = PreviewCachePublication::new();
        let existing = inner
            .entries
            .iter()
            .position(|group| group.key == key)
            .map(|index| inner.entries.remove(index));
        let mut group = existing.unwrap_or_else(|| PreviewSourceGroup {
            key,
            previews: Vec::new(),
            complete: false,
            publication: Arc::clone(&publication.marker),
        });
        group.publication = Arc::clone(&publication.marker);
        for preview in previews {
            merge_preview(&mut group.previews, preview);
        }
        self.publish_group_locked(&mut inner, group)
            .then_some(publication)
    }

    pub(crate) fn get_or_try_build_complete<F>(
        &self,
        key: &PreviewSourceKey,
        requested_source_frame_ids: &[u32],
        builder: F,
    ) -> Result<CompletePreviewLookup, PipelineError>
    where
        F: FnOnce() -> Result<Vec<CachedPreview>, PipelineError>,
    {
        if requested_source_frame_ids.is_empty() {
            return Err(invalid_frame_selection());
        }

        {
            let mut inner = self.lock_inner();
            if let Some(index) = inner.entries.iter().position(|group| &group.key == key) {
                let group = inner.entries.remove(index);
                let selected =
                    select_requested_previews(&group.previews, requested_source_frame_ids);
                let complete = group.complete;
                inner.entries.push(group);
                if complete {
                    return selected
                        .map(|previews| CompletePreviewLookup {
                            previews,
                            publication: None,
                        })
                        .ok_or_else(invalid_frame_selection);
                }
            }
        }

        let previews = deduplicate_previews(builder()?);
        let selected = select_requested_previews(&previews, requested_source_frame_ids)
            .ok_or_else(invalid_frame_selection)?;
        let publication = self.publish_complete_with_publication(key.clone(), previews);
        Ok(CompletePreviewLookup {
            previews: selected,
            publication,
        })
    }

    pub(crate) fn invalidate(&self, key: &PreviewSourceKey) -> bool {
        let mut inner = self.lock_inner();
        let Some(index) = inner.entries.iter().position(|group| &group.key == key) else {
            return false;
        };
        inner.entries.remove(index);
        compact_entries(&mut inner);
        true
    }

    pub(crate) fn invalidate_publication(
        &self,
        key: &PreviewSourceKey,
        publication: &PreviewCachePublication,
    ) -> bool {
        let mut inner = self.lock_inner();
        let Some(index) = inner.entries.iter().position(|group| {
            &group.key == key && Arc::ptr_eq(&group.publication, &publication.marker)
        }) else {
            return false;
        };
        inner.entries.remove(index);
        compact_entries(&mut inner);
        true
    }

    fn publish_group_locked(
        &self,
        inner: &mut PreviewCacheInner,
        group: PreviewSourceGroup,
    ) -> bool {
        let key = group.key.clone();
        if minimum_cache_bytes_with_group(&group) > self.max_bytes {
            if let Some(index) = inner.entries.iter().position(|entry| entry.key == key) {
                inner.entries.remove(index);
                compact_entries(inner);
            }
            return false;
        }

        if let Some(index) = inner.entries.iter().position(|entry| entry.key == key) {
            inner.entries.remove(index);
        }
        inner.entries.push(group);
        compact_entries(inner);

        while accounted_cache_bytes(inner) > self.max_bytes && inner.entries.len() > 1 {
            inner.entries.remove(0);
            compact_entries(inner);
        }

        if accounted_cache_bytes(inner) > self.max_bytes {
            if let Some(index) = inner.entries.iter().position(|entry| entry.key == key) {
                inner.entries.remove(index);
                compact_entries(inner);
            }
            return false;
        }
        inner.entries.iter().any(|entry| entry.key == key)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::media_error::PipelineError;
    use crate::media_limits::SourceIdentity;

    use super::{
        accounted_cached_preview_bytes, accounted_source_group_bytes, checked_preview_cache_add,
        CachedPreview, PreviewCache, PreviewSourceGroup, PreviewSourceKey, PreviewVariant,
        ARC_ALLOCATION_HEADER_BYTES, MAX_PREVIEW_CACHE_BYTES,
    };

    fn identity(path: &str, file_len: u64, modified_nanos: u128) -> SourceIdentity {
        SourceIdentity {
            canonical_path: PathBuf::from(path),
            file_len,
            modified_nanos,
        }
    }

    fn key(
        path: &str,
        file_len: u64,
        modified_nanos: u128,
        revision: &str,
        variant: PreviewVariant,
    ) -> PreviewSourceKey {
        PreviewSourceKey {
            identity: identity(path, file_len, modified_nanos),
            source_revision: revision.into(),
            variant,
        }
    }

    fn preview(source_frame_id: u32, png_len: usize) -> CachedPreview {
        let bytes = (0..png_len)
            .map(|index| {
                (index as u8)
                    .wrapping_mul(31)
                    .wrapping_add(source_frame_id as u8)
            })
            .collect::<Vec<_>>();
        CachedPreview {
            source_frame_id,
            png_bytes: Arc::from(bytes.into_boxed_slice()),
            width: 128,
            height: 128,
        }
    }

    fn native_key(name: &str) -> PreviewSourceKey {
        key(
            &format!(r"C:\preview\{name}.gif"),
            1_024,
            10,
            "source-test",
            PreviewVariant::Native,
        )
    }

    #[test]
    fn serialized_batches_build_one_complete_native_group_while_resident() {
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let key = native_key("serialized");
        let build_count = Cell::new(0_u32);

        let first = cache
            .get_or_try_build_complete(&key, &[3, 1], || {
                build_count.set(build_count.get() + 1);
                Ok::<_, PipelineError>(vec![preview(1, 32), preview(2, 32), preview(3, 32)])
            })
            .expect("first batch must build the complete group");
        let second = cache
            .get_or_try_build_complete(&key, &[2], || -> Result<_, PipelineError> {
                panic!("resident complete group must not rebuild")
            })
            .expect("second batch must read the resident group");

        assert_eq!(build_count.get(), 1);
        assert!(first.publication.is_some());
        assert!(second.publication.is_none());
        assert_eq!(
            first
                .previews
                .iter()
                .map(|item| item.source_frame_id)
                .collect::<Vec<_>>(),
            vec![3, 1]
        );
        assert_eq!(second.previews[0].source_frame_id, 2);
    }

    #[test]
    fn identity_revision_and_output_variant_all_partition_cache_keys() {
        let base = key(
            r"C:\preview\partition.mp4",
            100,
            200,
            "source-a",
            PreviewVariant::Video {
                width: 128,
                height: 64,
            },
        );
        let keys = HashSet::from([
            base.clone(),
            key(
                r"C:\preview\partition.mp4",
                101,
                200,
                "source-a",
                PreviewVariant::Video {
                    width: 128,
                    height: 64,
                },
            ),
            key(
                r"C:\preview\partition.mp4",
                100,
                201,
                "source-a",
                PreviewVariant::Video {
                    width: 128,
                    height: 64,
                },
            ),
            key(
                r"C:\preview\partition.mp4",
                100,
                200,
                "source-b",
                PreviewVariant::Video {
                    width: 128,
                    height: 64,
                },
            ),
            key(
                r"C:\preview\partition.mp4",
                100,
                200,
                "source-a",
                PreviewVariant::Native,
            ),
            key(
                r"C:\preview\partition.mp4",
                100,
                200,
                "source-a",
                PreviewVariant::Video {
                    width: 64,
                    height: 128,
                },
            ),
        ]);

        assert_eq!(keys.len(), 6);
        assert!(keys.contains(&base));
    }

    #[test]
    fn accounting_charges_arc_allocations_vector_capacity_and_key_metadata() {
        let cached = preview(1, 1_024);
        assert!(
            accounted_cached_preview_bytes(&cached)
                > cached.png_bytes.len() + std::mem::size_of::<CachedPreview>()
        );

        let short_key = native_key("a");
        let long_key = native_key("a-very-long-source-name-that-must-be-accounted");
        let mut tight = Vec::with_capacity(1);
        tight.push(cached.clone());
        let mut spare = Vec::with_capacity(32);
        spare.push(cached);

        let expected_without_publication_header = std::mem::size_of::<PreviewSourceGroup>()
            + std::mem::size_of::<Vec<CachedPreview>>()
            + short_key.identity.canonical_path.capacity()
            + short_key.source_revision.capacity()
            + tight.capacity() * std::mem::size_of::<CachedPreview>()
            + tight
                .iter()
                .map(accounted_cached_preview_bytes)
                .sum::<usize>();
        assert_eq!(
            accounted_source_group_bytes(&short_key, &tight),
            expected_without_publication_header + ARC_ALLOCATION_HEADER_BYTES
        );

        assert!(
            accounted_source_group_bytes(&long_key, &tight)
                > accounted_source_group_bytes(&short_key, &tight)
        );
        assert!(
            accounted_source_group_bytes(&short_key, &spare)
                > accounted_source_group_bytes(&short_key, &tight)
        );
    }

    #[test]
    fn three_hundred_incompressible_128_edge_payloads_stay_within_32_mib() {
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let key = native_key("three-hundred");
        let previews = (1..=300)
            .map(|source_frame_id| preview(source_frame_id, 128 * 128 * 4 + 512))
            .collect::<Vec<_>>();

        assert!(cache.publish_complete(key.clone(), previews));
        assert!(cache.contains_key(&key));
        assert!(cache.accounted_bytes() <= MAX_PREVIEW_CACHE_BYTES);
    }

    #[test]
    fn accounting_overflow_saturates_to_an_over_budget_value() {
        assert_eq!(checked_preview_cache_add(usize::MAX, 1), usize::MAX);
        assert!(checked_preview_cache_add(MAX_PREVIEW_CACHE_BYTES, 1) > MAX_PREVIEW_CACHE_BYTES);
    }

    #[test]
    fn partial_video_batches_merge_without_losing_resident_ids() {
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let key = key(
            r"C:\preview\partial.mp4",
            1_024,
            10,
            "source-partial",
            PreviewVariant::Video {
                width: 128,
                height: 64,
            },
        );
        assert!(cache
            .merge_partial(key.clone(), vec![preview(1, 32)])
            .is_some());
        assert!(cache
            .merge_partial(key.clone(), vec![preview(3, 32)])
            .is_some());

        let merged = cache
            .get_requested(&key, &[3, 1])
            .expect("both partial batches must remain resident");
        assert_eq!(
            merged
                .iter()
                .map(|item| item.source_frame_id)
                .collect::<Vec<_>>(),
            vec![3, 1]
        );
    }

    #[test]
    fn lru_access_preserves_recent_entry_and_evicts_the_oldest() {
        let keys = [
            native_key("lru-a"),
            native_key("lru-b"),
            native_key("lru-c"),
        ];
        let probe = PreviewCache::new(usize::MAX);
        assert!(probe.publish_complete(keys[0].clone(), vec![preview(1, 512)]));
        assert!(probe.publish_complete(keys[1].clone(), vec![preview(1, 512)]));
        let two_entry_budget = probe.accounted_bytes();

        let cache = PreviewCache::new(two_entry_budget);
        assert!(cache.publish_complete(keys[0].clone(), vec![preview(1, 512)]));
        assert!(cache.publish_complete(keys[1].clone(), vec![preview(1, 512)]));
        assert!(cache.get_requested(&keys[0], &[1]).is_some());
        assert!(cache.publish_complete(keys[2].clone(), vec![preview(1, 512)]));

        assert!(cache.contains_key(&keys[0]));
        assert!(!cache.contains_key(&keys[1]));
        assert!(cache.contains_key(&keys[2]));
        assert!(cache.accounted_bytes() <= two_entry_budget);
    }

    #[test]
    fn replacement_updates_payload_without_double_charging_old_storage() {
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let key = native_key("replacement");
        assert!(cache.publish_complete(key.clone(), vec![preview(1, 128)]));
        let before = cache.accounted_bytes();

        assert!(cache.publish_complete(key.clone(), vec![preview(1, 1_024)]));
        let cached = cache
            .get_requested(&key, &[1])
            .expect("replacement must remain cached");

        assert_eq!(cached[0].png_bytes.len(), 1_024);
        assert!(cache.accounted_bytes() > before);
        assert!(cache.accounted_bytes() <= MAX_PREVIEW_CACHE_BYTES);
    }

    #[test]
    fn oversized_success_is_returned_transiently_without_cache_insertion() {
        let cache = PreviewCache::new(4_096);
        let key = native_key("oversized");

        let returned = cache
            .get_or_try_build_complete(&key, &[1], || {
                Ok::<_, PipelineError>(vec![preview(1, 8_192)])
            })
            .expect("oversized successful build must still serve this request");

        assert_eq!(returned.previews.len(), 1);
        assert!(returned.publication.is_none());
        assert!(!cache.contains_key(&key));
        assert!(cache.accounted_bytes() <= cache.max_bytes());
    }

    #[test]
    fn failed_or_cancelled_builder_is_not_cached_and_can_retry() {
        for first_error in [
            PipelineError::Cancelled,
            PipelineError::SourceChanged,
            PipelineError::Io {
                operation: "build preview cache",
                message: "fixture failure".into(),
            },
        ] {
            let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
            let key = native_key("retry");
            let build_count = Cell::new(0_u32);
            let error = cache
                .get_or_try_build_complete(&key, &[1], || {
                    build_count.set(build_count.get() + 1);
                    Err::<Vec<CachedPreview>, _>(first_error.clone())
                })
                .expect_err("first builder call must fail");
            assert_eq!(error, first_error);
            assert!(!cache.contains_key(&key));

            let retry = cache
                .get_or_try_build_complete(&key, &[1], || {
                    build_count.set(build_count.get() + 1);
                    Ok::<_, PipelineError>(vec![preview(1, 32)])
                })
                .expect("retry must rebuild after a non-cached failure");
            assert_eq!(retry.previews.len(), 1);
            assert!(retry.publication.is_some());
            assert_eq!(build_count.get(), 2);
            assert!(cache.contains_key(&key));
        }
    }

    #[test]
    fn older_publication_receipt_cannot_invalidate_a_newer_generation() {
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let key = native_key("publication-generation");
        let older = cache
            .publish_complete_with_publication(key.clone(), vec![preview(1, 32)])
            .expect("older publication must be resident");
        let newer = cache
            .publish_complete_with_publication(key.clone(), vec![preview(1, 64)])
            .expect("newer publication must replace the older generation");

        assert!(!cache.invalidate_publication(&key, &older));
        let resident = cache
            .get_requested(&key, &[1])
            .expect("stale receipt must not evict the newer publication");
        assert_eq!(resident[0].png_bytes.len(), 64);
        assert!(cache.invalidate_publication(&key, &newer));
        assert!(!cache.contains_key(&key));
    }

    #[test]
    fn matching_partial_publication_receipt_invalidates_the_current_generation() {
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let key = native_key("matching-publication");
        let publication = cache
            .merge_partial(key.clone(), vec![preview(1, 32)])
            .expect("partial publication must be resident");

        assert!(cache.invalidate_publication(&key, &publication));
        assert!(!cache.contains_key(&key));
    }

    #[test]
    fn cached_preview_clones_share_the_same_arc_payload() {
        let original = preview(1, 64);
        let cloned = original.clone();
        assert!(Arc::ptr_eq(&original.png_bytes, &cloned.png_bytes));
    }

    #[test]
    fn invalidation_after_publication_removes_the_whole_matching_key() {
        let cache = PreviewCache::new(MAX_PREVIEW_CACHE_BYTES);
        let key = native_key("post-publish-mutation");
        assert!(cache.publish_complete(key.clone(), vec![preview(1, 32), preview(2, 32)]));

        assert!(cache.invalidate(&key));
        assert!(!cache.contains_key(&key));
        assert!(cache.get_requested(&key, &[1]).is_none());
    }

    #[test]
    fn production_cache_types_never_store_full_resolution_rgba_images() {
        let source = include_str!("preview_cache.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production prefix must exist");
        assert!(!production.contains("RgbaImage"));
        assert!(production.contains("Arc<[u8]>"));
    }
}
