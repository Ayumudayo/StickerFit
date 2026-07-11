use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::media_error::PipelineError;

const MAX_OPERATION_ID_BYTES: usize = 128;
const TOMBSTONE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PRE_CANCELLED: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PermitProfile {
    None,
    Decode,
    OutputDecode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaOperationKind {
    Inspect,
    BuildPlan,
    Preview,
    StaticConversion,
    OptimizerSearch,
}

impl MediaOperationKind {
    pub(crate) fn timeout(self) -> Duration {
        match self {
            Self::Inspect | Self::BuildPlan => Duration::from_secs(15),
            Self::Preview => Duration::from_secs(30),
            Self::StaticConversion => Duration::from_secs(60),
            Self::OptimizerSearch => Duration::from_secs(180),
        }
    }

    pub(crate) fn permit_profile(self) -> PermitProfile {
        match self {
            Self::BuildPlan => PermitProfile::None,
            Self::Inspect | Self::Preview => PermitProfile::Decode,
            Self::StaticConversion | Self::OptimizerSearch => PermitProfile::OutputDecode,
        }
    }
}

#[derive(Clone)]
pub(crate) struct OperationContext {
    operation_id: Arc<str>,
    deadline: Instant,
    cancellation: CancellationToken,
}

impl OperationContext {
    fn new(operation_id: String, deadline: Instant) -> Self {
        Self {
            operation_id: operation_id.into(),
            deadline,
            cancellation: CancellationToken::new(),
        }
    }

    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) fn checkpoint(&self) -> Result<(), PipelineError> {
        if self.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(PipelineError::TimedOut { stage: "operation" });
        }
        Ok(())
    }
}

struct ActiveOperation {
    generation: u64,
    context: OperationContext,
}

struct RegistryState {
    active: HashMap<String, ActiveOperation>,
    pre_cancelled: HashMap<String, Instant>,
    next_generation: u64,
}

impl Default for RegistryState {
    fn default() -> Self {
        Self {
            active: HashMap::new(),
            pre_cancelled: HashMap::new(),
            next_generation: 1,
        }
    }
}

impl RegistryState {
    fn prune_tombstones(&mut self, now: Instant) {
        self.pre_cancelled.retain(
            |_, created_at| match now.checked_duration_since(*created_at) {
                Some(age) => age < TOMBSTONE_TTL,
                None => true,
            },
        );
    }

    fn enforce_tombstone_cap(&mut self) {
        while self.pre_cancelled.len() > MAX_PRE_CANCELLED {
            let oldest = self
                .pre_cancelled
                .iter()
                .min_by(|(left_id, left_time), (right_id, right_time)| {
                    left_time
                        .cmp(right_time)
                        .then_with(|| left_id.cmp(right_id))
                })
                .map(|(operation_id, _)| operation_id.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.pre_cancelled.remove(&oldest);
        }
    }
}

#[derive(Clone, Default)]
struct OperationRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl OperationRegistry {
    fn new() -> Self {
        Self::default()
    }

    fn lock_state(&self) -> MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn register(
        &self,
        operation_id: &str,
        timeout: Duration,
    ) -> Result<RegisteredOperation, PipelineError> {
        let now = Instant::now();
        self.register_at(operation_id, now, now + timeout)
    }

    fn register_at(
        &self,
        operation_id: &str,
        now: Instant,
        deadline: Instant,
    ) -> Result<RegisteredOperation, PipelineError> {
        validate_operation_id(operation_id)?;

        let mut state = self.lock_state();
        state.prune_tombstones(now);
        if state.pre_cancelled.remove(operation_id).is_some() {
            return Err(PipelineError::Cancelled);
        }
        if state.active.contains_key(operation_id) {
            return Err(PipelineError::OperationConflict {
                operation_id: operation_id.into(),
            });
        }

        let generation = state.next_generation;
        state.next_generation =
            state
                .next_generation
                .checked_add(1)
                .ok_or_else(|| PipelineError::Io {
                    operation: "allocate operation generation",
                    message: "operation generation counter exhausted".into(),
                })?;
        let context = OperationContext::new(operation_id.into(), deadline);
        state.active.insert(
            operation_id.into(),
            ActiveOperation {
                generation,
                context: context.clone(),
            },
        );

        Ok(RegisteredOperation {
            registry: self.clone(),
            operation_id: operation_id.into(),
            generation,
            context,
        })
    }

