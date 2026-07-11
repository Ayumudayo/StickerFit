use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::media_error::PipelineError;
use crate::operation::OperationContext;

const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(25);
const CALLER_CLEANUP_GRACE: Duration = Duration::from_millis(500);
const READ_CHUNK_BYTES: usize = 8 * 1024;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcessLimits {
    pub timeout: Duration,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CapturedProcess {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FrameStreamSummary {
    pub(crate) frame_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ObservedExit {
    success: bool,
    exit_code: Option<i32>,
}

impl ObservedExit {
    fn success(&self) -> bool {
        self.success
    }
}

impl From<ExitStatus> for ObservedExit {
    fn from(status: ExitStatus) -> Self {
        Self {
            success: status.success(),
            exit_code: status.code(),
        }
    }
}

trait ChildLifecycle: Send + 'static {
    fn try_wait_exit(&mut self) -> io::Result<Option<ObservedExit>>;
    fn kill_child(&mut self) -> io::Result<()>;
    fn wait_exit(&mut self) -> io::Result<ObservedExit>;
}

impl ChildLifecycle for Child {
    fn try_wait_exit(&mut self) -> io::Result<Option<ObservedExit>> {
        Child::try_wait(self).map(|status| status.map(ObservedExit::from))
    }

    fn kill_child(&mut self) -> io::Result<()> {
        Child::kill(self)
    }

    fn wait_exit(&mut self) -> io::Result<ObservedExit> {
        Child::wait(self).map(ObservedExit::from)
    }
}

struct BoundedReaderState {
    bytes: Vec<u8>,
    overflow_actual: Option<u64>,
    error: Option<String>,
    done: bool,
}

impl BoundedReaderState {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(READ_CHUNK_BYTES)),
            overflow_actual: None,
            error: None,
            done: false,
        }
    }
}

struct PrefixReaderState {
    bytes: Vec<u8>,
    error: Option<String>,
    done: bool,
}

impl PrefixReaderState {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(READ_CHUNK_BYTES)),
            error: None,
            done: false,
        }
    }
}

enum FrameEvent {
    Frame(Vec<u8>),
    Eof(Vec<u8>),
    ReadError(String),
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn frame_buffer_count(frame_size: usize, max_frames: usize, decoded_limit: usize) -> usize {
    let cap_limited_frames = decoded_limit.checked_div(frame_size).unwrap_or(0).max(1);
    3.min(max_frames).min(cap_limited_frames)
}

fn io_pipeline_error(operation: &'static str, error: impl ToString) -> PipelineError {
    PipelineError::Io {
        operation,
        message: error.to_string(),
    }
}

fn process_failed(program: &OsStr, status: ObservedExit, stderr: &[u8]) -> PipelineError {
    PipelineError::ProcessFailed {
        command: program.to_string_lossy().into_owned(),
        exit_code: status.exit_code,
        stderr: String::from_utf8_lossy(stderr).into_owned(),
    }
}

fn configure_managed_child(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

fn spawn_managed_child(program: &OsStr, args: &[OsString]) -> Result<Child, PipelineError> {
    let mut command = Command::new(program);
    configure_managed_child(&mut command);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| io_pipeline_error("spawn child process", error))
}

fn spawn_bounded_reader<R>(
    name: &str,
    mut reader: R,
    limit: usize,
    state: Arc<Mutex<BoundedReaderState>>,
) -> io::Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new().name(name.into()).spawn(move || {
        let mut chunk = [0_u8; READ_CHUNK_BYTES];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => {
                    lock_unpoisoned(&state).done = true;
                    return;
                }
                Ok(count) => {
                    let mut state = lock_unpoisoned(&state);
                    let remaining = limit.saturating_sub(state.bytes.len());
                    let retained = remaining.min(count);
                    if let Err(error) = state.bytes.try_reserve_exact(retained) {
                        state.error = Some(error.to_string());
                        state.done = true;
                        return;
                    }
                    state.bytes.extend_from_slice(&chunk[..retained]);
                    if retained < count {
                        state.overflow_actual = Some(as_u64(limit).saturating_add(1));
                        state.done = true;
                        return;
                    }
                }
                Err(error) => {
                    let mut state = lock_unpoisoned(&state);
                    state.error = Some(error.to_string());
                    state.done = true;
                    return;
                }
            }
        }
    })
}

fn spawn_prefix_reader<R>(
    name: &str,
    mut reader: R,
    limit: usize,
    state: Arc<Mutex<PrefixReaderState>>,
) -> io::Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new().name(name.into()).spawn(move || {
        let mut chunk = [0_u8; READ_CHUNK_BYTES];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => {
                    lock_unpoisoned(&state).done = true;
                    return;
                }
                Ok(count) => {
                    let mut state = lock_unpoisoned(&state);
                    let remaining = limit.saturating_sub(state.bytes.len());
                    let retained = remaining.min(count);
                    if let Err(error) = state.bytes.try_reserve_exact(retained) {
                        state.error = Some(error.to_string());
                        state.done = true;
                        return;
                    }
                    state.bytes.extend_from_slice(&chunk[..retained]);
                }
                Err(error) => {
                    let mut state = lock_unpoisoned(&state);
                    state.error = Some(error.to_string());
                    state.done = true;
                    return;
                }
            }
        }
    })
}

fn spawn_frame_reader<R>(
    mut reader: R,
    frame_size: usize,
    mut frame: Vec<u8>,
    frame_sender: SyncSender<FrameEvent>,
    recycle_receiver: Receiver<Vec<u8>>,
) -> io::Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("stickerfit-process-stdout-frames".into())
        .spawn(move || {
            let mut chunk = [0_u8; READ_CHUNK_BYTES];
            loop {
                frame.clear();
                while frame.len() < frame_size {
                    let wanted = (frame_size - frame.len()).min(chunk.len());
                    match reader.read(&mut chunk[..wanted]) {
                        Ok(0) => {
                            let _ = frame_sender.send(FrameEvent::Eof(frame));
                            return;
                        }
                        Ok(count) => frame.extend_from_slice(&chunk[..count]),
                        Err(error) => {
                            let _ = frame_sender.send(FrameEvent::ReadError(error.to_string()));
                            return;
                        }
                    }
                }

                if frame_sender.send(FrameEvent::Frame(frame)).is_err() {
                    return;
                }
                frame = match recycle_receiver.recv() {
                    Ok(frame) => frame,
                    Err(_) => return,
                };
            }
        })
}

