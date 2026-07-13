use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::media_error::PipelineError;
use crate::preview_cache::{PreviewCache, MAX_PREVIEW_CACHE_BYTES};

const MAX_OPERATION_ID_BYTES: usize = 128;
const TOMBSTONE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PRE_CANCELLED: usize = 1024;
const RECENT_FINISHED_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_RECENT_FINISHED: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PermitProfile {
    None,
    Decode,
    EstimateDecode,
    OutputDecode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaOperationKind {
    Inspect,
    BuildPlan,
    Preview,
    StaticEstimate,
    StaticConversion,
    OptimizerSearch,
    OptimizerEstimate,
    OptimizerProbe,
}

impl MediaOperationKind {
    pub(crate) fn timeout(self) -> Duration {
        match self {
            Self::Inspect | Self::BuildPlan => Duration::from_secs(15),
            Self::Preview => Duration::from_secs(30),
            Self::StaticEstimate | Self::StaticConversion => Duration::from_secs(60),
            Self::OptimizerEstimate => Duration::from_secs(90),
            Self::OptimizerProbe => Duration::from_secs(120),
            Self::OptimizerSearch => Duration::from_secs(180),
        }
    }

    pub(crate) fn permit_profile(self) -> PermitProfile {
        match self {
            Self::BuildPlan => PermitProfile::None,
            Self::Inspect | Self::Preview => PermitProfile::Decode,
            Self::StaticEstimate | Self::OptimizerEstimate | Self::OptimizerProbe => {
                PermitProfile::EstimateDecode
            }
            Self::StaticConversion | Self::OptimizerSearch => PermitProfile::OutputDecode,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationPhase {
    Reserved,
    Running,
    Publishing,
    Completed,
}

struct OperationLifecycle {
    phase: OperationPhase,
    cancelled: bool,
    deadline: Option<Instant>,
}

#[derive(Clone)]
pub(crate) struct OperationContext {
    operation_id: Arc<str>,
    lifecycle: Arc<Mutex<OperationLifecycle>>,
    cancellation: CancellationToken,
}

impl OperationContext {
    fn new(operation_id: String, deadline: Instant) -> Self {
        Self::with_lifecycle(operation_id, OperationPhase::Running, Some(deadline))
    }

    fn reserved(operation_id: String) -> Self {
        Self::with_lifecycle(operation_id, OperationPhase::Reserved, None)
    }

    fn with_lifecycle(
        operation_id: String,
        phase: OperationPhase,
        deadline: Option<Instant>,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            lifecycle: Arc::new(Mutex::new(OperationLifecycle {
                phase,
                cancelled: false,
                deadline,
            })),
            cancellation: CancellationToken::new(),
        }
    }

    fn lock_lifecycle(&self) -> MutexGuard<'_, OperationLifecycle> {
        self.lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn activate(&self, timeout: Duration) -> Result<(), PipelineError> {
        let deadline = bounded_deadline(Instant::now(), timeout);
        let mut lifecycle = self.lock_lifecycle();
        if lifecycle.cancelled || self.cancellation.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        if lifecycle.phase != OperationPhase::Reserved {
            return Err(PipelineError::Io {
                operation: "activate operation",
                message: "operation is not reserved".into(),
            });
        }
        lifecycle.deadline = Some(deadline);
        lifecycle.phase = OperationPhase::Running;
        Ok(())
    }

    pub(crate) fn detached(timeout: Duration) -> Self {
        Self::new(
            "detached-process".into(),
            bounded_deadline(Instant::now(), timeout),
        )
    }

    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.lock_lifecycle().deadline.unwrap_or_else(Instant::now)
    }

    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        let lifecycle = self.lock_lifecycle();
        lifecycle.cancelled || self.cancellation.is_cancelled()
    }

    pub(crate) fn cancel(&self) -> bool {
        let mut lifecycle = self.lock_lifecycle();
        if matches!(
            lifecycle.phase,
            OperationPhase::Publishing | OperationPhase::Completed
        ) {
            return false;
        }
        lifecycle.cancelled = true;
        self.cancellation.cancel();
        true
    }

    pub(crate) fn checkpoint(&self) -> Result<(), PipelineError> {
        let lifecycle = self.lock_lifecycle();
        if lifecycle.cancelled || self.cancellation.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        if lifecycle.phase == OperationPhase::Running
            && lifecycle
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(PipelineError::TimedOut { stage: "operation" });
        }
        Ok(())
    }

    pub(crate) fn finalize<T>(
        &self,
        finalizer: impl FnOnce() -> Result<T, PipelineError>,
    ) -> Result<T, PipelineError> {
        {
            let mut lifecycle = self.lock_lifecycle();
            if lifecycle.cancelled || self.cancellation.is_cancelled() {
                return Err(PipelineError::Cancelled);
            }
            if lifecycle.phase != OperationPhase::Running {
                return Err(PipelineError::Io {
                    operation: "finalize operation",
                    message: "operation is not running".into(),
                });
            }
            if lifecycle
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                return Err(PipelineError::TimedOut { stage: "operation" });
            }
            lifecycle.phase = OperationPhase::Publishing;
        }

        let _completion = CompletionGuard {
            lifecycle: Arc::clone(&self.lifecycle),
        };
        finalizer()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_publishing(&self) -> bool {
        self.lock_lifecycle().phase == OperationPhase::Publishing
    }

    pub(crate) fn is_completed(&self) -> bool {
        self.lock_lifecycle().phase == OperationPhase::Completed
    }
}

struct CompletionGuard {
    lifecycle: Arc<Mutex<OperationLifecycle>>,
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lifecycle.phase = OperationPhase::Completed;
    }
}

fn bounded_deadline(now: Instant, timeout: Duration) -> Instant {
    let mut bounded_timeout = timeout;
    loop {
        if let Some(deadline) = now.checked_add(bounded_timeout) {
            return deadline;
        }
        bounded_timeout = bounded_timeout.checked_div(2).unwrap_or(Duration::ZERO);
        if bounded_timeout.is_zero() {
            return now.checked_add(Duration::from_nanos(1)).unwrap_or(now);
        }
    }
}

struct ActiveOperation {
    generation: u64,
    context: OperationContext,
}

struct RegistryState {
    active: HashMap<String, ActiveOperation>,
    pre_cancelled: HashMap<String, Instant>,
    recent_finished: HashMap<String, Instant>,
    next_generation: u64,
}