    fn cancel(&self, operation_id: &str) -> bool {
        self.cancel_at(operation_id, Instant::now())
    }

    fn cancel_at(&self, operation_id: &str, now: Instant) -> bool {
        if validate_operation_id(operation_id).is_err() {
            return false;
        }

        let mut state = self.lock_state();
        state.prune_tombstones(now);
        if let Some(active) = state.active.get(operation_id) {
            active.context.cancel();
            return true;
        }

        state.pre_cancelled.insert(operation_id.into(), now);
        state.enforce_tombstone_cap();
        true
    }
}

fn validate_operation_id(operation_id: &str) -> Result<(), PipelineError> {
    if operation_id.is_empty() || operation_id.len() > MAX_OPERATION_ID_BYTES {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-operation-id",
        });
    }
    Ok(())
}

pub(crate) struct RegisteredOperation {
    registry: OperationRegistry,
    operation_id: String,
    generation: u64,
    context: OperationContext,
}

impl RegisteredOperation {
    pub(crate) fn context(&self) -> &OperationContext {
        &self.context
    }
}

impl Drop for RegisteredOperation {
    fn drop(&mut self) {
        let mut state = self.registry.lock_state();
        if state
            .active
            .get(&self.operation_id)
            .is_some_and(|active| active.generation == self.generation)
        {
            state.active.remove(&self.operation_id);
        }
    }
}

#[derive(Clone)]
pub(crate) struct PipelineState {
    registry: OperationRegistry,
    output: Arc<Semaphore>,
    estimate: Arc<Semaphore>,
    decode: Arc<Semaphore>,
}

impl PipelineState {
    pub(crate) fn new() -> Self {
        Self {
            registry: OperationRegistry::new(),
            output: Arc::new(Semaphore::new(1)),
            estimate: Arc::new(Semaphore::new(1)),
            decode: Arc::new(Semaphore::new(2)),
        }
    }

    pub(crate) fn register(
        &self,
        operation_id: &str,
        timeout: Duration,
    ) -> Result<RegisteredOperation, PipelineError> {
        self.registry.register(operation_id, timeout)
    }

    pub(crate) fn cancel(&self, operation_id: &str) -> bool {
        self.registry.cancel(operation_id)
    }

    pub(crate) async fn acquire_decode(
        &self,
        context: &OperationContext,
    ) -> Result<OwnedSemaphorePermit, PipelineError> {
        acquire_permit(Arc::clone(&self.decode), context).await
    }

    pub(crate) async fn acquire_output_then_decode(
        &self,
        context: &OperationContext,
    ) -> Result<OutputDecodePermits, PipelineError> {
        let output = acquire_permit(Arc::clone(&self.output), context).await?;
        let decode = acquire_permit(Arc::clone(&self.decode), context).await?;
        Ok(OutputDecodePermits {
            _output: output,
            _decode: decode,
        })
    }

    pub(crate) async fn acquire_estimate_then_decode(
        &self,
        context: &OperationContext,
    ) -> Result<EstimateDecodePermits, PipelineError> {
        let estimate = acquire_permit(Arc::clone(&self.estimate), context).await?;
        let decode = acquire_permit(Arc::clone(&self.decode), context).await?;
        Ok(EstimateDecodePermits {
            _estimate: estimate,
            _decode: decode,
        })
    }
}

pub(crate) struct OutputDecodePermits {
    _output: OwnedSemaphorePermit,
    _decode: OwnedSemaphorePermit,
}