fn poll_child_status<C: ChildLifecycle>(
    child: &mut C,
    status: &mut Option<ObservedExit>,
) -> Result<(), PipelineError> {
    if status.is_none() {
        *status = child
            .try_wait_exit()
            .map_err(|error| io_pipeline_error("poll child process", error))?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminationPoll {
    Reaped(ObservedExit),
    Pending,
}

fn poll_termination<C: ChildLifecycle>(
    child: &mut C,
    status: &mut Option<ObservedExit>,
    kill_requested: &mut bool,
    last_error: &mut Option<String>,
) -> TerminationPoll {
    match child.try_wait_exit() {
        Ok(Some(observed)) => {
            *status = Some(observed);
            return TerminationPoll::Reaped(observed);
        }
        Ok(None) => {}
        Err(error) => *last_error = Some(error.to_string()),
    }

    if !*kill_requested {
        match child.kill_child() {
            Ok(()) => *kill_requested = true,
            Err(error) => *last_error = Some(error.to_string()),
        }
    }
    TerminationPoll::Pending
}

fn join_reader(
    handle: JoinHandle<()>,
    operation: &'static str,
    primary: &mut Option<PipelineError>,
) {
    if handle.join().is_err() && primary.is_none() {
        *primary = Some(io_pipeline_error(operation, "reader thread panicked"));
    }
}

struct DeferredCleanup<C: ChildLifecycle> {
    child: Option<C>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
}

fn run_deferred_cleanup<C: ChildLifecycle>(mut cleanup: DeferredCleanup<C>) {
    if let Some(mut child) = cleanup.child.take() {
        loop {
            match child.try_wait_exit() {
                Ok(Some(_)) => break,
                Ok(None) | Err(_) => match child.kill_child() {
                    Ok(()) => match child.wait_exit() {
                        Ok(_) => break,
                        Err(_) => thread::sleep(CONTROL_POLL_INTERVAL),
                    },
                    Err(_) => thread::sleep(CONTROL_POLL_INTERVAL),
                },
            }
        }
    }

    if let Some(handle) = cleanup.stdout_reader.take() {
        let _ = handle.join();
    }
    if let Some(handle) = cleanup.stderr_reader.take() {
        let _ = handle.join();
    }
}

fn spawn_deferred_cleanup<C: ChildLifecycle>(cleanup: DeferredCleanup<C>) -> io::Result<()> {
    thread::Builder::new()
        .name("stickerfit-deferred-process-cleanup".into())
        .spawn(move || run_deferred_cleanup(cleanup))
        .map(|handle| drop(handle))
}

struct CleanupResult {
    status: Option<ObservedExit>,
    primary: Option<PipelineError>,
}

fn finish_reader_cleanup(
    primary: Option<PipelineError>,
    readers_deferred: bool,
) -> Option<PipelineError> {
    match (primary, readers_deferred) {
        (None, true) => Some(io_pipeline_error(
            "finish child process readers",
            "reader threads exceeded the caller cleanup grace period",
        )),
        (primary, _) => primary,
    }
}

fn cleanup_child_and_readers<C: ChildLifecycle>(
    mut child: C,
    mut status: Option<ObservedExit>,
    mut stdout_reader: Option<JoinHandle<()>>,
    mut stderr_reader: Option<JoinHandle<()>>,
    mut primary: Option<PipelineError>,
) -> CleanupResult {
    let started = Instant::now();
    let deadline = started.checked_add(CALLER_CLEANUP_GRACE).unwrap_or(started);
    let mut kill_requested = false;
    let mut last_error = None;

    loop {
        if status.is_none() {
            let _ = poll_termination(
                &mut child,
                &mut status,
                &mut kill_requested,
                &mut last_error,
            );
        }

        if stdout_reader
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            join_reader(
                stdout_reader.take().expect("finished stdout reader"),
                "join child stdout reader",
                &mut primary,
            );
        }
        if stderr_reader
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            join_reader(
                stderr_reader.take().expect("finished stderr reader"),
                "join child stderr reader",
                &mut primary,
            );
        }

        if status.is_some() && stdout_reader.is_none() && stderr_reader.is_none() {
            return CleanupResult { status, primary };
        }

        let now = Instant::now();
        if now >= deadline {
            break;
        }
        thread::sleep(CONTROL_POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
    }

    let readers_deferred = stdout_reader.is_some() || stderr_reader.is_some();
    let cleanup = DeferredCleanup {
        child: status.is_none().then_some(child),
        stdout_reader,
        stderr_reader,
    };
    if let Err(error) = spawn_deferred_cleanup(cleanup) {
        if primary.is_none() {
            let detail = last_error
                .map(|last_error| format!("{error}; last cleanup error: {last_error}"))
                .unwrap_or_else(|| error.to_string());
            primary = Some(io_pipeline_error("start deferred process cleanup", detail));
        }
    }
    primary = finish_reader_cleanup(primary, readers_deferred);

    CleanupResult { status, primary }
}

fn latch_nonzero(status: Option<ObservedExit>, pending_nonzero: &mut Option<ObservedExit>) -> bool {
    let Some(status) = status.filter(|status| !status.success()) else {
        return false;
    };
    if pending_nonzero.is_none() {
        *pending_nonzero = Some(status);
    }
    true
}

fn recycle_frame(
    sender: &SyncSender<Vec<u8>>,
    mut frame: Vec<u8>,
    started: Instant,
    timeout: Duration,
    context: &OperationContext,
) -> Result<(), PipelineError> {
    loop {
        context.checkpoint()?;
        if started.elapsed() >= timeout {
            return Err(PipelineError::TimedOut {
                stage: "child-process",
            });
        }
        match sender.try_send(frame) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                frame = returned;
                thread::sleep(CONTROL_POLL_INTERVAL);
            }
            Err(TrySendError::Disconnected(_)) => return Ok(()),
        }
    }
}

fn captured_guard(
    stdout: &Arc<Mutex<BoundedReaderState>>,
    stderr: &Arc<Mutex<BoundedReaderState>>,
    limits: ProcessLimits,
) -> Option<PipelineError> {
    let stdout = lock_unpoisoned(stdout);
    let stderr = lock_unpoisoned(stderr);
    if let Some(actual) = stdout.overflow_actual {
        return Some(PipelineError::LimitExceeded {
            resource: "process-stdout-bytes",
            limit: as_u64(limits.max_stdout_bytes),
            actual,
        });
    }
    if let Some(actual) = stderr.overflow_actual {
        return Some(PipelineError::LimitExceeded {
            resource: "process-stderr-bytes",
            limit: as_u64(limits.max_stderr_bytes),
            actual,
        });
    }
    if let Some(error) = stdout.error.as_ref() {
        return Some(io_pipeline_error("read child stdout", error));
    }
    if let Some(error) = stderr.error.as_ref() {
        return Some(io_pipeline_error("read child stderr", error));
    }
    None
}