impl Default for RegistryState {
    fn default() -> Self {
        Self {
            active: HashMap::new(),
            pre_cancelled: HashMap::new(),
            recent_finished: HashMap::new(),
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

    fn prune_recent_finished(&mut self, now: Instant) {
        self.recent_finished.retain(|_, finished_at| {
            match now.checked_duration_since(*finished_at) {
                Some(age) => age < RECENT_FINISHED_TTL,
                None => true,
            }
        });
    }

    fn enforce_recent_finished_cap(&mut self) {
        while self.recent_finished.len() > MAX_RECENT_FINISHED {
            let oldest = self
                .recent_finished
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
            self.recent_finished.remove(&oldest);
        }
    }

    fn record_finished_at(&mut self, operation_id: &str, finished_at: Instant) {
        self.prune_recent_finished(finished_at);
        self.pre_cancelled.remove(operation_id);
        self.recent_finished
            .insert(operation_id.into(), finished_at);
        self.enforce_recent_finished_cap();
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

    #[cfg_attr(not(test), allow(dead_code))]
    fn register(
        &self,
        operation_id: &str,
        timeout: Duration,
    ) -> Result<RegisteredOperation, PipelineError> {
        let now = Instant::now();
        self.register_at(operation_id, now, bounded_deadline(now, timeout))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn register_at(
        &self,
        operation_id: &str,
        now: Instant,
        deadline: Instant,
    ) -> Result<RegisteredOperation, PipelineError> {
        self.claim_at(operation_id, now, Some(deadline))
    }

    fn reserve(&self, operation_id: &str) -> Result<RegisteredOperation, PipelineError> {
        self.reserve_at(operation_id, Instant::now())
    }

    fn reserve_at(
        &self,
        operation_id: &str,
        now: Instant,
    ) -> Result<RegisteredOperation, PipelineError> {
        self.claim_at(operation_id, now, None)
    }

    fn claim_at(
        &self,
        operation_id: &str,
        now: Instant,
        deadline: Option<Instant>,
    ) -> Result<RegisteredOperation, PipelineError> {
        validate_operation_id(operation_id)?;

        let mut state = self.lock_state();
        state.prune_tombstones(now);
        state.prune_recent_finished(now);
        if state.pre_cancelled.remove(operation_id).is_some() {
            return Err(PipelineError::Cancelled);
        }
        if state.active.contains_key(operation_id)
            || state.recent_finished.contains_key(operation_id)
        {
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
        let context = match deadline {
            Some(deadline) => OperationContext::new(operation_id.into(), deadline),
            None => OperationContext::reserved(operation_id.into()),
        };
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

    #[cfg_attr(not(test), allow(dead_code))]
    fn record_finished_at(&self, operation_id: &str, finished_at: Instant) {
        self.lock_state()
            .record_finished_at(operation_id, finished_at);
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
        state.prune_recent_finished(now);
        if let Some(active) = state.active.get(operation_id) {
            return active.context.cancel();
        }
        if state.recent_finished.contains_key(operation_id) {
            return false;
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
            state.record_finished_at(&self.operation_id, Instant::now());
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
    preview_cache: Arc<PreviewCache>,
}

impl PipelineState {
    pub(crate) fn new() -> Self {
        Self {
            registry: OperationRegistry::new(),
            output: Arc::new(Semaphore::new(1)),
            estimate: Arc::new(Semaphore::new(1)),
            decode: Arc::new(Semaphore::new(2)),
            preview_cache: Arc::new(PreviewCache::new(MAX_PREVIEW_CACHE_BYTES)),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn register(
        &self,
        operation_id: &str,
        timeout: Duration,
    ) -> Result<RegisteredOperation, PipelineError> {
        self.registry.register(operation_id, timeout)
    }

    pub(crate) fn reserve(
        &self,
        operation_id: &str,
    ) -> Result<PreflightReservation, PipelineError> {
        self.reservation_from(self.registry.reserve(operation_id)?)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn reserve_at(
        &self,
        operation_id: &str,
        now: Instant,
    ) -> Result<PreflightReservation, PipelineError> {
        self.reservation_from(self.registry.reserve_at(operation_id, now)?)
    }

    fn reservation_from(
        &self,
        registration: RegisteredOperation,
    ) -> Result<PreflightReservation, PipelineError> {
        Ok(PreflightReservation {
            registration,
            output: Arc::clone(&self.output),
            estimate: Arc::clone(&self.estimate),
            decode: Arc::clone(&self.decode),
        })
    }

    pub(crate) fn cancel(&self, operation_id: &str) -> bool {
        self.registry.cancel(operation_id)
    }

    pub(crate) fn preview_cache(&self) -> Arc<PreviewCache> {
        Arc::clone(&self.preview_cache)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn acquire_decode(
        &self,
        context: &OperationContext,
    ) -> Result<OwnedSemaphorePermit, PipelineError> {
        acquire_permit(Arc::clone(&self.decode), context).await
    }

    #[cfg_attr(not(test), allow(dead_code))]
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

    #[cfg_attr(not(test), allow(dead_code))]
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

pub(crate) struct PreflightReservation {
    registration: RegisteredOperation,
    output: Arc<Semaphore>,
    estimate: Arc<Semaphore>,
    decode: Arc<Semaphore>,
}

impl PreflightReservation {
    pub(crate) fn context(&self) -> &OperationContext {
        self.registration.context()
    }

    pub(crate) async fn promote<S: ProgressSink>(
        self,
        kind: MediaOperationKind,
        progress: &ValidatedProgressSink<S>,
    ) -> Result<ManagedOperation, PipelineError> {
        if progress.kind != kind {
            return Err(PipelineError::Io {
                operation: "activate operation",
                message: "progress sink operation kind mismatch".into(),
            });
        }

        self.registration.context().activate(kind.timeout())?;
        if !progress.try_send(OperationProgress {
            operation_id: self.registration.context().operation_id().into(),
            stage: ProgressStage::Queued,
            completed: 0,
            total: None,
            message_code: ProgressStage::Queued.message_code(),
        }) {
            return Err(PipelineError::Io {
                operation: "activate operation",
                message: "queued progress was rejected".into(),
            });
        }

        let permits = match kind.permit_profile() {
            PermitProfile::None => ManagedPermits::None,
            PermitProfile::Decode => ManagedPermits::Decode {
                _decode: acquire_permit(Arc::clone(&self.decode), self.registration.context())
                    .await?,
            },
            PermitProfile::EstimateDecode => {
                let estimate =
                    acquire_permit(Arc::clone(&self.estimate), self.registration.context()).await?;
                let decode =
                    acquire_permit(Arc::clone(&self.decode), self.registration.context()).await?;
                ManagedPermits::EstimateDecode {
                    _permits: EstimateDecodePermits {
                        _estimate: estimate,
                        _decode: decode,
                    },
                }
            }
            PermitProfile::OutputDecode => {
                let output =
                    acquire_permit(Arc::clone(&self.output), self.registration.context()).await?;
                let decode =
                    acquire_permit(Arc::clone(&self.decode), self.registration.context()).await?;
                ManagedPermits::OutputDecode {
                    _permits: OutputDecodePermits {
                        _output: output,
                        _decode: decode,
                    },
                }
            }
        };

        Ok(ManagedOperation {
            registration: self.registration,
            _permits: permits,
        })
    }
}

enum ManagedPermits {
    None,
    Decode { _decode: OwnedSemaphorePermit },
    EstimateDecode { _permits: EstimateDecodePermits },
    OutputDecode { _permits: OutputDecodePermits },
}

pub(crate) struct ManagedOperation {
    registration: RegisteredOperation,
    _permits: ManagedPermits,
}

impl ManagedOperation {
    pub(crate) fn context(&self) -> &OperationContext {
        self.registration.context()
    }
}

pub(crate) async fn run_managed_blocking<T, F>(
    managed: ManagedOperation,
    job: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _managed = managed;
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

#[derive(Default)]
struct ProgressCursor {
    stage: Option<ProgressStage>,
    completed: u32,
    total: Option<u32>,
    sealed: bool,
}

impl ProgressCursor {
    fn accept(&mut self, kind: MediaOperationKind, progress: &OperationProgress) -> bool {
        if self.sealed
            || progress
                .total
                .is_some_and(|total| progress.completed > total)
        {
            return false;
        }

        match self.stage {
            None => {
                if progress.stage != ProgressStage::Queued
                    || progress.completed != 0
                    || progress.total.is_some()
                {
                    return false;
                }
            }
            Some(current) if current == progress.stage => {
                if matches!(current, ProgressStage::Queued | ProgressStage::Finalizing)
                    || progress.completed < self.completed
                {
                    return false;
                }
                match (self.total, progress.total) {
                    (Some(expected), Some(actual)) if expected == actual => {}
                    (Some(_), _) => return false,
                    (None, _) => {}
                }
            }
            Some(current) => {
                if !progress_transition_allowed(kind, current, progress.stage) {
                    return false;
                }
            }
        }

        self.stage = Some(progress.stage);
        self.completed = progress.completed;
        self.total = progress.total;
        self.sealed = progress.stage == ProgressStage::Finalizing;
        true
    }
}

fn progress_transition_allowed(
    kind: MediaOperationKind,
    current: ProgressStage,
    next: ProgressStage,
) -> bool {
    match kind {
        MediaOperationKind::Inspect => matches!(
            (current, next),
            (ProgressStage::Queued, ProgressStage::Inspecting)
                | (ProgressStage::Inspecting, ProgressStage::Decoding)
                | (ProgressStage::Inspecting, ProgressStage::Finalizing)
                | (ProgressStage::Decoding, ProgressStage::Finalizing)
        ),
        MediaOperationKind::BuildPlan => matches!(
            (current, next),
            (ProgressStage::Queued, ProgressStage::Estimating)
                | (ProgressStage::Estimating, ProgressStage::Finalizing)
        ),
        MediaOperationKind::StaticEstimate | MediaOperationKind::StaticConversion => matches!(
            (current, next),
            (ProgressStage::Queued, ProgressStage::Inspecting)
                | (ProgressStage::Inspecting, ProgressStage::Decoding)
                | (ProgressStage::Decoding, ProgressStage::Encoding)
                | (ProgressStage::Encoding, ProgressStage::Finalizing)
        ),
        MediaOperationKind::OptimizerSearch => matches!(
            (current, next),
            (ProgressStage::Queued, ProgressStage::Estimating)
                | (ProgressStage::Estimating, ProgressStage::Decoding)
                | (ProgressStage::Decoding, ProgressStage::Encoding)
                | (ProgressStage::Encoding, ProgressStage::Finalizing)
        ),
        MediaOperationKind::OptimizerEstimate => matches!(
            (current, next),
            (ProgressStage::Queued, ProgressStage::Decoding)
                | (ProgressStage::Decoding, ProgressStage::Estimating)
                | (ProgressStage::Estimating, ProgressStage::Finalizing)
        ),
        MediaOperationKind::OptimizerProbe => matches!(
            (current, next),
            (ProgressStage::Queued, ProgressStage::Decoding)
                | (ProgressStage::Decoding, ProgressStage::Encoding)
                | (ProgressStage::Encoding, ProgressStage::Finalizing)
        ),
        MediaOperationKind::Preview => matches!(
            (current, next),
            (ProgressStage::Queued, ProgressStage::Decoding)
                | (ProgressStage::Decoding, ProgressStage::Encoding)
                | (ProgressStage::Decoding, ProgressStage::Finalizing)
                | (ProgressStage::Encoding, ProgressStage::Finalizing)
        ),
    }
}

pub(crate) struct ValidatedProgressSink<S: ProgressSink> {
    kind: MediaOperationKind,
    sink: S,
    state: Mutex<ValidatedProgressState>,
}

#[derive(Default)]
struct ValidatedProgressState {
    cursor: ProgressCursor,
    pending: VecDeque<OperationProgress>,
    draining: bool,
}

struct ProgressDrainGuard<'a> {
    state: &'a Mutex<ValidatedProgressState>,
    armed: bool,
}

impl Drop for ProgressDrainGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.pending.clear();
            state.draining = false;
        }
    }
}

impl<S: ProgressSink> ValidatedProgressSink<S> {
    pub(crate) fn new(kind: MediaOperationKind, sink: S) -> Self {
        Self {
            kind,
            sink,
            state: Mutex::new(ValidatedProgressState::default()),
        }
    }

    pub(crate) fn try_send(&self, progress: OperationProgress) -> bool {
        let should_drain = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.cursor.accept(self.kind, &progress) {
                return false;
            }
            state.pending.push_back(progress);
            if state.draining {
                false
            } else {
                state.draining = true;
                true
            }
        };
        if !should_drain {
            return true;
        }

        let mut drain_guard = ProgressDrainGuard {
            state: &self.state,
            armed: true,
        };
        loop {
            let next = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(next) = state.pending.pop_front() else {
                    state.draining = false;
                    drain_guard.armed = false;
                    return true;
                };
                next
            };
            self.sink.send(next);
        }
    }
}

impl<S: ProgressSink> ProgressSink for ValidatedProgressSink<S> {
    fn send(&self, progress: OperationProgress) {
        let _ = self.try_send(progress);
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
    use std::sync::atomic::{AtomicBool, Ordering};
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

    #[derive(Clone, Default)]
    struct RecordingProgressSink {
        values: Arc<Mutex<Vec<OperationProgress>>>,
    }

    impl ProgressSink for RecordingProgressSink {
        fn send(&self, progress: OperationProgress) {
            self.values
                .lock()
                .expect("recording sink lock")
                .push(progress);
        }
    }

    struct BlockingEncodingSink {
        stages: Arc<Mutex<Vec<ProgressStage>>>,
        encoding_started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        release_encoding: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl ProgressSink for BlockingEncodingSink {
        fn send(&self, progress: OperationProgress) {
            if progress.stage == ProgressStage::Encoding {
                if let Some(started) = self
                    .encoding_started
                    .lock()
                    .expect("encoding-start lock")
                    .take()
                {
                    started.send(()).expect("encoding-start signal");
                    self.release_encoding
                        .lock()
                        .expect("encoding-release lock")
                        .recv()
                        .expect("encoding-release signal");
                }
            }
            self.stages
                .lock()
                .expect("blocking sink stages lock")
                .push(progress.stage);
        }
    }

    struct PanicOnceProgressSink {
        panic_next: AtomicBool,
        stages: Arc<Mutex<Vec<ProgressStage>>>,
    }

    impl ProgressSink for PanicOnceProgressSink {
        fn send(&self, progress: OperationProgress) {
            if self.panic_next.swap(false, Ordering::SeqCst) {
                panic!("injected progress sink panic");
            }
            self.stages
                .lock()
                .expect("panic sink stages lock")
                .push(progress.stage);
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
    fn detached_context_is_registry_free_live_and_uses_fixed_internal_id() {
        let context = OperationContext::detached(Duration::from_secs(1));

        assert_eq!(context.operation_id(), "detached-process");
        assert!(context.deadline() > Instant::now());
        assert_eq!(context.checkpoint(), Ok(()));
    }

    #[test]
    fn detached_context_preserves_cancellation_before_deadline_precedence() {
        let context = OperationContext::detached(Duration::ZERO);
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
                MediaOperationKind::StaticEstimate,
                Duration::from_secs(60),
                PermitProfile::EstimateDecode,
            ),
            (
                MediaOperationKind::OptimizerSearch,
                Duration::from_secs(180),
                PermitProfile::OutputDecode,
            ),
            (
                MediaOperationKind::OptimizerEstimate,
                Duration::from_secs(90),
                PermitProfile::EstimateDecode,
            ),
            (
                MediaOperationKind::OptimizerProbe,
                Duration::from_secs(120),
                PermitProfile::EstimateDecode,
            ),
        ];

        for (kind, deadline, permits) in cases {
            assert_eq!(kind.timeout(), deadline);
            assert_eq!(kind.permit_profile(), permits);
        }
    }

    #[test]
    fn static_estimate_promotion_owns_estimate_then_decode_without_output_permit() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let progress = ValidatedProgressSink::new(
                MediaOperationKind::StaticEstimate,
                RecordingProgressSink::default(),
            );
            let managed = state
                .reserve("static-estimate-permits")
                .expect("estimate reservation")
                .promote(MediaOperationKind::StaticEstimate, &progress)
                .await
                .expect("estimate promotion");

            assert_eq!(state.estimate.available_permits(), 0);
            assert_eq!(state.decode.available_permits(), 1);
            assert_eq!(state.output.available_permits(), 1);

            drop(managed);
            assert_eq!(state.estimate.available_permits(), 1);
            assert_eq!(state.decode.available_permits(), 2);
            assert_eq!(state.output.available_permits(), 1);
        });
    }

    #[test]
    fn optimizer_estimate_and_probe_own_estimate_then_decode_without_output_permit() {
        test_runtime().block_on(async {
            for (kind, operation_id) in [
                (
                    MediaOperationKind::OptimizerEstimate,
                    "optimizer-estimate-permits",
                ),
                (
                    MediaOperationKind::OptimizerProbe,
                    "optimizer-probe-permits",
                ),
            ] {
                let state = PipelineState::new();
                let progress = ValidatedProgressSink::new(kind, RecordingProgressSink::default());
                let managed = state
                    .reserve(operation_id)
                    .expect("candidate estimate reservation")
                    .promote(kind, &progress)
                    .await
                    .expect("candidate estimate promotion");
                assert_eq!(state.estimate.available_permits(), 0);
                assert_eq!(state.decode.available_permits(), 1);
                assert_eq!(state.output.available_permits(), 1);
                drop(managed);
                assert_eq!(state.estimate.available_permits(), 1);
                assert_eq!(state.decode.available_permits(), 2);
                assert_eq!(state.output.available_permits(), 1);
            }
        });
    }

    #[test]
    fn tauri_builder_and_command_scope_are_statically_wired() {
        let source = include_str!("lib.rs");
        assert!(source.contains(".manage(PipelineState::new())"));
        assert!(source.contains("cancel_media_operation,"));

        let managed_commands = [
            ("inspect_input_media", "MediaOperationKind::Inspect"),
            ("build_optimizer_plan", "MediaOperationKind::BuildPlan"),
            (
                "convert_static_image_to_png",
                "MediaOperationKind::StaticConversion",
            ),
            (
                "estimate_static_output_size",
                "MediaOperationKind::StaticEstimate",
            ),
            (
                "run_optimizer_search",
                "MediaOperationKind::OptimizerSearch",
            ),
            (
                "estimate_optimizer_candidates",
                "MediaOperationKind::OptimizerEstimate",
            ),
            (
                "probe_optimizer_candidate_size",
                "MediaOperationKind::OptimizerProbe",
            ),
            ("extract_frame_preview", "MediaOperationKind::Preview"),
            ("extract_frame_previews", "MediaOperationKind::Preview"),
        ];

        for (command, operation_kind) in managed_commands {
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
            assert_eq!(
                block.matches(".reserve(&operation_id)").count(),
                1,
                "{command}"
            );
            assert_eq!(block.matches(".promote(").count(), 1, "{command}");
            assert_eq!(
                block.matches("run_managed_blocking(managed").count(),
                1,
                "{command}"
            );
            assert_eq!(block.matches(".register(").count(), 0, "{command}");
            assert_eq!(block.matches(".acquire_").count(), 0, "{command}");
        }

        for command in ["check_media_tools", "open_folder_path"] {
            let block = command_source_block(source, command);
            assert!(!block.contains("operation_id: String"), "{command}");
            assert!(!block.contains("on_progress"), "{command}");
            assert!(!block.contains("PipelineState"), "{command}");
            assert!(!block.contains("MediaOperationKind::"), "{command}");
            assert!(!block.contains(".reserve("), "{command}");
            assert!(!block.contains(".promote("), "{command}");
            assert!(!block.contains(".register("), "{command}");
            assert!(!block.contains(".acquire_"), "{command}");
            assert!(!block.contains("run_managed_blocking"), "{command}");
        }

        let cancel_block = command_source_block(source, "cancel_media_operation");
        assert!(cancel_block.contains("pipeline_state.cancel(&operation_id)"));
        assert!(!cancel_block.contains("on_progress"));
        assert!(!cancel_block.contains("MediaOperationKind::"));
        assert!(!cancel_block.contains(".reserve("));
        assert!(!cancel_block.contains(".promote("));
        assert!(!cancel_block.contains(".register("));
        assert!(!cancel_block.contains(".acquire_"));
        assert!(!cancel_block.contains("run_managed_blocking"));
    }

    fn command_source_block<'a>(source: &'a str, command: &str) -> &'a str {
        let marker = format!("fn {command}(");
        let start = source.find(&marker).expect("command source marker");
        let remainder = &source[start..];
        let body_start = remainder.find('{').expect("command body marker");
        let mut depth = 0usize;

        for (offset, character) in remainder[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth = depth.checked_sub(1).expect("balanced command body");
                    if depth == 0 {
                        let end = body_start + offset + character.len_utf8();
                        return &remainder[..end];
                    }
                }
                _ => {}
            }
        }

        panic!("unterminated command body")
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
            let progress = ValidatedProgressSink::new(
                MediaOperationKind::Inspect,
                RecordingProgressSink::default(),
            );
            let managed = state
                .reserve("managed-worker")
                .expect("reservation must be created")
                .promote(MediaOperationKind::Inspect, &progress)
                .await
                .expect("promotion must succeed");
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();

            let waiter = tokio::spawn(run_managed_blocking(managed, move || {
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
                    let registry_is_empty = state.registry.lock_state().active.is_empty();
                    let permit_is_fully_returned = state.decode.available_permits() == 2;
                    if registry_is_empty && permit_is_fully_returned {
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

    fn progress_event(
        context: &OperationContext,
        stage: ProgressStage,
        completed: u32,
        total: Option<u32>,
    ) -> OperationProgress {
        OperationProgress {
            operation_id: context.operation_id().into(),
            stage,
            completed,
            total,
            message_code: stage.message_code(),
        }
    }

    #[test]
    fn preflight_reservation_rejects_duplicate_and_consumes_pre_cancel() {
        let state = PipelineState::new();
        let _reservation = state
            .reserve("reserved-duplicate")
            .expect("first reservation must own the operation ID");
        assert!(matches!(
            state.reserve("reserved-duplicate"),
            Err(PipelineError::OperationConflict { operation_id })
                if operation_id == "reserved-duplicate"
        ));

        assert!(state.cancel("reserved-pre-cancel"));
        assert!(matches!(
            state.reserve("reserved-pre-cancel"),
            Err(PipelineError::Cancelled)
        ));
    }

    #[test]
    fn reserved_cancellation_survives_tombstone_eviction_and_blocks_promotion() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let reservation = state
                .reserve("reserved-through-eviction")
                .expect("reservation must be created");
            assert!(state.cancel("reserved-through-eviction"));

            for index in 0..=MAX_PRE_CANCELLED {
                assert!(state.cancel(&format!("unrelated-tombstone-{index}")));
            }

            let recording = RecordingProgressSink::default();
            let progress =
                ValidatedProgressSink::new(MediaOperationKind::OptimizerSearch, recording.clone());
            let promoted = reservation
                .promote(MediaOperationKind::OptimizerSearch, &progress)
                .await;

            assert!(matches!(promoted, Err(PipelineError::Cancelled)));
            assert!(recording
                .values
                .lock()
                .expect("recording sink lock")
                .is_empty());
            assert_eq!(state.output.available_permits(), 1);
            assert_eq!(state.decode.available_permits(), 2);
        });
    }

    #[test]
    fn output_then_decode_promotion_cancellation_releases_partial_permits_and_registry() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let held_decode_a = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("first decode permit must be held");
            let held_decode_b = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("second decode permit must be held");
            let reservation = state
                .reserve("promote-output-before-decode")
                .expect("reservation must be created");
            let recording = RecordingProgressSink::default();
            let waiter_recording = recording.clone();
            let waiter = tokio::spawn(async move {
                let progress = ValidatedProgressSink::new(
                    MediaOperationKind::StaticConversion,
                    waiter_recording,
                );
                reservation
                    .promote(MediaOperationKind::StaticConversion, &progress)
                    .await
            });

            within_test_timeout(async {
                loop {
                    if state.output.available_permits() == 0 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            assert_eq!(state.output.available_permits(), 0);
            assert_eq!(state.decode.available_permits(), 0);
            assert_eq!(state.registry.lock_state().active.len(), 1);
            {
                let queued = recording.values.lock().expect("recording sink lock");
                assert_eq!(queued.len(), 1);
                assert_eq!(queued[0].stage, ProgressStage::Queued);
            }

            assert!(state.cancel("promote-output-before-decode"));
            let result = within_test_timeout(waiter)
                .await
                .expect("promotion task must not panic");
            assert!(matches!(result, Err(PipelineError::Cancelled)));
            assert!(state.registry.lock_state().active.is_empty());
            assert_eq!(state.output.available_permits(), 1);
            assert_eq!(state.decode.available_permits(), 0);

            drop(held_decode_a);
            drop(held_decode_b);
            assert_eq!(state.decode.available_permits(), 2);
        });
    }

    #[test]
    fn estimate_then_decode_promotion_cancellation_releases_partial_permits_and_registry() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let held_decode_a = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("first decode permit must be held");
            let held_decode_b = state
                .decode
                .clone()
                .try_acquire_owned()
                .expect("second decode permit must be held");
            let reservation = state
                .reserve("promote-estimate-before-decode")
                .expect("reservation must be created");
            let recording = RecordingProgressSink::default();
            let waiter_recording = recording.clone();
            let waiter = tokio::spawn(async move {
                let progress = ValidatedProgressSink::new(
                    MediaOperationKind::StaticEstimate,
                    waiter_recording,
                );
                reservation
                    .promote(MediaOperationKind::StaticEstimate, &progress)
                    .await
            });

            within_test_timeout(async {
                loop {
                    if state.estimate.available_permits() == 0 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            assert_eq!(state.estimate.available_permits(), 0);
            assert_eq!(state.decode.available_permits(), 0);
            assert_eq!(state.output.available_permits(), 1);
            assert_eq!(state.registry.lock_state().active.len(), 1);
            {
                let queued = recording.values.lock().expect("recording sink lock");
                assert_eq!(queued.len(), 1);
                assert_eq!(queued[0].stage, ProgressStage::Queued);
            }

            assert!(state.cancel("promote-estimate-before-decode"));
            let result = within_test_timeout(waiter)
                .await
                .expect("promotion task must not panic");
            assert!(matches!(result, Err(PipelineError::Cancelled)));
            assert!(state.registry.lock_state().active.is_empty());
            assert_eq!(state.estimate.available_permits(), 1);
            assert_eq!(state.decode.available_permits(), 0);
            assert_eq!(state.output.available_permits(), 1);

            drop(held_decode_a);
            drop(held_decode_b);
            assert_eq!(state.decode.available_permits(), 2);
        });
    }

    #[test]
    fn reservation_promotion_starts_deadline_and_emits_queued_once() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let reservation = state
                .reserve("promotion-deadline")
                .expect("reservation must be created");
            let recording = RecordingProgressSink::default();
            let progress =
                ValidatedProgressSink::new(MediaOperationKind::BuildPlan, recording.clone());
            let before = Instant::now();
            let managed = reservation
                .promote(MediaOperationKind::BuildPlan, &progress)
                .await
                .expect("promotion must succeed");
            let after = Instant::now();
            let deadline = managed.context().deadline();
            let timeout = MediaOperationKind::BuildPlan.timeout();

            assert!(deadline >= before + timeout);
            assert!(deadline <= after + timeout);
            let values = recording.values.lock().expect("recording sink lock");
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].stage, ProgressStage::Queued);
            assert_eq!(values[0].completed, 0);
            assert_eq!(values[0].total, None);
        });
    }