pub(crate) struct EstimateDecodePermits {
    _estimate: OwnedSemaphorePermit,
    _decode: OwnedSemaphorePermit,
}

pub(crate) async fn run_managed_blocking<T, P, F>(
    registration: RegisteredOperation,
    permits: P,
    job: F,
) -> Result<T, String>
where
    T: Send + 'static,
    P: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _registration = registration;
        let _permits = permits;
        job()
    })
    .await
    .map_err(|error| error.to_string())
}

async fn acquire_permit(
    semaphore: Arc<Semaphore>,
    context: &OperationContext,
) -> Result<OwnedSemaphorePermit, PipelineError> {
    context.checkpoint()?;
    let cancellation = context.cancellation();
    let acquisition = cancellation.run_until_cancelled(semaphore.acquire_owned());
    let deadline = tokio::time::Instant::from_std(context.deadline());

    match tokio::time::timeout_at(deadline, acquisition).await {
        Ok(Some(Ok(permit))) => {
            context.checkpoint()?;
            Ok(permit)
        }
        Ok(None) => Err(PipelineError::Cancelled),
        Err(_) if context.is_cancelled() => Err(PipelineError::Cancelled),
        Err(_) => Err(PipelineError::TimedOut {
            stage: "permit-wait",
        }),
        Ok(Some(Err(error))) => {
            context.checkpoint()?;
            Err(PipelineError::Io {
                operation: "acquire pipeline permit",
                message: error.to_string(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ProgressStage {
    Queued,
    Inspecting,
    Decoding,
    Estimating,
    Encoding,
    Finalizing,
}

impl ProgressStage {
    fn message_code(self) -> &'static str {
        match self {
            Self::Queued => "media-operation-queued",
            Self::Inspecting => "media-operation-inspecting",
            Self::Decoding => "media-operation-decoding",
            Self::Estimating => "media-operation-estimating",
            Self::Encoding => "media-operation-encoding",
            Self::Finalizing => "media-operation-finalizing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperationProgress {
    operation_id: String,
    stage: ProgressStage,
    completed: u32,
    total: Option<u32>,
    message_code: &'static str,
}

pub(crate) trait ProgressSink: Send + Sync {
    fn send(&self, progress: OperationProgress);
}

pub(crate) struct ChannelProgressSink {
    channel: Channel<OperationProgress>,
}

impl ChannelProgressSink {
    pub(crate) fn new(channel: Channel<OperationProgress>) -> Self {
        Self { channel }
    }
}

impl ProgressSink for ChannelProgressSink {
    fn send(&self, progress: OperationProgress) {
        let _ = self.channel.send(progress);
    }
}

pub(crate) fn publish_progress(
    sink: &(impl ProgressSink + ?Sized),
    context: &OperationContext,
    stage: ProgressStage,
    completed: u32,
    total: Option<u32>,
) {
    sink.send(OperationProgress {
        operation_id: context.operation_id().into(),
        stage,
        completed,
        total,
        message_code: stage.message_code(),
    });
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime")
    }

    fn future_deadline() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    async fn within_test_timeout<F: Future>(future: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(1), future)
            .await
            .expect("test future must settle within one second")
    }

    #[derive(Default)]
    struct RecordingProgressSink {
        values: Mutex<Vec<OperationProgress>>,
    }

    impl ProgressSink for RecordingProgressSink {
        fn send(&self, progress: OperationProgress) {
            self.values
                .lock()
                .expect("recording sink lock")
                .push(progress);
        }
    }

    #[test]
    fn checkpoint_prefers_cancellation_over_an_elapsed_deadline() {
        let context = OperationContext::new(
            "cancelled-and-expired".into(),
            Instant::now() - Duration::from_millis(1),
        );
        context.cancel();

        assert_eq!(context.checkpoint(), Err(PipelineError::Cancelled));
    }

    #[test]
    fn checkpoint_reports_an_already_elapsed_deadline() {
        let context =
            OperationContext::new("expired".into(), Instant::now() - Duration::from_millis(1));

        assert!(matches!(
            context.checkpoint(),
            Err(PipelineError::TimedOut { stage: "operation" })
        ));
    }

    #[test]
    fn registered_operation_drop_cleans_up_its_active_entry() {
        let registry = OperationRegistry::new();
        let registered = registry
            .register("drop-cleanup", Duration::from_secs(10))
            .expect("operation registration");

        assert_eq!(registry.lock_state().active.len(), 1);
        drop(registered);
        assert_eq!(registry.lock_state().active.len(), 0);
    }

    #[test]
    fn stale_registered_drop_cannot_remove_a_new_generation() {
        let registry = OperationRegistry::new();
        let registered = registry
            .register("generation-safe", Duration::from_secs(10))
            .expect("operation registration");
        let stale_generation = registered.generation;
        let replacement_context =
            OperationContext::new("generation-safe".into(), future_deadline());

        {
            let mut state = registry.lock_state();
            state.active.insert(
                "generation-safe".into(),
                ActiveOperation {
                    generation: stale_generation + 1,
                    context: replacement_context.clone(),
                },
            );
        }

        drop(registered);
        let state = registry.lock_state();
        let active = state
            .active
            .get("generation-safe")
            .expect("replacement generation must remain active");
        assert_eq!(active.generation, stale_generation + 1);
        assert_eq!(active.context.operation_id(), "generation-safe");
    }

    #[test]
    fn duplicate_operation_id_is_rejected() {
        let registry = OperationRegistry::new();
        let _registered = registry
            .register("duplicate", Duration::from_secs(10))
            .expect("first registration");

        assert!(matches!(
            registry.register("duplicate", Duration::from_secs(10)),
            Err(PipelineError::OperationConflict { operation_id })
                if operation_id == "duplicate"
        ));
    }

    #[test]
    fn pre_cancel_is_consumed_by_registration() {
        let registry = OperationRegistry::new();
        assert!(registry.cancel("pre-cancelled"));
        assert_eq!(registry.lock_state().pre_cancelled.len(), 1);

        assert!(matches!(
            registry.register("pre-cancelled", Duration::from_secs(10)),
            Err(PipelineError::Cancelled)
        ));
        assert_eq!(registry.lock_state().active.len(), 0);
        assert_eq!(registry.lock_state().pre_cancelled.len(), 0);
    }

    #[test]
    fn cancelling_an_active_registration_cancels_its_token_without_a_tombstone() {
        let registry = OperationRegistry::new();
        let registered = registry
            .register("active-cancel", Duration::from_secs(10))
            .expect("operation registration");

        assert!(registry.cancel("active-cancel"));

        assert!(registered.context().is_cancelled());
        assert!(registry.lock_state().pre_cancelled.is_empty());
    }

    #[test]
    fn concurrent_register_and_cancel_cannot_lose_cancellation() {
        for iteration in 0..32 {
            let registry = OperationRegistry::new();
            let operation_id = format!("race-{iteration}");
            let barrier = Arc::new(Barrier::new(3));

            let register_registry = registry.clone();
            let register_id = operation_id.clone();
            let register_barrier = Arc::clone(&barrier);
            let register_thread = thread::spawn(move || {
                register_barrier.wait();
                register_registry.register(&register_id, Duration::from_secs(10))
            });

            let cancel_registry = registry.clone();
            let cancel_id = operation_id.clone();
            let cancel_barrier = Arc::clone(&barrier);
            let cancel_thread = thread::spawn(move || {
                cancel_barrier.wait();
                cancel_registry.cancel(&cancel_id)
            });

            barrier.wait();
            let registered = register_thread.join().expect("register thread");
            assert!(cancel_thread.join().expect("cancel thread"));

            match registered {
                Ok(registered) => assert!(registered.context().is_cancelled()),
                Err(error) => assert_eq!(error, PipelineError::Cancelled),
            }
        }
    }

    #[test]
    fn tombstones_expire_after_five_minutes_without_sleeping() {
        let registry = OperationRegistry::new();
        let started = Instant::now();
        assert!(registry.cancel_at("expired-tombstone", started));

        let registered = registry
            .register_at(
                "expired-tombstone",
                started + TOMBSTONE_TTL,
                started + TOMBSTONE_TTL + Duration::from_secs(10),
            )
            .expect("expired tombstone must not pre-cancel registration");

        assert_eq!(registered.context().operation_id(), "expired-tombstone");
        assert_eq!(registry.lock_state().pre_cancelled.len(), 0);
    }

    #[test]
    fn tombstones_are_capped_by_evicting_the_oldest_entry() {
        let registry = OperationRegistry::new();
        let started = Instant::now();

        for index in 0..=MAX_PRE_CANCELLED {
            assert!(registry.cancel_at(
                &format!("tombstone-{index}"),
                started + Duration::from_nanos(index as u64),
            ));
        }

        let state = registry.lock_state();
        assert_eq!(state.pre_cancelled.len(), MAX_PRE_CANCELLED);
        assert!(!state.pre_cancelled.contains_key("tombstone-0"));
        assert!(state
            .pre_cancelled
            .contains_key(&format!("tombstone-{MAX_PRE_CANCELLED}")));
    }

    #[test]
    fn cancelling_an_inactive_id_refreshes_its_tombstone_timestamp() {
        let registry = OperationRegistry::new();
        let started = Instant::now();
        assert!(registry.cancel_at("refreshed", started));
        assert!(registry.cancel_at("refreshed", started + TOMBSTONE_TTL / 2));

        assert!(matches!(
            registry.register_at(
                "refreshed",
                started + TOMBSTONE_TTL,
                started + TOMBSTONE_TTL + Duration::from_secs(60),
            ),
            Err(PipelineError::Cancelled)
        ));
        assert!(registry.lock_state().pre_cancelled.is_empty());
    }

    #[test]
    fn invalid_operation_ids_never_allocate_tombstones() {
        let registry = OperationRegistry::new();
        let maximum = "x".repeat(MAX_OPERATION_ID_BYTES);
        let oversized = "x".repeat(MAX_OPERATION_ID_BYTES + 1);

        let accepted = registry
            .register(&maximum, Duration::from_secs(10))
            .expect("128-byte operation ID must be accepted");
        drop(accepted);
        assert!(!registry.cancel(""));
        assert!(!registry.cancel(&oversized));
        assert_eq!(registry.lock_state().pre_cancelled.len(), 0);
        assert!(matches!(
            registry.register("", Duration::from_secs(10)),
            Err(PipelineError::InvalidRequest { .. })
        ));
        assert!(matches!(
            registry.register(&oversized, Duration::from_secs(10)),
            Err(PipelineError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn pipeline_semaphore_capacities_are_one_one_two() {
        let state = PipelineState::new();

        assert_eq!(state.output.available_permits(), 1);
        assert_eq!(state.estimate.available_permits(), 1);
        assert_eq!(state.decode.available_permits(), 2);
    }

    #[test]
    fn command_kinds_keep_fixed_deadlines_and_permit_profiles() {
        let cases = [
            (
                MediaOperationKind::Inspect,
                Duration::from_secs(15),
                PermitProfile::Decode,
            ),
            (
                MediaOperationKind::BuildPlan,
                Duration::from_secs(15),
                PermitProfile::None,
            ),
            (
                MediaOperationKind::Preview,
                Duration::from_secs(30),
                PermitProfile::Decode,
            ),
            (
                MediaOperationKind::StaticConversion,
                Duration::from_secs(60),
                PermitProfile::OutputDecode,
            ),
            (
                MediaOperationKind::OptimizerSearch,
                Duration::from_secs(180),
                PermitProfile::OutputDecode,
            ),
        ];

        for (kind, deadline, permits) in cases {
            assert_eq!(kind.timeout(), deadline);
            assert_eq!(kind.permit_profile(), permits);
        }
    }

    #[test]
    fn tauri_builder_and_command_scope_are_statically_wired() {
        let source = include_str!("lib.rs");
        assert!(source.contains(".manage(PipelineState::new())"));
        assert!(source.contains("cancel_media_operation,"));

        let managed_commands = [
            (
                "inspect_input_media",
                "MediaOperationKind::Inspect",
                Some("acquire_decode"),
            ),
            (
                "build_optimizer_plan",
                "MediaOperationKind::BuildPlan",
                None,
            ),
            (
                "convert_static_image_to_png",
                "MediaOperationKind::StaticConversion",
                Some("acquire_output_then_decode"),
            ),
            (
                "run_optimizer_search",
                "MediaOperationKind::OptimizerSearch",
                Some("acquire_output_then_decode"),
            ),
            (
                "extract_frame_preview",
                "MediaOperationKind::Preview",
                Some("acquire_decode"),
            ),
            (
                "extract_frame_previews",
                "MediaOperationKind::Preview",
                Some("acquire_decode"),
            ),
        ];

        for (command, operation_kind, permit_method) in managed_commands {
            let block = command_source_block(source, command);
            assert!(block.contains("operation_id: String"), "{command}");
            assert!(
                block.contains("on_progress: Channel<OperationProgress>"),
                "{command}"
            );
            assert!(block.contains("State<'_, PipelineState>"), "{command}");
            assert!(block.contains(operation_kind), "{command}");
            assert_eq!(
                block.matches("MediaOperationKind::").count(),
                1,
                "{command}"
            );
            assert!(block.contains("run_managed_blocking("), "{command}");

            match permit_method {
                Some(permit_method) => {
                    assert!(block.contains(permit_method), "{command}");
                    assert_eq!(block.matches(".acquire_").count(), 1, "{command}");
                }
                None => assert!(!block.contains(".acquire_"), "{command}"),
            }
        }

        for command in ["check_media_tools", "open_folder_path"] {
            let block = command_source_block(source, command);
            assert!(!block.contains("operation_id: String"), "{command}");
            assert!(!block.contains("on_progress"), "{command}");
            assert!(!block.contains("PipelineState"), "{command}");
            assert!(!block.contains("MediaOperationKind::"), "{command}");
            assert!(!block.contains(".register("), "{command}");
            assert!(!block.contains(".acquire_"), "{command}");
            assert!(!block.contains("run_managed_blocking("), "{command}");
        }

        let cancel_block = command_source_block(source, "cancel_media_operation");
        assert!(cancel_block.contains("pipeline_state.cancel(&operation_id)"));
        assert!(!cancel_block.contains("on_progress"));
        assert!(!cancel_block.contains("MediaOperationKind::"));
        assert!(!cancel_block.contains(".register("));
        assert!(!cancel_block.contains(".acquire_"));
        assert!(!cancel_block.contains("run_managed_blocking("));
    }

    fn command_source_block<'a>(source: &'a str, command: &str) -> &'a str {
        let marker = format!("fn {command}(");
        let start = source.find(&marker).expect("command source marker");
        let remainder = &source[start..];
        let tail = &remainder[marker.len()..];
        let end = ["#[tauri::command]", "\npub fn run("]
            .into_iter()
            .filter_map(|next_marker| tail.find(next_marker))
            .min()
            .map(|offset| marker.len() + offset)
            .unwrap_or(remainder.len());
        &remainder[..end]
    }

    #[test]
    fn output_is_acquired_before_decode_and_released_when_second_wait_is_cancelled() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let _decode_a = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("first decode permit");
            let _decode_b = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("second decode permit");
            let registered = state
                .register("output-before-decode", Duration::from_secs(10))
                .expect("operation registration");
            let context = registered.context().clone();
            let waiter_state = state.clone();
            let waiter_context = context.clone();
            let waiter = tokio::spawn(async move {
                waiter_state
                    .acquire_output_then_decode(&waiter_context)
                    .await
            });

            for _ in 0..16 {
                if state.output.available_permits() == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(state.output.available_permits(), 0);
            assert_eq!(state.decode.available_permits(), 0);

            context.cancel();
            let wait_result = within_test_timeout(waiter)
                .await
                .expect("permit waiter task");
            assert!(matches!(wait_result, Err(PipelineError::Cancelled)));
            assert_eq!(state.output.available_permits(), 1);
        });
    }

    #[test]
    fn output_permit_excludes_a_second_output_operation_until_cancelled() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let first = state
                .register("first-output", Duration::from_secs(10))
                .expect("first operation registration");
            let first_permits = state
                .acquire_output_then_decode(first.context())
                .await
                .expect("first output/decode permits");
            let second = state
                .register("second-output", Duration::from_secs(10))
                .expect("second operation registration");
            let second_context = second.context().clone();
            let waiter_state = state.clone();
            let waiter_context = second_context.clone();
            let waiter = tokio::spawn(async move {
                waiter_state
                    .acquire_output_then_decode(&waiter_context)
                    .await
            });

            tokio::task::yield_now().await;
            assert_eq!(state.output.available_permits(), 0);
            assert_eq!(state.decode.available_permits(), 1);

            second_context.cancel();
            let wait_result = within_test_timeout(waiter)
                .await
                .expect("second output waiter task");
            assert!(matches!(wait_result, Err(PipelineError::Cancelled)));
            assert_eq!(state.output.available_permits(), 0);

            drop(first_permits);
            assert_eq!(state.output.available_permits(), 1);
            assert_eq!(state.decode.available_permits(), 2);
        });
    }

    #[test]
    fn first_permit_is_released_when_the_second_semaphore_is_closed() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            state.decode.close();
            let registered = state
                .register("closed-decode", Duration::from_secs(10))
                .expect("operation registration");

            assert!(matches!(
                state.acquire_output_then_decode(registered.context()).await,
                Err(PipelineError::Io {
                    operation: "acquire pipeline permit",
                    ..
                })
            ));
            assert_eq!(state.output.available_permits(), 1);
        });
    }

    #[test]
    fn permit_wait_observes_cancellation() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let _decode_a = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("first decode permit");
            let _decode_b = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("second decode permit");
            let registered = state
                .register("cancel-wait", Duration::from_secs(10))
                .expect("operation registration");
            let context = registered.context().clone();
            let waiter_state = state.clone();
            let waiter_context = context.clone();
            let waiter =
                tokio::spawn(async move { waiter_state.acquire_decode(&waiter_context).await });

            tokio::task::yield_now().await;
            context.cancel();

            let wait_result = within_test_timeout(waiter)
                .await
                .expect("permit waiter task");
            assert!(matches!(wait_result, Err(PipelineError::Cancelled)));
        });
    }

    #[test]
    fn permit_wait_observes_a_future_deadline_while_capacity_is_held() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let _decode_a = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("first decode permit");
            let _decode_b = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("second decode permit");
            let registered = state
                .register("deadline-wait", Duration::from_millis(200))
                .expect("operation registration");

            let wait_result = within_test_timeout(state.acquire_decode(registered.context())).await;
            assert!(matches!(
                wait_result,
                Err(PipelineError::TimedOut {
                    stage: "permit-wait"
                })
            ));
        });
    }

    #[test]
    fn estimate_is_acquired_before_decode_and_released_when_second_wait_is_cancelled() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let _decode_a = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("first decode permit");
            let _decode_b = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("second decode permit");
            let registered = state
                .register("estimate-before-decode", Duration::from_secs(10))
                .expect("operation registration");
            let context = registered.context().clone();
            let waiter_state = state.clone();
            let waiter_context = context.clone();
            let waiter = tokio::spawn(async move {
                waiter_state
                    .acquire_estimate_then_decode(&waiter_context)
                    .await
            });

            for _ in 0..16 {
                if state.estimate.available_permits() == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(state.estimate.available_permits(), 0);
            assert_eq!(state.decode.available_permits(), 0);

            context.cancel();
            let wait_result = within_test_timeout(waiter)
                .await
                .expect("estimate/decode waiter task");
            assert!(matches!(wait_result, Err(PipelineError::Cancelled)));
            assert_eq!(state.estimate.available_permits(), 1);
        });
    }

    #[test]
    fn managed_blocking_worker_keeps_registration_and_permit_after_async_waiter_is_aborted() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let registered = state
                .register("managed-worker", Duration::from_secs(10))
                .expect("operation registration");
            let permit = state
                .acquire_decode(registered.context())
                .await
                .expect("decode permit");
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();

            let waiter = tokio::spawn(run_managed_blocking(registered, permit, move || {
                let _ = started_tx.send(());
                release_rx.recv().expect("worker release signal");
                let _ = finished_tx.send(());
            }));
            within_test_timeout(started_rx)
                .await
                .expect("worker start signal");

            waiter.abort();
            tokio::task::yield_now().await;
            assert_eq!(state.registry.lock_state().active.len(), 1);
            assert_eq!(state.decode.available_permits(), 1);

            release_tx.send(()).expect("release worker");
            within_test_timeout(finished_rx)
                .await
                .expect("worker finish signal");
            within_test_timeout(async {
                loop {
                    if state.registry.lock_state().active.is_empty() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            assert!(state.registry.lock_state().active.is_empty());
            assert_eq!(state.decode.available_permits(), 2);
        });
    }

    #[test]
    fn recording_sink_preserves_id_closed_codes_order_and_counts() {
        let context = OperationContext::new("progress-id".into(), future_deadline());
        let sink = RecordingProgressSink::default();
        let stages = [
            ProgressStage::Queued,
            ProgressStage::Inspecting,
            ProgressStage::Decoding,
            ProgressStage::Estimating,
            ProgressStage::Encoding,
            ProgressStage::Finalizing,
        ];

        for (index, stage) in stages.into_iter().enumerate() {
            publish_progress(
                &sink,
                &context,
                stage,
                if stage == ProgressStage::Encoding {
                    index as u32
                } else {
                    0
                },
                (stage == ProgressStage::Encoding).then_some(stages.len() as u32),
            );
        }

        let values = sink.values.lock().expect("recording sink lock");
        assert_eq!(values.len(), stages.len());
        assert_eq!(
            values.iter().map(|value| value.stage).collect::<Vec<_>>(),
            stages
        );
        assert!(values
            .iter()
            .all(|value| value.operation_id == "progress-id"));
        assert_eq!(
            values
                .iter()
                .map(|value| value.message_code)
                .collect::<Vec<_>>(),
            vec![
                "media-operation-queued",
                "media-operation-inspecting",
                "media-operation-decoding",
                "media-operation-estimating",
                "media-operation-encoding",
                "media-operation-finalizing",
            ]
        );
        assert_eq!(values[4].completed, 4);
        assert_eq!(values[4].total, Some(6));
        assert_eq!(
            serde_json::to_value(&values[4]).expect("progress JSON"),
            serde_json::json!({
                "operationId": "progress-id",
                "stage": "encoding",
                "completed": 4,
                "total": 6,
                "messageCode": "media-operation-encoding",
            })
        );
        assert_eq!(
            serde_json::to_value(&values[0]).expect("queued progress JSON"),
            serde_json::json!({
                "operationId": "progress-id",
                "stage": "queued",
                "completed": 0,
                "total": null,
                "messageCode": "media-operation-queued",
            })
        );
    }
}