pub(crate) fn run_captured(
    program: &OsStr,
    args: &[OsString],
    limits: ProcessLimits,
    context: &OperationContext,
) -> Result<CapturedProcess, PipelineError> {
    context.checkpoint()?;
    if limits.timeout.is_zero() {
        return Err(PipelineError::TimedOut {
            stage: "child-process",
        });
    }
    let started = Instant::now();
    let mut child = spawn_managed_child(program, args)?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let primary = Some(io_pipeline_error(
                "capture child stdout",
                "child stdout pipe was unavailable",
            ));
            let cleanup = cleanup_child_and_readers(child, None, None, None, primary);
            return Err(cleanup.primary.expect("missing stdout primary error"));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            let primary = Some(io_pipeline_error(
                "capture child stderr",
                "child stderr pipe was unavailable",
            ));
            let cleanup = cleanup_child_and_readers(child, None, None, None, primary);
            return Err(cleanup.primary.expect("missing stderr primary error"));
        }
    };
    let stdout_state = Arc::new(Mutex::new(BoundedReaderState::new(limits.max_stdout_bytes)));
    let stderr_state = Arc::new(Mutex::new(BoundedReaderState::new(limits.max_stderr_bytes)));
    let stdout_handle = match spawn_bounded_reader(
        "stickerfit-process-stdout-capture",
        stdout,
        limits.max_stdout_bytes,
        Arc::clone(&stdout_state),
    ) {
        Ok(handle) => handle,
        Err(error) => {
            drop(stderr);
            let primary = Some(io_pipeline_error("start child stdout reader", error));
            let cleanup = cleanup_child_and_readers(child, None, None, None, primary);
            return Err(cleanup.primary.expect("stdout reader primary error"));
        }
    };
    let stderr_handle = match spawn_bounded_reader(
        "stickerfit-process-stderr-capture",
        stderr,
        limits.max_stderr_bytes,
        Arc::clone(&stderr_state),
    ) {
        Ok(handle) => handle,
        Err(error) => {
            let primary = Some(io_pipeline_error("start child stderr reader", error));
            let cleanup =
                cleanup_child_and_readers(child, None, Some(stdout_handle), None, primary);
            return Err(cleanup.primary.expect("stderr reader primary error"));
        }
    };

    let mut primary = None;
    let mut status = None;
    let mut pending_nonzero = None;
    loop {
        if let Err(error) = context.checkpoint() {
            primary = Some(error);
            break;
        }
        if started.elapsed() >= limits.timeout {
            primary = Some(PipelineError::TimedOut {
                stage: "child-process",
            });
            break;
        }
        if let Some(error) = captured_guard(&stdout_state, &stderr_state, limits) {
            primary = Some(error);
            break;
        }
        if stdout_handle.is_finished() && !lock_unpoisoned(&stdout_state).done {
            primary = Some(io_pipeline_error(
                "read child stdout",
                "stdout reader terminated without completion",
            ));
            break;
        }
        if stderr_handle.is_finished() && !lock_unpoisoned(&stderr_state).done {
            primary = Some(io_pipeline_error(
                "read child stderr",
                "stderr reader terminated without completion",
            ));
            break;
        }
        if let Err(error) = poll_child_status(&mut child, &mut status) {
            primary = Some(error);
            break;
        }
        if status.is_some_and(|status| !status.success()) {
            if let Some(error) = captured_guard(&stdout_state, &stderr_state, limits) {
                primary = Some(error);
                break;
            }
            if stdout_handle.is_finished() && !lock_unpoisoned(&stdout_state).done {
                primary = Some(io_pipeline_error(
                    "read child stdout",
                    "stdout reader terminated without completion",
                ));
                break;
            }
            if stderr_handle.is_finished() && !lock_unpoisoned(&stderr_state).done {
                primary = Some(io_pipeline_error(
                    "read child stderr",
                    "stderr reader terminated without completion",
                ));
                break;
            }
            if latch_nonzero(status, &mut pending_nonzero) {
                break;
            }
        }
        let readers_done =
            lock_unpoisoned(&stdout_state).done && lock_unpoisoned(&stderr_state).done;
        if status.is_some() && readers_done {
            break;
        }
        thread::sleep(CONTROL_POLL_INTERVAL);
    }

    let CleanupResult {
        status,
        primary: cleanup_primary,
    } = cleanup_child_and_readers(
        child,
        status,
        Some(stdout_handle),
        Some(stderr_handle),
        primary,
    );
    primary = cleanup_primary;
    if primary.is_none() {
        primary = captured_guard(&stdout_state, &stderr_state, limits);
    }
    if let Some(error) = primary {
        return Err(error);
    }

    let status = status.ok_or_else(|| {
        io_pipeline_error(
            "wait for child process",
            "child status was unavailable after reader completion",
        )
    })?;
    let stdout = std::mem::take(&mut lock_unpoisoned(&stdout_state).bytes);
    let stderr = std::mem::take(&mut lock_unpoisoned(&stderr_state).bytes);
    if let Some(nonzero) = pending_nonzero.or_else(|| (!status.success()).then_some(status)) {
        return Err(process_failed(program, nonzero, &stderr));
    }
    Ok(CapturedProcess { stdout, stderr })
}

#[derive(Debug)]
enum PreparedStreamEvent {
    Frame {
        frame: Vec<u8>,
        next_count: usize,
        next_bytes: usize,
    },
    Eof(Vec<u8>),
    Idle,
}

fn prepare_stream_event(
    event: Option<FrameEvent>,
    frame_count: usize,
    decoded_bytes: usize,
    max_frames: usize,
    decoded_limit: usize,
) -> Result<PreparedStreamEvent, PipelineError> {
    match event {
        Some(FrameEvent::Frame(frame)) => {
            let next_count = frame_count
                .checked_add(1)
                .ok_or(PipelineError::LimitExceeded {
                    resource: "decoded-bytes",
                    limit: as_u64(decoded_limit),
                    actual: u64::MAX,
                })?;
            let next_bytes =
                decoded_bytes
                    .checked_add(frame.len())
                    .ok_or(PipelineError::LimitExceeded {
                        resource: "decoded-bytes",
                        limit: as_u64(decoded_limit),
                        actual: u64::MAX,
                    })?;
            if next_count > max_frames || next_bytes > decoded_limit {
                return Err(PipelineError::LimitExceeded {
                    resource: "decoded-bytes",
                    limit: as_u64(decoded_limit),
                    actual: as_u64(next_bytes),
                });
            }
            Ok(PreparedStreamEvent::Frame {
                frame,
                next_count,
                next_bytes,
            })
        }
        Some(FrameEvent::Eof(partial)) => {
            let actual =
                decoded_bytes
                    .checked_add(partial.len())
                    .ok_or(PipelineError::LimitExceeded {
                        resource: "decoded-bytes",
                        limit: as_u64(decoded_limit),
                        actual: u64::MAX,
                    })?;
            if actual > decoded_limit {
                return Err(PipelineError::LimitExceeded {
                    resource: "decoded-bytes",
                    limit: as_u64(decoded_limit),
                    actual: as_u64(actual),
                });
            }
            Ok(PreparedStreamEvent::Eof(partial))
        }
        Some(FrameEvent::ReadError(error)) => Err(io_pipeline_error("read child stdout", error)),
        None => Ok(PreparedStreamEvent::Idle),
    }
}