    #[test]
    fn finalize_cancellation_wins_without_running_the_closure() {
        let context = OperationContext::new("finalize-cancelled".into(), future_deadline());
        let called = AtomicBool::new(false);
        assert!(context.cancel());

        let result = context.finalize(|| {
            called.store(true, Ordering::SeqCst);
            Ok::<_, PipelineError>(())
        });

        assert_eq!(result, Err(PipelineError::Cancelled));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn finalize_first_completes_and_rejects_later_cancellation() {
        let context = OperationContext::new("finalize-wins".into(), future_deadline());

        assert_eq!(context.finalize(|| Ok::<_, PipelineError>(42)), Ok(42));
        assert!(context.is_completed());
        assert!(!context.cancel());
    }

    #[test]
    fn finalize_rejects_an_elapsed_deadline_before_running_the_closure() {
        let context = OperationContext::new(
            "finalize-expired".into(),
            Instant::now() - Duration::from_millis(1),
        );
        let called = AtomicBool::new(false);

        let result = context.finalize(|| {
            called.store(true, Ordering::SeqCst);
            Ok::<_, PipelineError>(())
        });

        assert!(matches!(
            result,
            Err(PipelineError::TimedOut { stage: "operation" })
        ));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn finalize_panic_unwind_marks_the_phase_completed() {
        let context = OperationContext::new("finalize-panic".into(), future_deadline());

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<(), PipelineError> = context.finalize(|| panic!("publisher panic"));
        }));