pub(crate) fn stream_fixed_rgba_frames<F>(
    program: &OsStr,
    args: &[OsString],
    frame_size: usize,
    max_frames: usize,
    limits: ProcessLimits,
    context: &OperationContext,
    mut on_frame: F,
) -> Result<FrameStreamSummary, PipelineError>
where
    F: FnMut(&[u8]) -> Result<(), PipelineError>,
{
    context.checkpoint()?;
    if frame_size == 0 || max_frames == 0 {
        return Err(PipelineError::InvalidRequest {
            reason: "invalid-process-frame-config",
        });
    }
    let frame_ceiling = frame_size
        .checked_mul(max_frames)
        .ok_or(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: as_u64(limits.max_stdout_bytes),
            actual: u64::MAX,
        })?;
    let decoded_limit = limits.max_stdout_bytes.min(frame_ceiling);
    if frame_size > decoded_limit {
        return Err(PipelineError::LimitExceeded {
            resource: "decoded-bytes",
            limit: as_u64(decoded_limit),
            actual: as_u64(frame_size),
        });
    }
    if limits.timeout.is_zero() {
        return Err(PipelineError::TimedOut {
            stage: "child-process",
        });
    }
    let buffer_count = frame_buffer_count(frame_size, max_frames, decoded_limit);
    let mut frame_buffers = Vec::with_capacity(buffer_count);
    for _ in 0..buffer_count {
        let mut frame = Vec::new();
        frame
            .try_reserve_exact(frame_size)
            .map_err(|error| io_pipeline_error("allocate child stdout frame", error))?;
        frame_buffers.push(frame);
    }
    let initial_frame = frame_buffers
        .pop()
        .expect("validated frame buffer count is positive");
    let started = Instant::now();
    let mut child = spawn_managed_child(program, args)?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let primary = Some(io_pipeline_error(
                "stream child stdout",
                "child stdout pipe was unavailable",
            ));
            let cleanup = cleanup_child_and_readers(child, None, None, None, primary);
            return Err(cleanup.primary.expect("missing stdout primary error"));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            let primary = Some(io_pipeline_error(
                "capture child stderr",
                "child stderr pipe was unavailable",
            ));
            let cleanup = cleanup_child_and_readers(child, None, None, None, primary);
            return Err(cleanup.primary.expect("missing stderr primary error"));
        }
    };

    let (frame_sender, frame_receiver) = mpsc::sync_channel::<FrameEvent>(2);
    let (recycle_sender, recycle_receiver) = mpsc::sync_channel::<Vec<u8>>(2);
    for frame in frame_buffers {
        recycle_sender.send(frame).expect("empty recycle channel");
    }
    let stdout_handle = match spawn_frame_reader(
        stdout,
        frame_size,
        initial_frame,
        frame_sender,
        recycle_receiver,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            drop(frame_receiver);
            drop(recycle_sender);
            drop(stderr);
            let primary = Some(io_pipeline_error("start child stdout reader", error));
            let cleanup = cleanup_child_and_readers(child, None, None, None, primary);
            return Err(cleanup.primary.expect("stdout reader primary error"));
        }
    };
    let stderr_state = Arc::new(Mutex::new(PrefixReaderState::new(limits.max_stderr_bytes)));
    let stderr_handle = match spawn_prefix_reader(
        "stickerfit-process-stderr-prefix",
        stderr,
        limits.max_stderr_bytes,
        Arc::clone(&stderr_state),
    ) {
        Ok(handle) => handle,
        Err(error) => {
            drop(frame_receiver);
            drop(recycle_sender);
            let primary = Some(io_pipeline_error("start child stderr reader", error));
            let cleanup =
                cleanup_child_and_readers(child, None, Some(stdout_handle), None, primary);
            return Err(cleanup.primary.expect("stderr reader primary error"));
        }
    };

    let mut primary = None;
    let mut status = None;
    let mut frame_count = 0_usize;
    let mut decoded_bytes = 0_usize;
    let mut partial_bytes = 0_usize;
    let mut stdout_done = false;
    let mut pending_nonzero = None;

    loop {
        if let Err(error) = context.checkpoint() {
            primary = Some(error);
            break;
        }
        if started.elapsed() >= limits.timeout {
            primary = Some(PipelineError::TimedOut {
                stage: "child-process",
            });
            break;
        }
        if let Some(error) = lock_unpoisoned(&stderr_state).error.clone() {
            primary = Some(io_pipeline_error("read child stderr", error));
            break;
        }
        if stderr_handle.is_finished() && !lock_unpoisoned(&stderr_state).done {
            primary = Some(io_pipeline_error(
                "read child stderr",
                "stderr reader terminated without completion",
            ));
            break;
        }

        let event = if stdout_done {
            thread::sleep(CONTROL_POLL_INTERVAL);
            None
        } else {
            match frame_receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    primary = Some(io_pipeline_error(
                        "read child stdout",
                        "stdout reader disconnected without terminal event",
                    ));
                    break;
                }
            }
        };

        if let Err(error) = context.checkpoint() {
            primary = Some(error);
            break;
        }
        if started.elapsed() >= limits.timeout {
            primary = Some(PipelineError::TimedOut {
                stage: "child-process",
            });
            break;
        }
        if let Some(error) = lock_unpoisoned(&stderr_state).error.clone() {
            primary = Some(io_pipeline_error("read child stderr", error));
            break;
        }
        if stderr_handle.is_finished() && !lock_unpoisoned(&stderr_state).done {
            primary = Some(io_pipeline_error(
                "read child stderr",
                "stderr reader terminated without completion",
            ));
            break;
        }

        let prepared_event = match prepare_stream_event(
            event,
            frame_count,
            decoded_bytes,
            max_frames,
            decoded_limit,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                primary = Some(error);
                break;
            }
        };

        if let Err(error) = poll_child_status(&mut child, &mut status) {
            primary = Some(error);
            break;
        }
        if latch_nonzero(status, &mut pending_nonzero) {
            break;
        }

        match prepared_event {
            PreparedStreamEvent::Frame {
                mut frame,
                next_count,
                next_bytes,
            } => {
                if let Err(error) = on_frame(&frame) {
                    primary = Some(error);
                    break;
                }
                frame_count = next_count;
                decoded_bytes = next_bytes;
                frame.clear();
                if let Err(error) =
                    recycle_frame(&recycle_sender, frame, started, limits.timeout, context)
                {
                    primary = Some(error);
                    break;
                }
            }
            PreparedStreamEvent::Eof(partial) => {
                partial_bytes = partial.len();
                stdout_done = true;
            }
            PreparedStreamEvent::Idle => {}
        }

        let stderr_done = lock_unpoisoned(&stderr_state).done;
        if stdout_done && stderr_done && status.is_some() {
            if status.as_ref().is_some_and(|status| status.success()) && partial_bytes > 0 {
                primary = Some(PipelineError::MalformedProcessOutput {
                    reason: "raw RGBA output ended with a partial frame".into(),
                });
            }
            break;
        }
    }

    drop(frame_receiver);
    drop(recycle_sender);
    let CleanupResult {
        status,
        primary: cleanup_primary,
    } = cleanup_child_and_readers(
        child,
        status,
        Some(stdout_handle),
        Some(stderr_handle),
        primary,
    );
    primary = cleanup_primary;
    if primary.is_none() {
        if let Some(error) = lock_unpoisoned(&stderr_state).error.clone() {
            primary = Some(io_pipeline_error("read child stderr", error));
        }
    }
    if let Some(error) = primary {
        return Err(error);
    }

    let status = status.ok_or_else(|| {
        io_pipeline_error(
            "wait for child process",
            "child status was unavailable after reader completion",
        )
    })?;
    let stderr = std::mem::take(&mut lock_unpoisoned(&stderr_state).bytes);
    if let Some(nonzero) = pending_nonzero.or_else(|| (!status.success()).then_some(status)) {
        return Err(process_failed(program, nonzero, &stderr));
    }
    Ok(FrameStreamSummary { frame_count })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::{OsStr, OsString};
    use std::io::{self, Write};
    use std::path::PathBuf;
    use std::process;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::media_error::PipelineError;
    use crate::operation::OperationContext;

    use super::{
        run_captured, stream_fixed_rgba_frames, ChildLifecycle, ObservedExit, ProcessLimits,
        TerminationPoll,
    };

    const FIXTURE_ENTRY: &str = "process_runner::tests::fixture_entry";
    const FIXTURE_MODE: &str = "STICKERFIT_PROCESS_RUNNER_FIXTURE_MODE";
    const FIXTURE_BYTES: &str = "STICKERFIT_PROCESS_RUNNER_FIXTURE_BYTES";
    const FIXTURE_OFFSET: &str = "STICKERFIT_PROCESS_RUNNER_FIXTURE_OFFSET";
    const FIXTURE_EXIT: &str = "STICKERFIT_PROCESS_RUNNER_FIXTURE_EXIT";
    const FIXTURE_PARTIAL: &str = "STICKERFIT_PROCESS_RUNNER_FIXTURE_PARTIAL";
    const FIXTURE_MARKER: &str = "STICKERFIT_PROCESS_RUNNER_FIXTURE_MARKER";
    const UNICODE_SKIP_TOKEN: &str = "스티커핏-유니코드-인수";
    const PROBE_MARKER: &[u8] = b"<stickerfit-fixture-probe-7f3a9c>";
    const ONE_MIB: usize = 1024 * 1024;

    static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct ScriptedChild {
        polls: VecDeque<io::Result<Option<ObservedExit>>>,
        kills: VecDeque<io::Result<()>>,
        kill_calls: usize,
        wait_calls: usize,
    }

    impl ChildLifecycle for ScriptedChild {
        fn try_wait_exit(&mut self) -> io::Result<Option<ObservedExit>> {
            self.polls.pop_front().unwrap_or(Ok(None))
        }

        fn kill_child(&mut self) -> io::Result<()> {
            self.kill_calls += 1;
            self.kills.pop_front().unwrap_or(Ok(()))
        }

        fn wait_exit(&mut self) -> io::Result<ObservedExit> {
            self.wait_calls += 1;
            panic!("caller-side cleanup must never use blocking wait")
        }
    }

    fn observed_exit(code: i32) -> ObservedExit {
        ObservedExit {
            success: code == 0,
            exit_code: Some(code),
        }
    }

    struct EnvironmentScope {
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvironmentScope {
        fn new() -> Self {
            Self {
                previous: Vec::new(),
            }
        }

        fn set(&mut self, key: &'static str, value: impl AsRef<OsStr>) {
            if !self.previous.iter().any(|(existing, _)| *existing == key) {
                self.previous.push((key, std::env::var_os(key)));
            }
            std::env::set_var(key, value);
        }

        fn remove(&mut self, key: &'static str) {
            if !self.previous.iter().any(|(existing, _)| *existing == key) {
                self.previous.push((key, std::env::var_os(key)));
            }
            std::env::remove_var(key);
        }
    }

    impl Drop for EnvironmentScope {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..).rev() {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    struct FixtureSession {
        environment: EnvironmentScope,
        _lock: MutexGuard<'static, ()>,
        program: PathBuf,
        args: Vec<OsString>,
        prefix: Vec<u8>,
    }

    impl FixtureSession {
        fn start() -> Self {
            let lock = FIXTURE_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut environment = EnvironmentScope::new();
            for key in [
                FIXTURE_MODE,
                FIXTURE_BYTES,
                FIXTURE_OFFSET,
                FIXTURE_EXIT,
                FIXTURE_PARTIAL,
                FIXTURE_MARKER,
            ] {
                environment.remove(key);
            }
            let program = std::env::current_exe().expect("current libtest executable");
            let args = fixture_args();
            environment.set(FIXTURE_MODE, "probe");
            environment.set(
                FIXTURE_MARKER,
                OsString::from(String::from_utf8_lossy(PROBE_MARKER).into_owned()),
            );
            let first = run_captured(
                program.as_os_str(),
                &args,
                capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
                &OperationContext::detached(Duration::from_secs(2)),
            )
            .expect("first fixture prefix probe");
            let second = run_captured(
                program.as_os_str(),
                &args,
                capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
                &OperationContext::detached(Duration::from_secs(2)),
            )
            .expect("second fixture prefix probe");
            let first_offset = marker_offset(&first.stdout);
            let second_offset = marker_offset(&second.stdout);
            assert_eq!(first_offset, second_offset, "libtest prefix must be stable");
            assert_eq!(
                &first.stdout[..first_offset],
                &second.stdout[..second_offset]
            );

            Self {
                environment,
                _lock: lock,
                program,
                args,
                prefix: first.stdout[..first_offset].to_vec(),
            }
        }

        fn configure_payload(&mut self, mode: &str, desired_total: usize) {
            assert!(
                self.prefix.len() <= desired_total,
                "fixture prefix must fit requested output"
            );
            self.environment.set(FIXTURE_MODE, mode);
            self.environment.set(
                FIXTURE_BYTES,
                (desired_total - self.prefix.len()).to_string(),
            );
            self.environment
                .set(FIXTURE_OFFSET, self.prefix.len().to_string());
            self.environment.remove(FIXTURE_PARTIAL);
            self.environment.remove(FIXTURE_EXIT);
        }

        fn configure_stderr(&mut self, bytes: usize, exit_code: i32) {
            self.environment.set(FIXTURE_MODE, "stderr-bytes");
            self.environment.set(FIXTURE_BYTES, bytes.to_string());
            self.environment.set(FIXTURE_OFFSET, "0");
            self.environment.set(FIXTURE_EXIT, exit_code.to_string());
        }

        fn context(&self, timeout: Duration) -> OperationContext {
            OperationContext::detached(timeout)
        }
    }

    fn fixture_args() -> Vec<OsString> {
        [
            "--ignored",
            "--exact",
            FIXTURE_ENTRY,
            "--test-threads=1",
            "--color",
            "never",
            "--nocapture",
            "--skip",
            UNICODE_SKIP_TOKEN,
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    fn capture_limits(
        timeout: Duration,
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
    ) -> ProcessLimits {
        ProcessLimits {
            timeout,
            max_stdout_bytes,
            max_stderr_bytes,
        }
    }

    fn marker_offset(output: &[u8]) -> usize {
        assert!(
            output.ends_with(PROBE_MARKER),
            "probe marker must be the final child bytes"
        );
        let matches = output
            .windows(PROBE_MARKER.len())
            .enumerate()
            .filter_map(|(offset, candidate)| (candidate == PROBE_MARKER).then_some(offset))
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "probe marker must occur exactly once");
        matches[0]
    }

    fn payload_byte(offset: usize) -> u8 {
        b'a' + u8::try_from(offset % 26).expect("payload modulo")
    }

    fn write_payload(writer: &mut impl Write, bytes: usize, offset: usize) {
        const CHUNK: usize = 4096;
        let mut written = 0;
        while written < bytes {
            let count = (bytes - written).min(CHUNK);
            let chunk = (0..count)
                .map(|index| payload_byte(offset + written + index))
                .collect::<Vec<_>>();
            writer.write_all(&chunk).expect("fixture payload write");
            written += count;
        }
        writer.flush().expect("fixture payload flush");
    }

    fn env_usize(key: &str) -> usize {
        std::env::var(key)
            .unwrap_or_else(|_| panic!("missing fixture variable {key}"))
            .parse()
            .unwrap_or_else(|_| panic!("invalid fixture variable {key}"))
    }

    fn env_i32(key: &str, default: i32) -> i32 {
        std::env::var(key)
            .ok()
            .map(|value| value.parse().expect("fixture exit code"))
            .unwrap_or(default)
    }

    #[test]
    #[ignore]
    fn fixture_entry() {
        let Some(mode) = std::env::var_os(FIXTURE_MODE) else {
            return;
        };
        let mode = mode.to_string_lossy();
        match mode.as_ref() {
            "probe" => {
                assert!(
                    std::env::args_os().any(|argument| argument == OsStr::new(UNICODE_SKIP_TOKEN)),
                    "Unicode OsString token must survive re-exec"
                );
                let marker = std::env::var(FIXTURE_MARKER).expect("fixture marker");
                let mut stdout = io::stdout().lock();
                stdout
                    .write_all(marker.as_bytes())
                    .expect("probe marker write");
                stdout.flush().expect("probe marker flush");
                process::exit(0);
            }
            "stdout-bytes" | "frames" | "frames-hang" | "frames-partial" => {
                let bytes = env_usize(FIXTURE_BYTES);
                let offset = env_usize(FIXTURE_OFFSET);
                let mut stdout = io::stdout().lock();
                write_payload(&mut stdout, bytes, offset);
                if mode == "frames-partial" {
                    let partial = env_usize(FIXTURE_PARTIAL);
                    write_payload(&mut stdout, partial, offset + bytes);
                }
                if mode == "frames-hang" {
                    thread::sleep(Duration::from_secs(30));
                }
                process::exit(env_i32(FIXTURE_EXIT, 0));
            }
            "stderr-bytes" => {
                let bytes = env_usize(FIXTURE_BYTES);
                let offset = env_usize(FIXTURE_OFFSET);
                let mut stderr = io::stderr().lock();
                write_payload(&mut stderr, bytes, offset);
                process::exit(env_i32(FIXTURE_EXIT, 0));
            }
            "parseable-nonzero" => {
                io::stderr()
                    .write_all(b"Video: h264, yuv420p, 320x240, 30 fps\n")
                    .expect("parseable stderr write");
                io::stderr().flush().expect("parseable stderr flush");
                process::exit(17);
            }
            "silent-hang" => thread::sleep(Duration::from_secs(30)),
            unexpected => panic!("unexpected fixture mode {unexpected}"),
        }
    }

    #[test]
    fn captured_unicode_os_string_round_trip_is_bounded() {
        let session = FixtureSession::start();
        let output = run_captured(
            session.program.as_os_str(),
            &session.args,
            capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
            &session.context(Duration::from_secs(2)),
        )
        .expect("Unicode fixture probe");

        assert_eq!(marker_offset(&output.stdout), session.prefix.len());
        assert_eq!(&output.stdout[..session.prefix.len()], session.prefix);
    }

    #[test]
    fn parseable_stderr_with_exit_17_is_process_failed_not_success() {
        let mut session = FixtureSession::start();
        session.environment.set(FIXTURE_MODE, "parseable-nonzero");
        let result = run_captured(
            session.program.as_os_str(),
            &session.args,
            capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
            &session.context(Duration::from_secs(2)),
        );

        assert_eq!(
            result,
            Err(PipelineError::ProcessFailed {
                command: session.program.to_string_lossy().into_owned(),
                exit_code: Some(17),
                stderr: "Video: h264, yuv420p, 320x240, 30 fps\n".into(),
            })
        );
    }

    #[test]
    fn captured_silent_child_times_out_and_is_reaped_under_outer_bound() {
        let mut session = FixtureSession::start();
        session.environment.set(FIXTURE_MODE, "silent-hang");
        let started = Instant::now();
        let result = run_captured(
            session.program.as_os_str(),
            &session.args,
            capture_limits(Duration::from_millis(200), ONE_MIB, ONE_MIB),
            &session.context(Duration::from_secs(5)),
        );

        assert!(matches!(
            result,
            Err(PipelineError::TimedOut {
                stage: "child-process"
            })
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn captured_stdout_exact_cap_succeeds_and_first_extra_byte_is_rejected() {
        let mut session = FixtureSession::start();
        session.configure_payload("stdout-bytes", ONE_MIB);
        let exact = run_captured(
            session.program.as_os_str(),
            &session.args,
            capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
            &session.context(Duration::from_secs(2)),
        )
        .expect("stdout exactly at cap");
        assert_eq!(exact.stdout.len(), ONE_MIB);

        session.configure_payload("stdout-bytes", ONE_MIB + 1);
        let started = Instant::now();
        assert!(matches!(
            run_captured(
                session.program.as_os_str(),
                &session.args,
                capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
                &session.context(Duration::from_secs(2)),
            ),
            Err(PipelineError::LimitExceeded {
                resource: "process-stdout-bytes",
                limit,
                actual,
            }) if limit == ONE_MIB as u64 && actual == (ONE_MIB + 1) as u64
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn captured_stdout_guard_beats_natural_nonzero_exit() {
        let mut session = FixtureSession::start();
        let cap = session.prefix.len() + 64;
        session.configure_payload("stdout-bytes", cap + 1);
        session.environment.set(FIXTURE_EXIT, "17");

        assert!(matches!(
            run_captured(
                session.program.as_os_str(),
                &session.args,
                capture_limits(Duration::from_secs(2), cap, ONE_MIB),
                &session.context(Duration::from_secs(2)),
            ),
            Err(PipelineError::LimitExceeded {
                resource: "process-stdout-bytes",
                limit,
                actual,
            }) if limit == cap as u64 && actual == (cap + 1) as u64
        ));
    }

    #[test]
    fn captured_stderr_exact_cap_succeeds_and_first_extra_byte_is_rejected() {
        let mut session = FixtureSession::start();
        session.configure_stderr(ONE_MIB, 0);
        let exact = run_captured(
            session.program.as_os_str(),
            &session.args,
            capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
            &session.context(Duration::from_secs(2)),
        )
        .expect("stderr exactly at cap");
        assert_eq!(exact.stderr.len(), ONE_MIB);

        session.configure_stderr(ONE_MIB + 1, 0);
        let started = Instant::now();
        assert!(matches!(
            run_captured(
                session.program.as_os_str(),
                &session.args,
                capture_limits(Duration::from_secs(2), ONE_MIB, ONE_MIB),
                &session.context(Duration::from_secs(2)),
            ),
            Err(PipelineError::LimitExceeded {
                resource: "process-stderr-bytes",
                limit,
                actual,
            }) if limit == ONE_MIB as u64 && actual == (ONE_MIB + 1) as u64
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn streaming_4096_frames_validates_every_prefix_and_payload_byte() {
        const FRAME_SIZE: usize = 64;
        const FRAME_COUNT: usize = 4096;
        const TOTAL: usize = FRAME_SIZE * FRAME_COUNT;
        let mut session = FixtureSession::start();
        session.configure_payload("frames", TOTAL);
        let expected_prefix = session.prefix.clone();
        let mut observed_offset = 0;
        let summary = stream_fixed_rgba_frames(
            session.program.as_os_str(),
            &session.args,
            FRAME_SIZE,
            FRAME_COUNT,
            capture_limits(Duration::from_secs(5), TOTAL, ONE_MIB),
            &session.context(Duration::from_secs(5)),
            |frame| {
                for (index, actual) in frame.iter().copied().enumerate() {
                    let absolute = observed_offset + index;
                    let expected = expected_prefix
                        .get(absolute)
                        .copied()
                        .unwrap_or_else(|| payload_byte(absolute));
                    assert_eq!(actual, expected, "stream byte offset {absolute}");
                }
                observed_offset += frame.len();
                Ok(())
            },
        )
        .expect("complete fixed-frame stream");

        assert_eq!(summary.frame_count, FRAME_COUNT);
        assert_eq!(observed_offset, TOTAL);
    }

    #[test]
    fn streaming_partial_tail_callbacks_only_complete_frames_then_is_malformed() {
        const FRAME_SIZE: usize = 64;
        const COMPLETE_FRAMES: usize = 3;
        let mut session = FixtureSession::start();
        session.configure_payload("frames-partial", FRAME_SIZE * COMPLETE_FRAMES);
        session.environment.set(FIXTURE_PARTIAL, "31");
        let callbacks = AtomicUsize::new(0);
        let result = stream_fixed_rgba_frames(
            session.program.as_os_str(),
            &session.args,
            FRAME_SIZE,
            10,
            capture_limits(Duration::from_secs(2), FRAME_SIZE * 10, ONE_MIB),
            &session.context(Duration::from_secs(2)),
            |_| {
                callbacks.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(PipelineError::MalformedProcessOutput { .. })
        ));
        assert_eq!(callbacks.load(Ordering::SeqCst), COMPLETE_FRAMES);
    }

    #[test]
    fn fourth_frame_exceeds_three_frame_limit_without_fourth_callback() {
        const FRAME_SIZE: usize = 64;
        let mut session = FixtureSession::start();
        session.configure_payload("frames-hang", FRAME_SIZE * 4);
        let callbacks = AtomicUsize::new(0);
        let started = Instant::now();
        let result = stream_fixed_rgba_frames(
            session.program.as_os_str(),
            &session.args,
            FRAME_SIZE,
            3,
            capture_limits(Duration::from_secs(5), ONE_MIB, ONE_MIB),
            &session.context(Duration::from_secs(5)),
            |_| {
                callbacks.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );

        assert_eq!(
            result,
            Err(PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 192,
                actual: 256,
            })
        );
        assert_eq!(callbacks.load(Ordering::SeqCst), 3);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn callback_sentinel_is_retained_while_hanging_child_is_reaped() {
        const FRAME_SIZE: usize = 64;
        let mut session = FixtureSession::start();
        session.configure_payload("frames-hang", FRAME_SIZE * 16);
        let callbacks = AtomicUsize::new(0);
        let sentinel = PipelineError::InvalidRequest {
            reason: "fixture-callback-sentinel",
        };
        let started = Instant::now();
        let result = stream_fixed_rgba_frames(
            session.program.as_os_str(),
            &session.args,
            FRAME_SIZE,
            16,
            capture_limits(Duration::from_secs(5), FRAME_SIZE * 16, ONE_MIB),
            &session.context(Duration::from_secs(5)),
            |_| {
                let callback = callbacks.fetch_add(1, Ordering::SeqCst) + 1;
                if callback == 3 {
                    return Err(sentinel.clone());
                }
                Ok(())
            },
        );

        assert_eq!(result, Err(sentinel));
        assert_eq!(callbacks.load(Ordering::SeqCst), 3);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn streaming_silent_child_uses_watchdog_timeout_and_reaps() {
        let mut session = FixtureSession::start();
        session.environment.set(FIXTURE_MODE, "silent-hang");
        let frame_size = session
            .prefix
            .len()
            .checked_add(1)
            .expect("silent fixture frame size");
        let callbacks = AtomicUsize::new(0);
        let started = Instant::now();
        let result = stream_fixed_rgba_frames(
            session.program.as_os_str(),
            &session.args,
            frame_size,
            1,
            capture_limits(Duration::from_millis(200), frame_size, ONE_MIB),
            &session.context(Duration::from_secs(5)),
            |_| {
                callbacks.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(PipelineError::TimedOut {
                stage: "child-process"
            })
        ));
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn streaming_drains_large_stderr_but_nonzero_retains_only_prefix() {
        const STDERR_CAP: usize = 128;
        let mut session = FixtureSession::start();
        session.configure_stderr(ONE_MIB, 17);
        let started = Instant::now();
        let result = stream_fixed_rgba_frames(
            session.program.as_os_str(),
            &session.args,
            ONE_MIB,
            1,
            capture_limits(Duration::from_secs(5), ONE_MIB, STDERR_CAP),
            &session.context(Duration::from_secs(5)),
            |_| Ok(()),
        );

        let Err(PipelineError::ProcessFailed {
            exit_code: Some(17),
            stderr,
            ..
        }) = result
        else {
            panic!("expected bounded ProcessFailed, got {result:?}");
        };
        assert_eq!(stderr.len(), STDERR_CAP);
        assert!(stderr
            .bytes()
            .enumerate()
            .all(|(offset, byte)| byte == payload_byte(offset)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn capacity_two_stream_cancellation_joins_reader_after_blocked_callback() {
        const FRAME_SIZE: usize = 64;
        const FRAME_COUNT: usize = 4096;
        let mut session = FixtureSession::start();
        session.configure_payload("frames-hang", FRAME_SIZE * FRAME_COUNT);
        let context = OperationContext::detached(Duration::from_secs(5));
        let cancellation = context.clone();
        let program = session.program.clone();
        let args = session.args.clone();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_worker = Arc::clone(&callback_count);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            stream_fixed_rgba_frames(
                program.as_os_str(),
                &args,
                FRAME_SIZE,
                FRAME_COUNT,
                capture_limits(Duration::from_secs(5), FRAME_SIZE * FRAME_COUNT, ONE_MIB),
                &context,
                |_| {
                    let count = callback_count_worker.fetch_add(1, Ordering::SeqCst) + 1;
                    assert_eq!(count, 1, "no callback may follow visible cancellation");
                    entered_tx.send(()).expect("callback entered signal");
                    release_rx.recv().expect("callback release signal");
                    Ok(())
                },
            )
        });

        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first callback must start");
        cancellation.cancel();
        thread::sleep(Duration::from_millis(150));
        let released_at = Instant::now();
        release_tx.send(()).expect("release callback");
        let result = worker.join().expect("stream worker join");

        assert_eq!(result, Err(PipelineError::Cancelled));
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        assert!(released_at.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn invalid_frame_configuration_and_overflow_are_rejected_before_spawn() {
        let missing_program = OsStr::new("stickerfit-process-runner-must-not-spawn");
        let context = OperationContext::detached(Duration::from_secs(1));
        let limits = capture_limits(Duration::from_secs(1), usize::MAX, 16);

        for (frame_size, max_frames) in [(0, 1), (1, 0)] {
            assert!(matches!(
                stream_fixed_rgba_frames(
                    missing_program,
                    &[],
                    frame_size,
                    max_frames,
                    limits,
                    &context,
                    |_| Ok(())
                ),
                Err(PipelineError::InvalidRequest {
                    reason: "invalid-process-frame-config"
                })
            ));
        }
        assert!(matches!(
            stream_fixed_rgba_frames(
                missing_program,
                &[],
                usize::MAX,
                2,
                limits,
                &context,
                |_| Ok(())
            ),
            Err(PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                actual: u64::MAX,
                ..
            })
        ));
        assert_eq!(
            stream_fixed_rgba_frames(
                missing_program,
                &[],
                32,
                1,
                capture_limits(Duration::from_secs(1), 16, 16),
                &context,
                |_| Ok(())
            ),
            Err(PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 16,
                actual: 32,
            })
        );
        assert_eq!(
            run_captured(
                missing_program,
                &[],
                capture_limits(Duration::ZERO, 16, 16),
                &context,
            ),
            Err(PipelineError::TimedOut {
                stage: "child-process",
            })
        );
        assert_eq!(
            stream_fixed_rgba_frames(
                missing_program,
                &[],
                1,
                1,
                capture_limits(Duration::ZERO, 1, 16),
                &context,
                |_| Ok(())
            ),
            Err(PipelineError::TimedOut {
                stage: "child-process",
            })
        );
    }

    #[test]
    fn frame_buffer_pool_never_reserves_more_full_frames_than_the_work_can_use() {
        assert_eq!(
            super::frame_buffer_count(160 * ONE_MIB, 1, 160 * ONE_MIB),
            1
        );
        assert_eq!(super::frame_buffer_count(64, 2, 128), 2);
        assert_eq!(super::frame_buffer_count(64, 4096, 64 * 4096), 3);
        assert_eq!(super::frame_buffer_count(64, 4096, 31), 1);
    }

    #[test]
    fn kill_failure_policy_retries_without_caller_side_blocking_wait() {
        let expected_exit = observed_exit(0);
        let mut child = ScriptedChild {
            polls: VecDeque::from([Ok(None), Ok(Some(expected_exit))]),
            kills: VecDeque::from([Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "scripted kill failure",
            ))]),
            ..ScriptedChild::default()
        };
        let mut status = None;
        let mut kill_requested = false;
        let mut last_error = None;

        assert_eq!(
            super::poll_termination(
                &mut child,
                &mut status,
                &mut kill_requested,
                &mut last_error,
            ),
            TerminationPoll::Pending
        );
        assert!(!kill_requested);
        assert_eq!(child.kill_calls, 1);
        assert_eq!(child.wait_calls, 0);
        assert!(last_error
            .as_deref()
            .is_some_and(|error| error.contains("scripted kill failure")));

        assert_eq!(
            super::poll_termination(
                &mut child,
                &mut status,
                &mut kill_requested,
                &mut last_error,
            ),
            TerminationPoll::Reaped(expected_exit)
        );
        assert_eq!(status, Some(expected_exit));
        assert_eq!(child.wait_calls, 0);
    }

    #[test]
    fn caller_cleanup_is_bounded_and_only_deferred_cleanup_may_block_waiting() {
        let source = include_str!("process_runner.rs");
        let caller_start = source
            .find("fn cleanup_child_and_readers")
            .expect("caller cleanup source");
        let caller_end = source[caller_start..]
            .find("fn captured_guard")
            .map(|offset| caller_start + offset)
            .expect("caller cleanup end");
        let caller = &source[caller_start..caller_end];
        let deferred_start = source
            .find("fn run_deferred_cleanup")
            .expect("deferred cleanup source");
        let deferred_end = source[deferred_start..]
            .find("fn cleanup_child_and_readers")
            .map(|offset| deferred_start + offset)
            .expect("deferred cleanup end");
        let deferred = &source[deferred_start..deferred_end];
        let blocking_wait = [".", "wait_exit("].concat();

        assert_eq!(super::CALLER_CLEANUP_GRACE, Duration::from_millis(500));
        assert!(!caller.contains(&blocking_wait));
        assert_eq!(deferred.matches(&blocking_wait).count(), 1);
        assert!(deferred.contains("stickerfit-deferred-process-cleanup"));
        assert!(caller.contains("is_finished()"));
    }

    #[test]
    fn deferred_readers_become_an_io_primary_without_replacing_existing_errors() {
        assert_eq!(
            super::finish_reader_cleanup(None, true),
            Some(PipelineError::Io {
                operation: "finish child process readers",
                message: "reader threads exceeded the caller cleanup grace period".into(),
            })
        );
        assert_eq!(super::finish_reader_cleanup(None, false), None);

        let existing_primaries = [
            PipelineError::Cancelled,
            PipelineError::TimedOut {
                stage: "child-process",
            },
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 192,
                actual: 256,
            },
            PipelineError::Io {
                operation: "stream frame callback",
                message: "callback rejected frame".into(),
            },
        ];
        for expected in existing_primaries {
            assert_eq!(
                super::finish_reader_cleanup(Some(expected.clone()), true),
                Some(expected)
            );
        }
    }

    #[test]
    fn nonzero_latch_prevents_callback_and_is_not_replaced_by_later_signals() {
        let exit = observed_exit(17);
        let mut pending_nonzero = None;
        let mut callbacks = 0;

        let must_stop = super::latch_nonzero(Some(exit), &mut pending_nonzero);
        if !must_stop {
            callbacks += 1;
        }
        let later_context = OperationContext::detached(Duration::ZERO);
        later_context.cancel();
        let later_timeout = OperationContext::detached(Duration::ZERO);

        assert_eq!(later_context.checkpoint(), Err(PipelineError::Cancelled));
        assert!(matches!(
            later_timeout.checkpoint(),
            Err(PipelineError::TimedOut { stage: "operation" })
        ));
        assert!(must_stop);
        assert_eq!(callbacks, 0);
        assert_eq!(pending_nonzero, Some(exit));
    }

    #[test]
    fn captured_source_breaks_immediately_after_nonzero_latch() {
        let source = include_str!("process_runner.rs");
        let start = source
            .find("pub(crate) fn run_captured")
            .expect("captured runner source");
        let end = source[start..]
            .find("enum PreparedStreamEvent")
            .map(|offset| start + offset)
            .expect("captured runner end");
        let captured = &source[start..end];
        let poll = captured
            .find("poll_child_status(")
            .expect("captured status poll");
        let latch = captured[poll..]
            .find("latch_nonzero(")
            .map(|offset| poll + offset)
            .expect("captured nonzero latch");
        let cleanup = captured[latch..]
            .find("cleanup_child_and_readers(")
            .map(|offset| latch + offset)
            .expect("captured cleanup after latch");

        assert!(poll < latch);
        assert!(captured[latch..cleanup].contains("break;"));
        assert!(!captured[latch..cleanup].contains("context.checkpoint()"));
    }

    #[test]
    fn streaming_frame_cap_is_prepared_before_nonzero_status_can_latch() {
        let error = super::prepare_stream_event(
            Some(super::FrameEvent::Frame(vec![0_u8; 64])),
            3,
            192,
            3,
            192,
        )
        .expect_err("fourth frame must be rejected before status handling");

        assert_eq!(
            error,
            PipelineError::LimitExceeded {
                resource: "decoded-bytes",
                limit: 192,
                actual: 256,
            }
        );
    }

    #[test]
    fn streaming_source_prepares_guards_before_polling_status_and_callbacks() {
        let source = include_str!("process_runner.rs");
        let start = source
            .find("pub(crate) fn stream_fixed_rgba_frames")
            .expect("streaming runner source");
        let end = source[start..]
            .find("#[cfg(test)]")
            .map(|offset| start + offset)
            .expect("streaming runner end");
        let streaming = &source[start..end];
        let prepare = streaming
            .find("prepare_stream_event(")
            .expect("prepared event call");
        let status = streaming[prepare..]
            .find("poll_child_status(")
            .map(|offset| prepare + offset)
            .expect("status poll after prepared event");
        let callback = streaming[status..]
            .find("on_frame(")
            .map(|offset| status + offset)
            .expect("callback after status poll");

        assert!(prepare < status);
        assert!(status < callback);
        assert!(streaming[status..callback].contains("latch_nonzero("));
    }

    #[test]
    fn recycling_after_normal_reader_terminal_is_a_no_op() {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(2);
        drop(receiver);
        let context = OperationContext::detached(Duration::from_secs(1));

        assert_eq!(
            super::recycle_frame(
                &sender,
                Vec::with_capacity(64),
                Instant::now(),
                Duration::from_secs(1),
                &context,
            ),
            Ok(())
        );
    }

    #[test]
    fn cancellation_wins_when_cancellation_and_deadline_are_both_visible() {
        let context = OperationContext::detached(Duration::from_millis(1));
        thread::sleep(Duration::from_millis(10));
        context.cancel();

        assert_eq!(
            run_captured(
                OsStr::new("stickerfit-process-runner-must-not-spawn"),
                &[],
                capture_limits(Duration::from_secs(1), 16, 16),
                &context,
            ),
            Err(PipelineError::Cancelled)
        );
    }

    #[test]
    fn fixture_protocol_uses_one_exact_argument_vector_for_every_child() {
        static EXPECTED: OnceLock<Vec<OsString>> = OnceLock::new();
        let expected = EXPECTED.get_or_init(fixture_args);
        assert_eq!(fixture_args(), *expected);
        assert_eq!(
            expected,
            &[
                OsString::from("--ignored"),
                OsString::from("--exact"),
                OsString::from(FIXTURE_ENTRY),
                OsString::from("--test-threads=1"),
                OsString::from("--color"),
                OsString::from("never"),
                OsString::from("--nocapture"),
                OsString::from("--skip"),
                OsString::from(UNICODE_SKIP_TOKEN),
            ]
        );
    }
}