        assert!(outcome.is_err());
        assert!(context.is_completed());
        assert!(!context.cancel());
    }

    #[test]
    fn completed_registration_rejects_late_cancel_and_recent_id_reuse() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let recording = RecordingProgressSink::default();
            let progress = ValidatedProgressSink::new(MediaOperationKind::BuildPlan, recording);
            let managed = state
                .reserve("recently-finished")
                .expect("reservation must be created")
                .promote(MediaOperationKind::BuildPlan, &progress)
                .await
                .expect("promotion must succeed");

            assert_eq!(
                managed.context().finalize(|| Ok::<_, PipelineError>(())),
                Ok(())
            );
            drop(managed);

            assert!(!state.cancel("recently-finished"));
            assert!(matches!(
                state.reserve("recently-finished"),
                Err(PipelineError::OperationConflict { operation_id })
                    if operation_id == "recently-finished"
            ));
        });
    }

    #[test]
    fn managed_blocking_worker_owns_publishing_registration_and_permits_until_completion() {
        test_runtime().block_on(async {
            let state = PipelineState::new();
            let recording = RecordingProgressSink::default();
            let progress =
                ValidatedProgressSink::new(MediaOperationKind::StaticConversion, recording);
            let managed = state
                .reserve("publishing-worker-ownership")
                .expect("reservation must be created")
                .promote(MediaOperationKind::StaticConversion, &progress)
                .await
                .expect("promotion must succeed");
            let context = managed.context().clone();
            let worker_context = context.clone();
            let (publishing_tx, publishing_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let waiter = tokio::spawn(run_managed_blocking(managed, move || {
                worker_context.finalize(|| {
                    let _ = publishing_tx.send(());
                    release_rx.recv().expect("publisher release signal");
                    Ok::<_, PipelineError>(())
                })
            }));

            within_test_timeout(publishing_rx)
                .await
                .expect("publisher must enter finalization");
            assert!(context.is_publishing());
            assert!(!state.cancel("publishing-worker-ownership"));
            assert_eq!(state.registry.lock_state().active.len(), 1);
            assert_eq!(state.output.available_permits(), 0);
            assert_eq!(state.decode.available_permits(), 1);

            release_tx.send(()).expect("release publisher");
            let worker_result = within_test_timeout(waiter)
                .await
                .expect("managed worker task must not panic")
                .expect("blocking worker must join");
            assert_eq!(worker_result, Ok(()));
            assert!(context.is_completed());
            assert!(state.registry.lock_state().active.is_empty());
            assert_eq!(state.output.available_permits(), 1);
            assert_eq!(state.decode.available_permits(), 2);
        });
    }

    #[test]
    fn recent_finished_ttl_allows_reuse_and_cap_evicts_oldest_deterministically() {
        let state = PipelineState::new();
        let started = Instant::now();
        state.registry.record_finished_at("finished-ttl", started);

        assert!(!state
            .registry
            .cancel_at("finished-ttl", started + RECENT_FINISHED_TTL / 2));
        assert!(matches!(
            state.reserve_at("finished-ttl", started + RECENT_FINISHED_TTL / 2),
            Err(PipelineError::OperationConflict { operation_id })
                if operation_id == "finished-ttl"
        ));
        drop(
            state
                .reserve_at(
                    "finished-ttl",
                    started + RECENT_FINISHED_TTL + Duration::from_nanos(1),
                )
                .expect("expired finished ID must be reusable"),
        );

        for index in 0..=MAX_RECENT_FINISHED {
            state.registry.record_finished_at(
                &format!("finished-cap-{index}"),
                started + Duration::from_millis(index as u64),
            );
        }
        let registry = state.registry.lock_state();
        assert_eq!(registry.recent_finished.len(), MAX_RECENT_FINISHED);
        assert!(!registry.recent_finished.contains_key("finished-cap-0"));
        assert!(registry
            .recent_finished
            .contains_key(&format!("finished-cap-{MAX_RECENT_FINISHED}")));
    }

    #[test]
    fn validated_progress_accepts_each_command_kind_canonical_graph() {
        let cases: &[(MediaOperationKind, &[ProgressStage])] = &[
            (
                MediaOperationKind::Inspect,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Inspecting,
                    ProgressStage::Decoding,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::OptimizerEstimate,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Decoding,
                    ProgressStage::Estimating,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::OptimizerProbe,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Decoding,
                    ProgressStage::Encoding,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::BuildPlan,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Estimating,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::StaticConversion,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Inspecting,
                    ProgressStage::Decoding,
                    ProgressStage::Encoding,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::StaticEstimate,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Inspecting,
                    ProgressStage::Decoding,
                    ProgressStage::Encoding,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::OptimizerSearch,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Estimating,
                    ProgressStage::Decoding,
                    ProgressStage::Encoding,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::Preview,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Decoding,
                    ProgressStage::Encoding,
                    ProgressStage::Finalizing,
                ],
            ),
        ];

        for (kind, stages) in cases {
            let context = OperationContext::new(format!("progress-{kind:?}"), future_deadline());
            let recording = RecordingProgressSink::default();
            let progress = ValidatedProgressSink::new(*kind, recording.clone());
            for stage in *stages {
                assert!(progress.try_send(progress_event(&context, *stage, 0, None)));
            }
            let recorded = recording
                .values
                .lock()
                .expect("recording sink lock")
                .iter()
                .map(|value| value.stage)
                .collect::<Vec<_>>();
            assert_eq!(recorded.as_slice(), *stages);
        }
    }

    #[test]
    fn validated_progress_accepts_optional_stages_and_rejects_kind_forbidden_stages() {
        let optional_cases: &[(MediaOperationKind, &[ProgressStage])] = &[
            (
                MediaOperationKind::Inspect,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Inspecting,
                    ProgressStage::Finalizing,
                ],
            ),
            (
                MediaOperationKind::Preview,
                &[
                    ProgressStage::Queued,
                    ProgressStage::Decoding,
                    ProgressStage::Finalizing,
                ],
            ),
        ];
        for (kind, stages) in optional_cases {
            let context = OperationContext::new(format!("optional-{kind:?}"), future_deadline());
            let progress = ValidatedProgressSink::new(*kind, RecordingProgressSink::default());
            for stage in *stages {
                assert!(progress.try_send(progress_event(&context, *stage, 0, None)));
            }
        }

        let forbidden_cases: &[(MediaOperationKind, &[ProgressStage], ProgressStage)] = &[
            (
                MediaOperationKind::Inspect,
                &[ProgressStage::Queued, ProgressStage::Inspecting],
                ProgressStage::Estimating,
            ),
            (
                MediaOperationKind::BuildPlan,
                &[ProgressStage::Queued],
                ProgressStage::Decoding,
            ),
            (
                MediaOperationKind::StaticConversion,
                &[ProgressStage::Queued, ProgressStage::Inspecting],
                ProgressStage::Estimating,
            ),
            (
                MediaOperationKind::StaticEstimate,
                &[ProgressStage::Queued, ProgressStage::Inspecting],
                ProgressStage::Estimating,
            ),
            (
                MediaOperationKind::OptimizerSearch,
                &[ProgressStage::Queued, ProgressStage::Estimating],
                ProgressStage::Inspecting,
            ),
            (
                MediaOperationKind::OptimizerEstimate,
                &[ProgressStage::Queued, ProgressStage::Decoding],
                ProgressStage::Encoding,
            ),
            (
                MediaOperationKind::OptimizerProbe,
                &[ProgressStage::Queued, ProgressStage::Decoding],
                ProgressStage::Estimating,
            ),
            (
                MediaOperationKind::Preview,
                &[ProgressStage::Queued, ProgressStage::Decoding],
                ProgressStage::Inspecting,
            ),
        ];
        for (kind, prefix, forbidden) in forbidden_cases {
            let context = OperationContext::new(format!("forbidden-{kind:?}"), future_deadline());
            let progress = ValidatedProgressSink::new(*kind, RecordingProgressSink::default());
            for stage in *prefix {
                assert!(progress.try_send(progress_event(&context, *stage, 0, None)));
            }
            assert!(!progress.try_send(progress_event(&context, *forbidden, 0, None)));
        }
    }

    #[test]
    fn validated_progress_enforces_counts_totals_and_final_seal() {
        let context = OperationContext::new("validated-progress".into(), future_deadline());
        let recording = RecordingProgressSink::default();
        let progress =
            ValidatedProgressSink::new(MediaOperationKind::OptimizerSearch, recording.clone());

        assert!(!progress.try_send(progress_event(&context, ProgressStage::Estimating, 0, None,)));
        assert!(progress.try_send(progress_event(&context, ProgressStage::Queued, 0, None,)));
        assert!(progress.try_send(progress_event(&context, ProgressStage::Estimating, 0, None,)));
        assert!(progress.try_send(progress_event(
            &context,
            ProgressStage::Decoding,
            0,
            Some(1),
        )));
        assert!(!progress.try_send(progress_event(&context, ProgressStage::Estimating, 1, None,)));
        assert!(!progress.try_send(progress_event(&context, ProgressStage::Decoding, 0, None,)));
        assert!(!progress.try_send(progress_event(
            &context,
            ProgressStage::Decoding,
            0,
            Some(2),
        )));
        assert!(progress.try_send(progress_event(
            &context,
            ProgressStage::Decoding,
            1,
            Some(1),
        )));
        assert!(!progress.try_send(progress_event(
            &context,
            ProgressStage::Decoding,
            0,
            Some(1),
        )));
        assert!(progress.try_send(progress_event(
            &context,
            ProgressStage::Encoding,
            0,
            Some(3),
        )));
        assert!(progress.try_send(progress_event(
            &context,
            ProgressStage::Encoding,
            1,
            Some(3),
        )));
        assert!(progress.try_send(progress_event(
            &context,
            ProgressStage::Finalizing,
            1,
            Some(3),
        )));
        assert!(!progress.try_send(progress_event(
            &context,
            ProgressStage::Finalizing,
            2,
            Some(3),
        )));

        assert_eq!(
            recording
                .values
                .lock()
                .expect("recording sink lock")
                .iter()
                .map(|value| value.stage)
                .collect::<Vec<_>>(),
            vec![
                ProgressStage::Queued,
                ProgressStage::Estimating,
                ProgressStage::Decoding,
                ProgressStage::Decoding,
                ProgressStage::Encoding,
                ProgressStage::Encoding,
                ProgressStage::Finalizing,
            ]
        );
    }

    #[test]
    fn validated_progress_delivers_concurrent_events_without_holding_cursor_lock() {
        let context = OperationContext::new("concurrent-progress".into(), future_deadline());
        let stages = Arc::new(Mutex::new(Vec::new()));
        let (encoding_started_tx, encoding_started_rx) = std::sync::mpsc::channel();
        let (release_encoding_tx, release_encoding_rx) = std::sync::mpsc::channel();
        let progress = Arc::new(ValidatedProgressSink::new(
            MediaOperationKind::OptimizerSearch,
            BlockingEncodingSink {
                stages: Arc::clone(&stages),
                encoding_started: Mutex::new(Some(encoding_started_tx)),
                release_encoding: Mutex::new(release_encoding_rx),
            },
        ));

        for stage in [
            ProgressStage::Queued,
            ProgressStage::Estimating,
            ProgressStage::Decoding,
        ] {
            assert!(progress.try_send(progress_event(&context, stage, 0, None)));
        }

        let encoding_progress = Arc::clone(&progress);
        let encoding_context = context.clone();
        let encoding_thread = thread::spawn(move || {
            encoding_progress.try_send(progress_event(
                &encoding_context,
                ProgressStage::Encoding,
                0,
                Some(1),
            ))
        });
        encoding_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("encoding delivery must start");

        let finalizing_progress = Arc::clone(&progress);
        let finalizing_context = context.clone();
        let (finalizing_done_tx, finalizing_done_rx) = std::sync::mpsc::channel();
        let finalizing_thread = thread::spawn(move || {
            let accepted = finalizing_progress.try_send(progress_event(
                &finalizing_context,
                ProgressStage::Finalizing,
                1,
                Some(1),
            ));
            finalizing_done_tx
                .send(accepted)
                .expect("finalizing completion signal");
        });
        let finalizing_before_release = finalizing_done_rx.recv_timeout(Duration::from_millis(100));

        release_encoding_tx
            .send(())
            .expect("release encoding delivery");
        assert!(encoding_thread.join().expect("encoding progress thread"));
        finalizing_thread
            .join()
            .expect("finalizing progress thread");
        assert!(finalizing_before_release
            .expect("finalizing acceptance must not wait for the generic sink callback"));
        assert_eq!(
            *stages.lock().expect("blocking sink stages lock"),
            vec![
                ProgressStage::Queued,
                ProgressStage::Estimating,
                ProgressStage::Decoding,
                ProgressStage::Encoding,
                ProgressStage::Finalizing,
            ]
        );
    }

    #[test]
    fn validated_progress_sink_panic_clears_dispatch_state_without_poisoning_cursor() {
        let context = OperationContext::new("panic-progress".into(), future_deadline());
        let stages = Arc::new(Mutex::new(Vec::new()));
        let progress = ValidatedProgressSink::new(
            MediaOperationKind::BuildPlan,
            PanicOnceProgressSink {
                panic_next: AtomicBool::new(true),
                stages: Arc::clone(&stages),
            },
        );

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            progress.try_send(progress_event(&context, ProgressStage::Queued, 0, None))
        }));
        assert!(outcome.is_err());
        {
            let state = progress
                .state
                .lock()
                .expect("validated progress state lock");
            assert!(!state.draining);
            assert!(state.pending.is_empty());
        }

        assert!(progress.try_send(progress_event(&context, ProgressStage::Estimating, 0, None,)));
        assert_eq!(
            *stages.lock().expect("panic sink stages lock"),
            vec![ProgressStage::Estimating]
        );
    }

    #[test]
    fn preview_progress_keeps_native_total_unknown_and_video_total_fixed() {
        let native_context = OperationContext::new("preview-native".into(), future_deadline());
        let native_recording = RecordingProgressSink::default();
        let native =
            ValidatedProgressSink::new(MediaOperationKind::Preview, native_recording.clone());
        assert!(native.try_send(progress_event(
            &native_context,
            ProgressStage::Queued,
            0,
            None,
        )));
        assert!(native.try_send(progress_event(
            &native_context,
            ProgressStage::Decoding,
            0,
            None,
        )));
        assert!(native.try_send(progress_event(
            &native_context,
            ProgressStage::Decoding,
            12,
            None,
        )));

        let video_context = OperationContext::new("preview-video".into(), future_deadline());
        let video_recording = RecordingProgressSink::default();
        let video =
            ValidatedProgressSink::new(MediaOperationKind::Preview, video_recording.clone());
        assert!(video.try_send(progress_event(
            &video_context,
            ProgressStage::Queued,
            0,
            None,
        )));
        assert!(video.try_send(progress_event(
            &video_context,
            ProgressStage::Decoding,
            0,
            Some(2),
        )));
        assert!(video.try_send(progress_event(
            &video_context,
            ProgressStage::Decoding,
            2,
            Some(2),
        )));
        assert!(video.try_send(progress_event(
            &video_context,
            ProgressStage::Encoding,
            0,
            Some(2),
        )));
        assert!(video.try_send(progress_event(
            &video_context,
            ProgressStage::Encoding,
            2,
            Some(2),
        )));

        assert!(native_recording
            .values
            .lock()
            .expect("native recording sink lock")
            .iter()
            .filter(|value| value.stage == ProgressStage::Decoding)
            .all(|value| value.total.is_none()));
        assert!(video_recording
            .values
            .lock()
            .expect("video recording sink lock")
            .iter()
            .filter(|value| matches!(
                value.stage,
                ProgressStage::Decoding | ProgressStage::Encoding
            ))
            .all(|value| value.total == Some(2)));
    }
}
