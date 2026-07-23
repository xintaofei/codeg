use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sacp::schema::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, TerminalExitStatus, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

type TerminalMap = HashMap<String, Arc<TerminalInstance>>;
const DEFAULT_OUTPUT_BYTE_LIMIT: u64 = 1_000_000;
/// After the child process exits, wait up to this long for the stdout/stderr
/// reader tasks to drain naturally before aborting them. Needed because a
/// grandchild process (e.g. Node spawned from a `.cmd` shim on Windows) can
/// inherit the pipe handle and keep it open long after the direct child
/// exits, turning `wait_for_exit` into a silent hang.
const READER_DRAIN_GRACE: Duration = Duration::from_millis(200);
const CHILD_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(unix)]
const SIGINT_GRACE: Duration = Duration::from_millis(500);
#[cfg(unix)]
const SIGTERM_GRACE: Duration = Duration::from_millis(1_500);
#[cfg(unix)]
const SIGKILL_GRACE: Duration = Duration::from_millis(2_000);
#[cfg(unix)]
const DESCENDANT_LEASE_DURATION: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum TerminalRuntimeError {
    InvalidParams(String),
    Internal(String),
}

impl TerminalRuntimeError {
    pub fn into_rpc_error(self) -> sacp::Error {
        match self {
            Self::InvalidParams(message) => sacp::Error::invalid_params().data(message),
            Self::Internal(message) => sacp::util::internal_error(message),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TerminalCleanupReport {
    failures: Vec<(String, String)>,
}

impl TerminalCleanupReport {
    pub(crate) fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    pub(crate) fn failure_count(&self) -> usize {
        self.failures.len()
    }
}

#[derive(Debug, Default, Clone)]
struct TerminalSnapshot {
    output: String,
    output_base_offset: u64,
    truncated: bool,
    exit_status: Option<TerminalExitStatus>,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessGroupKey {
    pgid: libc::pid_t,
    generation: u64,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessGroupPresence {
    Present,
    Missing,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessGroupSignalResult {
    Delivered,
    Missing,
}

#[cfg(unix)]
trait UnixProcessGroupBackend: Send + Sync {
    fn probe(&self, key: ProcessGroupKey) -> Result<ProcessGroupPresence, TerminalRuntimeError>;

    fn signal(
        &self,
        key: ProcessGroupKey,
        signal: libc::c_int,
    ) -> Result<ProcessGroupSignalResult, TerminalRuntimeError>;
}

#[cfg(unix)]
struct LibcProcessGroupBackend;

#[cfg(unix)]
impl UnixProcessGroupBackend for LibcProcessGroupBackend {
    fn probe(&self, key: ProcessGroupKey) -> Result<ProcessGroupPresence, TerminalRuntimeError> {
        let result = unsafe { libc::kill(-key.pgid, 0) };
        if result == 0 {
            return Ok(ProcessGroupPresence::Present);
        }

        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ESRCH) => Ok(ProcessGroupPresence::Missing),
            Some(libc::EPERM) => Ok(ProcessGroupPresence::Present),
            _ => Err(TerminalRuntimeError::Internal(format!(
                "failed to query terminal process group pgid={} generation={}: {err}",
                key.pgid, key.generation
            ))),
        }
    }

    fn signal(
        &self,
        key: ProcessGroupKey,
        signal: libc::c_int,
    ) -> Result<ProcessGroupSignalResult, TerminalRuntimeError> {
        let result = unsafe { libc::kill(-key.pgid, signal) };
        if result == 0 {
            return Ok(ProcessGroupSignalResult::Delivered);
        }

        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(ProcessGroupSignalResult::Missing);
        }
        Err(TerminalRuntimeError::Internal(format!(
            "failed to signal terminal process group pgid={} generation={} signal={signal}: {err}",
            key.pgid, key.generation
        )))
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnixProcessGroupState {
    OwnedLeaderAlive {
        key: ProcessGroupKey,
    },
    OwnedDescendants {
        key: ProcessGroupKey,
        deadline: tokio::time::Instant,
    },
    Retired,
}

#[cfg(unix)]
struct UnixProcessGroupLease {
    terminal_id: String,
    session_id: String,
    pid: u32,
    backend: Arc<dyn UnixProcessGroupBackend>,
    state: Mutex<UnixProcessGroupState>,
    cleanup_gate: Mutex<()>,
}

#[cfg(unix)]
impl UnixProcessGroupLease {
    fn new(
        terminal_id: String,
        session_id: String,
        pid: u32,
        generation: u64,
        backend: Arc<dyn UnixProcessGroupBackend>,
    ) -> Result<Self, TerminalRuntimeError> {
        let pgid = libc::pid_t::try_from(pid).map_err(|_| {
            TerminalRuntimeError::Internal(format!("terminal pid {pid} does not fit pid_t"))
        })?;
        Ok(Self {
            terminal_id,
            session_id,
            pid,
            backend,
            state: Mutex::new(UnixProcessGroupState::OwnedLeaderAlive {
                key: ProcessGroupKey { pgid, generation },
            }),
            cleanup_gate: Mutex::new(()),
        })
    }

    async fn observe_leader_exit(
        &self,
        observed_at: tokio::time::Instant,
    ) -> Result<(), TerminalRuntimeError> {
        let mut state = self.state.lock().await;
        let UnixProcessGroupState::OwnedLeaderAlive { key } = *state else {
            return Ok(());
        };

        match self.backend.probe(key) {
            Ok(ProcessGroupPresence::Missing) => {
                *state = UnixProcessGroupState::Retired;
                tracing::info!(
                    terminal_id = %self.terminal_id,
                    session_id = %self.session_id,
                    pid = self.pid,
                    pgid = key.pgid,
                    generation = key.generation,
                    "[ACP] retired terminal process-group lease after leader exit"
                );
                Ok(())
            }
            Ok(ProcessGroupPresence::Present) => {
                let deadline = observed_at + DESCENDANT_LEASE_DURATION;
                *state = UnixProcessGroupState::OwnedDescendants { key, deadline };
                tracing::info!(
                    terminal_id = %self.terminal_id,
                    session_id = %self.session_id,
                    pid = self.pid,
                    pgid = key.pgid,
                    generation = key.generation,
                    lease_ms = DESCENDANT_LEASE_DURATION.as_millis(),
                    "[ACP] started bounded descendant process-group lease"
                );
                Ok(())
            }
            Err(err) => {
                *state = UnixProcessGroupState::Retired;
                Err(err)
            }
        }
    }

    fn active_key(
        state: &mut UnixProcessGroupState,
        now: tokio::time::Instant,
    ) -> Option<(ProcessGroupKey, Option<tokio::time::Instant>)> {
        match *state {
            UnixProcessGroupState::OwnedLeaderAlive { key } => Some((key, None)),
            UnixProcessGroupState::OwnedDescendants { key, deadline } if now < deadline => {
                Some((key, Some(deadline)))
            }
            UnixProcessGroupState::OwnedDescendants { .. } => {
                *state = UnixProcessGroupState::Retired;
                None
            }
            UnixProcessGroupState::Retired => None,
        }
    }

    async fn signal_active(
        &self,
        signal: libc::c_int,
        stage: &'static str,
    ) -> Result<Option<(ProcessGroupKey, Option<tokio::time::Instant>)>, TerminalRuntimeError> {
        let mut state = self.state.lock().await;
        let Some((key, deadline)) = Self::active_key(&mut state, tokio::time::Instant::now())
        else {
            return Ok(None);
        };

        // A live leader anchors its PID/PGID, but descendants only have a
        // bounded numeric-PGID lease. Revalidate that lease immediately before
        // every signal so an already-disappeared group is retired instead of
        // receiving a signal after a delayed release. POSIX still permits the
        // final probe-to-signal syscall race; both calls stay under the same
        // terminal state lock to avoid a wider in-process race.
        if deadline.is_some() {
            match self.backend.probe(key) {
                Ok(ProcessGroupPresence::Present) => {}
                Ok(ProcessGroupPresence::Missing) => {
                    *state = UnixProcessGroupState::Retired;
                    return Ok(None);
                }
                Err(err) => {
                    *state = UnixProcessGroupState::Retired;
                    return Err(err);
                }
            }

            // A synchronous backend can itself consume the final lease time.
            // Expiry is absolute and non-renewable, so do not let a successful
            // probe grant time for a later signal.
            if Self::active_key(&mut state, tokio::time::Instant::now()).is_none() {
                return Ok(None);
            }
        }

        match self.backend.signal(key, signal) {
            Ok(ProcessGroupSignalResult::Delivered) => {
                tracing::info!(
                    terminal_id = %self.terminal_id,
                    session_id = %self.session_id,
                    pid = self.pid,
                    pgid = key.pgid,
                    generation = key.generation,
                    signal_stage = stage,
                    "[ACP] signalled terminal process group"
                );
                Ok(Some((key, deadline)))
            }
            Ok(ProcessGroupSignalResult::Missing) => {
                *state = UnixProcessGroupState::Retired;
                Ok(None)
            }
            Err(err) => {
                *state = UnixProcessGroupState::Retired;
                Err(err)
            }
        }
    }

    async fn probe_active(&self) -> Result<bool, TerminalRuntimeError> {
        let mut state = self.state.lock().await;
        let Some((key, _)) = Self::active_key(&mut state, tokio::time::Instant::now()) else {
            return Ok(false);
        };

        match self.backend.probe(key) {
            Ok(ProcessGroupPresence::Present) => Ok(true),
            Ok(ProcessGroupPresence::Missing) => {
                *state = UnixProcessGroupState::Retired;
                Ok(false)
            }
            Err(err) => {
                *state = UnixProcessGroupState::Retired;
                Err(err)
            }
        }
    }

    async fn retire(&self) {
        *self.state.lock().await = UnixProcessGroupState::Retired;
    }

    async fn cleanup_with_stages(
        &self,
        stages: &[(libc::c_int, &'static str, Duration)],
        terminal: Option<&TerminalInstance>,
    ) -> Result<(), TerminalRuntimeError> {
        let _cleanup_guard = self.cleanup_gate.lock().await;
        let started = std::time::Instant::now();
        let mut last_key = None;

        for &(signal, stage, grace) in stages {
            let Some((key, descendant_deadline)) = self.signal_active(signal, stage).await? else {
                return Ok(());
            };
            last_key = Some(key);

            let now = tokio::time::Instant::now();
            let stage_deadline = descendant_deadline
                .map(|lease_deadline| (now + grace).min(lease_deadline))
                .unwrap_or(now + grace);

            loop {
                if let Some(terminal) = terminal {
                    terminal.refresh_exit_status().await?;
                }

                let now = tokio::time::Instant::now();
                if !self.probe_active().await? {
                    tracing::info!(
                        terminal_id = %self.terminal_id,
                        session_id = %self.session_id,
                        pid = self.pid,
                        pgid = key.pgid,
                        generation = key.generation,
                        elapsed_ms = started.elapsed().as_millis(),
                        signal_stage = stage,
                        "[ACP] terminal process-group lease retired"
                    );
                    return Ok(());
                }

                if now >= stage_deadline {
                    break;
                }
                tokio::time::sleep(
                    CHILD_EXIT_POLL_INTERVAL.min(stage_deadline.saturating_duration_since(now)),
                )
                .await;
            }
        }

        self.retire().await;
        let key = last_key.expect("non-empty terminal signal stages");
        Err(TerminalRuntimeError::Internal(format!(
            "terminal process group pgid={} generation={} survived SIGKILL deadline",
            key.pgid, key.generation
        )))
    }

    async fn cleanup(&self, terminal: &TerminalInstance) -> Result<(), TerminalRuntimeError> {
        self.cleanup_with_stages(
            &[
                (libc::SIGINT, "sigint", SIGINT_GRACE),
                (libc::SIGTERM, "sigterm", SIGTERM_GRACE),
                (libc::SIGKILL, "sigkill", SIGKILL_GRACE),
            ],
            Some(terminal),
        )
        .await
    }
}

#[cfg(all(test, unix))]
#[derive(Clone)]
struct ExitObservationBarrier {
    reached: Arc<tokio::sync::Barrier>,
    resume: Arc<tokio::sync::Barrier>,
}

struct TerminalInstance {
    terminal_id: String,
    session_id: String,
    #[cfg(unix)]
    process_group: UnixProcessGroupLease,
    /// Serializes observing a direct-child exit with the corresponding Unix
    /// process-group lease transition. A cleanup must not see an absent child
    /// while the lease still describes a live leader.
    #[cfg(unix)]
    leader_exit_observation_gate: Mutex<()>,
    output_limit: Option<usize>,
    child: Mutex<Option<tokio::process::Child>>,
    snapshot: Mutex<TerminalSnapshot>,
    reader_handles: Mutex<Vec<JoinHandle<()>>>,
    #[cfg(all(test, unix))]
    exit_observation_barrier: std::sync::Mutex<Option<ExitObservationBarrier>>,
}

impl TerminalInstance {
    fn new(
        terminal_id: String,
        session_id: String,
        output_limit: Option<u64>,
        child: tokio::process::Child,
        #[cfg_attr(not(unix), allow(unused_variables))] pid: u32,
        #[cfg(unix)] generation: u64,
        #[cfg(unix)] process_group_backend: Arc<dyn UnixProcessGroupBackend>,
    ) -> Result<Self, TerminalRuntimeError> {
        #[cfg(unix)]
        let process_group = UnixProcessGroupLease::new(
            terminal_id.clone(),
            session_id.clone(),
            pid,
            generation,
            process_group_backend,
        )?;

        Ok(Self {
            terminal_id,
            session_id,
            #[cfg(unix)]
            process_group,
            #[cfg(unix)]
            leader_exit_observation_gate: Mutex::new(()),
            output_limit: output_limit.and_then(|v| usize::try_from(v).ok()),
            child: Mutex::new(Some(child)),
            snapshot: Mutex::new(TerminalSnapshot::default()),
            reader_handles: Mutex::new(Vec::new()),
            #[cfg(all(test, unix))]
            exit_observation_barrier: std::sync::Mutex::new(None),
        })
    }

    #[cfg(all(test, unix))]
    fn install_exit_observation_barrier(&self, barrier: ExitObservationBarrier) {
        *self
            .exit_observation_barrier
            .lock()
            .expect("exit observation barrier lock") = Some(barrier);
    }

    #[cfg(all(test, unix))]
    async fn pause_after_reap_before_lease_observation(&self) {
        let barrier = self
            .exit_observation_barrier
            .lock()
            .expect("exit observation barrier lock")
            .clone();
        if let Some(barrier) = barrier {
            barrier.reached.wait().await;
            barrier.resume.wait().await;
        }
    }

    /// Wait briefly for stdout/stderr reader tasks to finish; abort any that
    /// remain. Must be called after the direct child has already exited —
    /// otherwise we would abort readers that are still making progress.
    async fn drain_readers(&self) {
        let handles: Vec<JoinHandle<()>> = std::mem::take(&mut *self.reader_handles.lock().await);
        for handle in handles {
            let abort = handle.abort_handle();
            if tokio::time::timeout(READER_DRAIN_GRACE, handle)
                .await
                .is_err()
            {
                abort.abort();
            }
        }
    }

    async fn append_output(&self, text: &str) {
        let mut snapshot = self.snapshot.lock().await;
        snapshot.output.push_str(text);
        if let Some(limit) = self.output_limit {
            let removed = enforce_output_limit(&mut snapshot.output, limit);
            if removed > 0 {
                snapshot.truncated = true;
                snapshot.output_base_offset = snapshot
                    .output_base_offset
                    .saturating_add(u64::try_from(removed).unwrap_or(u64::MAX));
            }
        }
    }

    async fn refresh_exit_status(&self) -> Result<(), TerminalRuntimeError> {
        {
            let snapshot = self.snapshot.lock().await;
            if snapshot.exit_status.is_some() {
                return Ok(());
            }
        }

        #[cfg(unix)]
        let (maybe_status, process_group_result) = {
            let _observation_guard = self.leader_exit_observation_gate.lock().await;
            let mut child_guard = self.child.lock().await;
            if let Some(child) = child_guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Do not make the child absent visible until the group
                        // lease has either become bounded descendants or has
                        // retired. The child mutex is released before the
                        // async lease observation.
                        drop(child_guard);

                        #[cfg(all(test, unix))]
                        self.pause_after_reap_before_lease_observation().await;

                        let process_group_result = self
                            .process_group
                            .observe_leader_exit(tokio::time::Instant::now())
                            .await;

                        *self.child.lock().await = None;
                        (Some(status), process_group_result)
                    }
                    Ok(None) => (None, Ok(())),
                    Err(err) => {
                        return Err(TerminalRuntimeError::Internal(format!(
                            "failed to query terminal exit status: {err}"
                        )))
                    }
                }
            } else {
                (None, Ok(()))
            }
        };

        #[cfg(not(unix))]
        let maybe_status = {
            let mut child_guard = self.child.lock().await;
            if let Some(child) = child_guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *child_guard = None;
                        Some(status)
                    }
                    Ok(None) => None,
                    Err(err) => {
                        return Err(TerminalRuntimeError::Internal(format!(
                            "failed to query terminal exit status: {err}"
                        )))
                    }
                }
            } else {
                None
            }
        };

        if let Some(status) = maybe_status {
            // Drain readers BEFORE exposing exit_status. Otherwise a caller
            // polling `terminal/output` can see `exit_status = Some(...)` while
            // a grandchild process (e.g. Node spawned from a `.cmd` shim on
            // Windows) still holds the stdout/stderr pipe and is flushing
            // tail output. If the agent treats exit_status as "terminal done",
            // the trailing bytes never reach the UI. Draining here upholds the
            // invariant: whenever an external observer sees exit_status, the
            // snapshot already contains (or has explicitly given up on) all
            // reader output.
            self.drain_readers().await;
            let mut snapshot = self.snapshot.lock().await;
            snapshot.exit_status = Some(map_exit_status(status));

            #[cfg(unix)]
            process_group_result?;
        }

        Ok(())
    }

    async fn wait_for_exit(&self) -> Result<TerminalExitStatus, TerminalRuntimeError> {
        loop {
            self.refresh_exit_status().await?;
            if let Some(exit_status) = self.snapshot.lock().await.exit_status.clone() {
                return Ok(exit_status);
            }
            tokio::time::sleep(CHILD_EXIT_POLL_INTERVAL).await;
        }
    }

    #[cfg(unix)]
    async fn kill_command(&self) -> Result<(), TerminalRuntimeError> {
        self.refresh_exit_status().await?;
        let cleanup_result = self.process_group.cleanup(self).await;
        let refresh_result = self.refresh_exit_status().await;
        cleanup_result.and(refresh_result)
    }

    #[cfg(not(unix))]
    async fn kill_command(&self) -> Result<(), TerminalRuntimeError> {
        self.refresh_exit_status().await?;
        if self.snapshot.lock().await.exit_status.is_some() {
            return Ok(());
        }

        let pid = self
            .child
            .lock()
            .await
            .as_ref()
            .and_then(tokio::process::Child::id);
        if let Some(pid) = pid {
            if let Err(err) = kill_tree::tokio::kill_tree(pid).await {
                tracing::error!("[ACP] kill_tree failed for pid {pid}: {err}");
            }
        }
        self.wait_for_exit().await.map(|_| ())
    }

    async fn snapshot(&self) -> TerminalSnapshot {
        self.snapshot.lock().await.clone()
    }
}

pub struct TerminalRuntime {
    terminals: Mutex<TerminalMap>,
    #[cfg(unix)]
    process_group_backend: Arc<dyn UnixProcessGroupBackend>,
    #[cfg(unix)]
    next_process_group_generation: AtomicU64,
    /// Base environment merged into every spawned terminal command before
    /// the agent's per-request `env` is applied. This is where the codeg
    /// git credential helper (`GIT_CONFIG_*`) lives so an agent that runs
    /// `git push` via the ACP `terminal/create` tool inherits the same
    /// auth path the agent process itself does. Per-request env from the
    /// agent overrides on key collision so an agent can still scrub or
    /// override anything explicitly.
    base_env: BTreeMap<String, String>,
    /// Fallback working directory applied to spawned terminals when the
    /// agent's `terminal/create` request omits `cwd`. The connection layer
    /// sets this to the session's resolved working directory so terminals
    /// default to the folder the conversation runs in instead of codeg's own
    /// process cwd (often "/" on desktop, the dev crate dir in development).
    /// `None` leaves the process cwd inherited (legacy behavior).
    default_cwd: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TerminalOutputDelta {
    pub output: String,
    pub next_offset: u64,
    pub had_gap: bool,
    pub truncated: bool,
    pub exit_status: Option<TerminalExitStatus>,
}

impl TerminalRuntime {
    /// Construct a runtime where every spawned command starts with `base_env`
    /// applied, before the agent's per-request env overrides are layered on
    /// top. Use this to propagate process-level invariants like the git
    /// credential helper across `terminal/create` invocations.
    pub fn with_base_env(base_env: BTreeMap<String, String>) -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
            #[cfg(unix)]
            process_group_backend: Arc::new(LibcProcessGroupBackend),
            #[cfg(unix)]
            next_process_group_generation: AtomicU64::new(1),
            base_env,
            default_cwd: None,
        }
    }

    #[cfg(unix)]
    fn allocate_process_group_generation(&self) -> Result<u64, TerminalRuntimeError> {
        self.next_process_group_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
                generation.checked_add(1)
            })
            .map_err(|_| {
                TerminalRuntimeError::Internal(
                    "terminal process-group generation space exhausted".to_string(),
                )
            })
    }

    /// Set the fallback working directory used when a `terminal/create` request
    /// does not specify its own `cwd`. Chainable after `with_base_env`.
    pub fn with_default_cwd(mut self, default_cwd: Option<PathBuf>) -> Self {
        self.default_cwd = default_cwd;
        self
    }

    #[cfg(all(test, unix))]
    fn with_process_group_backend(mut self, backend: Arc<dyn UnixProcessGroupBackend>) -> Self {
        self.process_group_backend = backend;
        self
    }

    /// Apply stdio, working directory, and environment to a freshly built
    /// terminal command. Shared by the direct-exec and shell-fallback spawn
    /// paths in `create_terminal` so both honor the same cwd precedence and
    /// env layering.
    fn configure_command(
        &self,
        command: &mut tokio::process::Command,
        request: &CreateTerminalRequest,
    ) {
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // Working directory. An explicit `cwd` from the agent (validated
        // absolute in `create_terminal`) is honored as-is, so a non-existent
        // directory surfaces as a loud spawn failure rather than silently
        // running somewhere else. Only when the agent omits `cwd` do we fall
        // back to the connection's session working directory — agents like
        // CodeBuddy omit it, which would otherwise inherit codeg's own process
        // cwd instead of the folder the conversation runs in. The fallback is
        // guarded on `is_dir` so a not-yet-created session dir never turns into
        // a spawn failure (mirrors the cwd guard in `build_agent`).
        if let Some(cwd) = request.cwd.as_deref() {
            command.current_dir(cwd);
        } else if let Some(default_cwd) = self.default_cwd.as_deref() {
            if default_cwd.is_dir() {
                command.current_dir(default_cwd);
            }
        }

        // Apply the runtime's base env first (e.g. `GIT_CONFIG_*` for the
        // codeg credential helper), then layer the agent's request env on top
        // so agents can still override or scrub specific keys.
        for (key, value) in &self.base_env {
            command.env(key, value);
        }
        for env_var in &request.env {
            command.env(&env_var.name, &env_var.value);
        }

        #[cfg(unix)]
        {
            command.process_group(0);
        }
    }

    pub async fn create_terminal(
        &self,
        request: CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, TerminalRuntimeError> {
        if let Some(cwd) = request.cwd.as_ref() {
            if !cwd.is_absolute() {
                return Err(TerminalRuntimeError::InvalidParams(
                    "terminal/create requires an absolute cwd when provided".to_string(),
                ));
            }
        }

        if request.command.trim().is_empty() {
            return Err(TerminalRuntimeError::InvalidParams(
                "terminal/create requires a non-empty command".to_string(),
            ));
        }

        let output_byte_limit = request
            .output_byte_limit
            .unwrap_or(DEFAULT_OUTPUT_BYTE_LIMIT);
        if output_byte_limit == 0 {
            return Err(TerminalRuntimeError::InvalidParams(
                "terminal/create outputByteLimit must be greater than 0".to_string(),
            ));
        }

        #[cfg(unix)]
        let process_group_generation = self.allocate_process_group_generation()?;

        // Spawn the command. Try a direct exec first so a real program — one
        // resolved on PATH, an absolute path, or a relative/space-containing
        // path reachable through the request's cwd and env — runs exactly as
        // before, in the real spawn context. Only if the OS cannot find the
        // program (`NotFound`) AND the request looks like a whole shell line
        // crammed into `command` (empty args + embedded whitespace, the shape
        // CodeBuddy sends, e.g. "pnpm build") do we retry through the platform
        // shell so its `&&`, pipes, `$VAR`, and globs evaluate. Deciding off a
        // real failed spawn — rather than a pre-spawn `which` guess that runs
        // in codeg's own cwd/env — means we never reroute a command that would
        // otherwise have run.
        let mut direct = crate::process::tokio_command(&request.command);
        direct.args(&request.args);
        self.configure_command(&mut direct, &request);

        let mut child = match direct.spawn() {
            Ok(child) => child,
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && request.args.is_empty()
                    && request.command.contains(char::is_whitespace) =>
            {
                let mut shell = shell_wrapped_command(&request.command);
                self.configure_command(&mut shell, &request);
                shell.spawn().map_err(|err| {
                    TerminalRuntimeError::Internal(format!(
                        "failed to spawn terminal command {}: {err}",
                        request.command
                    ))
                })?
            }
            Err(err) => {
                return Err(TerminalRuntimeError::Internal(format!(
                    "failed to spawn terminal command {}: {err}",
                    request.command
                )));
            }
        };

        let pid = child.id().ok_or_else(|| {
            TerminalRuntimeError::Internal("spawned terminal has no process id".to_string())
        })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let terminal_id = format!("term_{}", uuid::Uuid::new_v4().simple());
        let terminal = Arc::new(TerminalInstance::new(
            terminal_id.clone(),
            request.session_id.to_string(),
            Some(output_byte_limit),
            child,
            pid,
            #[cfg(unix)]
            process_group_generation,
            #[cfg(unix)]
            Arc::clone(&self.process_group_backend),
        )?);

        #[cfg(unix)]
        tracing::info!(
            terminal_id = %terminal_id,
            session_id = %request.session_id,
            pid,
            pgid = pid,
            generation = process_group_generation,
            "[ACP] spawned isolated terminal process group"
        );

        let mut handles: Vec<JoinHandle<()>> = Vec::new();
        if let Some(reader) = stdout {
            let terminal_ref = terminal.clone();
            handles.push(tokio::spawn(async move {
                read_stream(reader, terminal_ref).await;
            }));
        }

        if let Some(reader) = stderr {
            let terminal_ref = terminal.clone();
            handles.push(tokio::spawn(async move {
                read_stream(reader, terminal_ref).await;
            }));
        }

        if !handles.is_empty() {
            terminal.reader_handles.lock().await.extend(handles);
        }

        self.terminals
            .lock()
            .await
            .insert(terminal_id.clone(), terminal);

        Ok(CreateTerminalResponse::new(terminal_id))
    }

    pub async fn terminal_output(
        &self,
        request: TerminalOutputRequest,
    ) -> Result<TerminalOutputResponse, TerminalRuntimeError> {
        let terminal = self
            .find_terminal(
                &request.terminal_id.to_string(),
                &request.session_id.to_string(),
            )
            .await?;

        terminal.refresh_exit_status().await?;
        let snapshot = terminal.snapshot().await;

        Ok(
            TerminalOutputResponse::new(snapshot.output, snapshot.truncated)
                .exit_status(snapshot.exit_status),
        )
    }

    pub async fn terminal_output_delta(
        &self,
        session_id: &str,
        terminal_id: &str,
        from_offset: Option<u64>,
    ) -> Result<TerminalOutputDelta, TerminalRuntimeError> {
        let terminal = self.find_terminal(terminal_id, session_id).await?;
        terminal.refresh_exit_status().await?;
        let snapshot = terminal.snapshot().await;

        let output_len = u64::try_from(snapshot.output.len()).unwrap_or(u64::MAX);
        let base_offset = snapshot.output_base_offset;
        let end_offset = base_offset.saturating_add(output_len);
        let requested_offset = from_offset.unwrap_or(base_offset);
        let had_gap = from_offset
            .map(|offset| offset < base_offset)
            .unwrap_or(false);
        let start_offset = requested_offset.clamp(base_offset, end_offset);
        let start_index = usize::try_from(start_offset.saturating_sub(base_offset)).unwrap_or(0);
        let output = snapshot.output[start_index..].to_string();

        Ok(TerminalOutputDelta {
            output,
            next_offset: end_offset,
            had_gap,
            truncated: snapshot.truncated,
            exit_status: snapshot.exit_status,
        })
    }

    pub async fn wait_for_terminal_exit(
        &self,
        request: WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse, TerminalRuntimeError> {
        let terminal = self
            .find_terminal(
                &request.terminal_id.to_string(),
                &request.session_id.to_string(),
            )
            .await?;
        let exit_status = terminal.wait_for_exit().await?;
        Ok(WaitForTerminalExitResponse::new(exit_status))
    }

    pub async fn kill_terminal(
        &self,
        request: KillTerminalRequest,
    ) -> Result<KillTerminalResponse, TerminalRuntimeError> {
        let terminal = self
            .find_terminal(
                &request.terminal_id.to_string(),
                &request.session_id.to_string(),
            )
            .await?;
        terminal.kill_command().await?;
        Ok(KillTerminalResponse::new())
    }

    pub async fn release_terminal(
        &self,
        request: ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse, TerminalRuntimeError> {
        let terminal_id = request.terminal_id.to_string();
        let session_id = request.session_id.to_string();
        let terminal = {
            let mut terminals = self.terminals.lock().await;
            let Some(existing) = terminals.get(&terminal_id) else {
                return Err(TerminalRuntimeError::InvalidParams(format!(
                    "terminal {terminal_id} not found"
                )));
            };
            if existing.session_id != session_id {
                return Err(TerminalRuntimeError::InvalidParams(format!(
                    "terminal {terminal_id} does not belong to session {session_id}"
                )));
            }
            terminals.remove(&terminal_id).expect("terminal exists")
        };

        terminal.kill_command().await?;
        Ok(ReleaseTerminalResponse::new())
    }

    pub(crate) async fn release_all_for_session(&self, session_id: &str) -> TerminalCleanupReport {
        let removed = {
            let mut terminals = self.terminals.lock().await;
            let ids: Vec<String> = terminals
                .iter()
                .filter(|(_, term)| term.session_id == session_id)
                .map(|(id, _)| id.clone())
                .collect();

            let mut removed = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(term) = terminals.remove(&id) {
                    removed.push(term);
                }
            }
            removed
        };

        let results = futures::future::join_all(removed.into_iter().map(|terminal| async move {
            let terminal_id = terminal.terminal_id.clone();
            (terminal_id, terminal.kill_command().await)
        }))
        .await;

        let failures = results
            .into_iter()
            .filter_map(|(terminal_id, result)| {
                result.err().map(|err| {
                    tracing::error!(
                        terminal_id = %terminal_id,
                        session_id,
                        error = ?err,
                        "[ACP] terminal session cleanup failed"
                    );
                    (terminal_id, format!("{err:?}"))
                })
            })
            .collect();

        TerminalCleanupReport { failures }
    }

    async fn find_terminal(
        &self,
        terminal_id: &str,
        session_id: &str,
    ) -> Result<Arc<TerminalInstance>, TerminalRuntimeError> {
        let terminal = {
            let terminals = self.terminals.lock().await;
            terminals.get(terminal_id).cloned()
        }
        .ok_or_else(|| {
            TerminalRuntimeError::InvalidParams(format!("terminal {terminal_id} not found"))
        })?;

        if terminal.session_id != session_id {
            return Err(TerminalRuntimeError::InvalidParams(format!(
                "terminal {terminal_id} does not belong to session {session_id}"
            )));
        }

        Ok(terminal)
    }
}

async fn read_stream<R>(mut reader: R, terminal: Arc<TerminalInstance>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 4096];
    let mut pending = Vec::<u8>::new();
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                if !pending.is_empty() {
                    let text = String::from_utf8_lossy(&pending).to_string();
                    terminal.append_output(&text).await;
                    pending.clear();
                }
                break;
            }
            Ok(size) => {
                pending.extend_from_slice(&buffer[..size]);
                let decoded = decode_available_utf8(&mut pending);
                if !decoded.is_empty() {
                    terminal.append_output(&decoded).await;
                }
            }
            Err(_) => break,
        }
    }
}

/// Wrap a full shell command line so it executes through the platform shell.
/// Used when an agent passes an entire command line in `command` with empty
/// `args` (see `create_terminal`); the shell preserves the `&&`, pipes,
/// `$VAR`, and globs the agent's line relies on. Reuses `tokio_command` so the
/// shell still inherits codeg's UTF-8 env and Windows program normalization.
#[cfg(not(windows))]
fn shell_wrapped_command(line: &str) -> tokio::process::Command {
    let mut command = crate::process::tokio_command("/bin/sh");
    command.arg("-c").arg(line);
    command
}

#[cfg(windows)]
fn shell_wrapped_command(line: &str) -> tokio::process::Command {
    let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    let mut command = crate::process::tokio_command(comspec);
    command.arg("/C").arg(line);
    command
}

fn map_exit_status(status: std::process::ExitStatus) -> TerminalExitStatus {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status).map(|s| s.to_string());
    #[cfg(not(unix))]
    let signal: Option<String> = None;

    let exit_code = status.code().and_then(|code| u32::try_from(code).ok());
    TerminalExitStatus::new()
        .exit_code(exit_code)
        .signal(signal)
}

fn enforce_output_limit(output: &mut String, limit: usize) -> usize {
    if output.len() <= limit {
        return 0;
    }

    let mut start = output.len().saturating_sub(limit);
    while start < output.len() && !output.is_char_boundary(start) {
        start += 1;
    }

    output.drain(..start);
    start
}

fn decode_available_utf8(pending: &mut Vec<u8>) -> String {
    let mut output = String::new();
    let mut consumed = 0usize;
    let mut remaining = pending.as_slice();

    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(text) => {
                output.push_str(text);
                consumed = consumed.saturating_add(remaining.len());
                break;
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0 {
                    if let Ok(text) = std::str::from_utf8(&remaining[..valid_up_to]) {
                        output.push_str(text);
                    }
                    consumed = consumed.saturating_add(valid_up_to);
                    remaining = &remaining[valid_up_to..];
                }

                match err.error_len() {
                    Some(invalid_len) => {
                        output.push_str(&String::from_utf8_lossy(&remaining[..invalid_len]));
                        consumed = consumed.saturating_add(invalid_len);
                        remaining = &remaining[invalid_len..];
                    }
                    None => break, // keep partial UTF-8 sequence for next chunk
                }
            }
        }
    }

    if consumed > 0 {
        pending.drain(..consumed);
    }
    output
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Instant;

    use kill_tree::Config;
    use sacp::schema::{EnvVariable, SessionId, WaitForTerminalExitRequest};

    fn init_test_tracing() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum BackendCall {
        Probe(ProcessGroupKey),
        Signal(ProcessGroupKey, libc::c_int),
    }

    #[derive(Default)]
    struct MockProcessGroupBackend {
        calls: std::sync::Mutex<Vec<BackendCall>>,
        probes: std::sync::Mutex<VecDeque<ProcessGroupPresence>>,
        signals: std::sync::Mutex<VecDeque<ProcessGroupSignalResult>>,
        probe_delay: Option<Duration>,
        call_tx: Option<tokio::sync::mpsc::UnboundedSender<BackendCall>>,
    }

    impl MockProcessGroupBackend {
        fn with_probes(probes: impl IntoIterator<Item = ProcessGroupPresence>) -> Self {
            Self {
                probes: std::sync::Mutex::new(probes.into_iter().collect()),
                ..Self::default()
            }
        }

        fn with_signals(signals: impl IntoIterator<Item = ProcessGroupSignalResult>) -> Self {
            Self {
                signals: std::sync::Mutex::new(signals.into_iter().collect()),
                ..Self::default()
            }
        }

        fn with_probe_delay(
            probes: impl IntoIterator<Item = ProcessGroupPresence>,
            probe_delay: Duration,
        ) -> Self {
            Self {
                probes: std::sync::Mutex::new(probes.into_iter().collect()),
                probe_delay: Some(probe_delay),
                ..Self::default()
            }
        }

        fn with_call_tx(
            mut self,
            call_tx: tokio::sync::mpsc::UnboundedSender<BackendCall>,
        ) -> Self {
            self.call_tx = Some(call_tx);
            self
        }

        fn record_call(&self, call: BackendCall) {
            self.calls
                .lock()
                .expect("mock calls lock")
                .push(call.clone());
            if let Some(call_tx) = &self.call_tx {
                let _ = call_tx.send(call);
            }
        }

        fn calls(&self) -> Vec<BackendCall> {
            self.calls.lock().expect("mock calls lock").clone()
        }

        fn delivered_signals(&self) -> Vec<libc::c_int> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    BackendCall::Signal(_, signal) => Some(signal),
                    BackendCall::Probe(_) => None,
                })
                .collect()
        }

        fn clear_calls(&self) {
            self.calls.lock().expect("mock calls lock").clear();
        }

        fn push_probe(&self, presence: ProcessGroupPresence) {
            self.probes
                .lock()
                .expect("mock probes lock")
                .push_back(presence);
        }
    }

    impl UnixProcessGroupBackend for MockProcessGroupBackend {
        fn probe(
            &self,
            key: ProcessGroupKey,
        ) -> Result<ProcessGroupPresence, TerminalRuntimeError> {
            self.record_call(BackendCall::Probe(key));
            let result = self
                .probes
                .lock()
                .expect("mock probes lock")
                .pop_front()
                .unwrap_or(ProcessGroupPresence::Present);
            if let Some(delay) = self.probe_delay {
                std::thread::sleep(delay);
            }
            Ok(result)
        }

        fn signal(
            &self,
            key: ProcessGroupKey,
            signal: libc::c_int,
        ) -> Result<ProcessGroupSignalResult, TerminalRuntimeError> {
            self.record_call(BackendCall::Signal(key, signal));
            Ok(self
                .signals
                .lock()
                .expect("mock signals lock")
                .pop_front()
                .unwrap_or(ProcessGroupSignalResult::Delivered))
        }
    }

    const TEST_PROCESS_GROUP_KEY: ProcessGroupKey = ProcessGroupKey {
        pgid: 42_424,
        generation: 7,
    };

    fn test_process_group_lease(
        state: UnixProcessGroupState,
        backend: Arc<MockProcessGroupBackend>,
    ) -> UnixProcessGroupLease {
        UnixProcessGroupLease {
            terminal_id: "test-terminal".to_string(),
            session_id: "test-session".to_string(),
            pid: TEST_PROCESS_GROUP_KEY.pgid as u32,
            backend,
            state: Mutex::new(state),
            cleanup_gate: Mutex::new(()),
        }
    }

    fn zero_grace_stages() -> [(libc::c_int, &'static str, Duration); 3] {
        [
            (libc::SIGINT, "sigint", Duration::ZERO),
            (libc::SIGTERM, "sigterm", Duration::ZERO),
            (libc::SIGKILL, "sigkill", Duration::ZERO),
        ]
    }

    #[tokio::test]
    async fn leader_exit_with_descendants_starts_bounded_lease() {
        let backend = Arc::new(MockProcessGroupBackend::default());
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedLeaderAlive {
                key: TEST_PROCESS_GROUP_KEY,
            },
            backend,
        );
        let observed_at = tokio::time::Instant::now();

        lease
            .observe_leader_exit(observed_at)
            .await
            .expect("observe leader exit");

        assert_eq!(
            *lease.state.lock().await,
            UnixProcessGroupState::OwnedDescendants {
                key: TEST_PROCESS_GROUP_KEY,
                deadline: observed_at + DESCENDANT_LEASE_DURATION,
            }
        );
    }

    #[tokio::test]
    async fn leader_exit_without_descendants_retires_before_delayed_release() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Missing,
        ]));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedLeaderAlive {
                key: TEST_PROCESS_GROUP_KEY,
            },
            backend.clone(),
        );
        lease
            .observe_leader_exit(tokio::time::Instant::now())
            .await
            .expect("observe leader exit");
        backend.clear_calls();

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("delayed cleanup");

        assert!(backend.calls().is_empty());
        assert!(backend.delivered_signals().is_empty());
        assert_eq!(*lease.state.lock().await, UnixProcessGroupState::Retired);
    }

    #[tokio::test]
    async fn naturally_exited_terminal_delayed_release_sends_no_group_signal() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Missing,
        ]));
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new())
            .with_process_group_backend(backend.clone());
        let session_id = SessionId::new("natural-exit-delayed-release".to_string());
        let request = CreateTerminalRequest::new(session_id.clone(), "/bin/true".to_string());
        let response = runtime
            .create_terminal(request)
            .await
            .expect("create short terminal");

        runtime
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                session_id.clone(),
                response.terminal_id,
            ))
            .await
            .expect("observe natural exit");
        backend.clear_calls();
        tokio::time::sleep(Duration::from_millis(25)).await;

        let report = runtime.release_all_for_session(session_id.0.as_ref()).await;

        assert!(report.is_clean());
        assert!(backend.calls().is_empty());
        assert!(backend.delivered_signals().is_empty());
    }

    #[tokio::test]
    async fn concurrent_cleanup_never_signals_after_reap_before_lease_observation() {
        let (call_tx, mut call_rx) = tokio::sync::mpsc::unbounded_channel();
        let backend = Arc::new(
            MockProcessGroupBackend::with_probes([ProcessGroupPresence::Missing])
                .with_call_tx(call_tx),
        );
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new())
            .with_process_group_backend(backend.clone());
        let session_id = SessionId::new("reap-before-lease-observation".to_string());
        let response = runtime
            .create_terminal(CreateTerminalRequest::new(
                session_id.clone(),
                "/bin/true".to_string(),
            ))
            .await
            .expect("create short terminal");
        let terminal = runtime
            .find_terminal(&response.terminal_id.to_string(), session_id.0.as_ref())
            .await
            .expect("find test terminal");
        let barrier = ExitObservationBarrier {
            reached: Arc::new(tokio::sync::Barrier::new(2)),
            resume: Arc::new(tokio::sync::Barrier::new(2)),
        };
        terminal.install_exit_observation_barrier(barrier.clone());

        let refresh_terminal = terminal.clone();
        let refresh = tokio::spawn(async move {
            loop {
                refresh_terminal.refresh_exit_status().await?;
                if refresh_terminal.snapshot.lock().await.exit_status.is_some() {
                    return Ok::<(), TerminalRuntimeError>(());
                }
                tokio::time::sleep(CHILD_EXIT_POLL_INTERVAL).await;
            }
        });
        tokio::time::timeout(Duration::from_secs(1), barrier.reached.wait())
            .await
            .expect("direct child did not reach controlled reap observation");
        let leader_key = match *terminal.process_group.state.lock().await {
            UnixProcessGroupState::OwnedLeaderAlive { key } => key,
            other => panic!("lease changed before the controlled observation point: {other:?}"),
        };
        assert!(
            terminal.child.lock().await.is_some(),
            "child must not become absent before the lease observation commits"
        );

        let cleanup_terminal = terminal.clone();
        let cleanup = tokio::spawn(async move { cleanup_terminal.kill_command().await });
        let early_call = tokio::time::timeout(Duration::from_millis(50), call_rx.recv()).await;
        assert!(
            early_call.is_err(),
            "cleanup signalled or probed while leader exit observation was paused: {early_call:?}"
        );

        tokio::time::timeout(Duration::from_secs(1), barrier.resume.wait())
            .await
            .expect("controlled reap observation did not resume");
        refresh.await.expect("join refresh").expect("refresh exit");
        cleanup.await.expect("join cleanup").expect("cleanup exit");

        assert_eq!(backend.calls(), vec![BackendCall::Probe(leader_key)]);
        assert!(backend.delivered_signals().is_empty());
        assert_eq!(
            *terminal.process_group.state.lock().await,
            UnixProcessGroupState::Retired
        );

        // A later user of the same numeric PGID remains outside this terminal's
        // retired lease and receives no further backend operation.
        backend.push_probe(ProcessGroupPresence::Present);
        let calls_after_retirement = backend.calls();
        terminal.kill_command().await.expect("repeated cleanup");
        assert_eq!(backend.calls(), calls_after_retirement);
        assert!(backend.delivered_signals().is_empty());
    }

    #[tokio::test]
    async fn descendant_lease_release_before_deadline_can_signal_original_group() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Missing,
        ]));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedDescendants {
                key: TEST_PROCESS_GROUP_KEY,
                deadline: tokio::time::Instant::now() + DESCENDANT_LEASE_DURATION,
            },
            backend.clone(),
        );

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("cleanup descendants");

        assert_eq!(
            backend.delivered_signals(),
            vec![libc::SIGINT, libc::SIGTERM, libc::SIGKILL]
        );
        assert!(backend.calls().iter().all(|call| match call {
            BackendCall::Probe(key) | BackendCall::Signal(key, _) => {
                *key == TEST_PROCESS_GROUP_KEY
            }
        }));
    }

    #[tokio::test]
    async fn missing_descendant_group_retires_before_signal_and_ignores_later_reuse() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Missing,
        ]));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedDescendants {
                key: TEST_PROCESS_GROUP_KEY,
                deadline: tokio::time::Instant::now() + DESCENDANT_LEASE_DURATION,
            },
            backend.clone(),
        );

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("missing descendant group cleanup");

        assert_eq!(
            backend.calls(),
            vec![BackendCall::Probe(TEST_PROCESS_GROUP_KEY)]
        );
        assert!(backend.delivered_signals().is_empty());
        assert_eq!(*lease.state.lock().await, UnixProcessGroupState::Retired);

        // The original group is gone. A later owner reusing its numeric PGID
        // must not be queried or signalled by this terminal.
        backend.push_probe(ProcessGroupPresence::Present);
        let calls_after_retirement = backend.calls();
        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("repeated cleanup after simulated reuse");

        assert_eq!(backend.calls(), calls_after_retirement);
        assert!(backend.delivered_signals().is_empty());
    }

    #[tokio::test]
    async fn descendant_lease_expiry_during_probe_retires_before_signal() {
        let backend = Arc::new(MockProcessGroupBackend::with_probe_delay(
            [ProcessGroupPresence::Present],
            Duration::from_millis(10),
        ));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedDescendants {
                key: TEST_PROCESS_GROUP_KEY,
                deadline: tokio::time::Instant::now() + Duration::from_millis(1),
            },
            backend.clone(),
        );

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("expired descendant group cleanup");

        assert_eq!(
            backend.calls(),
            vec![BackendCall::Probe(TEST_PROCESS_GROUP_KEY)]
        );
        assert!(backend.delivered_signals().is_empty());
        assert_eq!(*lease.state.lock().await, UnixProcessGroupState::Retired);
    }

    #[tokio::test]
    async fn descendant_probe_esrch_between_stages_retires_before_next_signal() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Missing,
        ]));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedDescendants {
                key: TEST_PROCESS_GROUP_KEY,
                deadline: tokio::time::Instant::now() + DESCENDANT_LEASE_DURATION,
            },
            backend.clone(),
        );

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("stage cleanup");

        assert_eq!(backend.delivered_signals(), vec![libc::SIGINT]);
        assert_eq!(
            backend.calls(),
            vec![
                BackendCall::Probe(TEST_PROCESS_GROUP_KEY),
                BackendCall::Signal(TEST_PROCESS_GROUP_KEY, libc::SIGINT),
                BackendCall::Probe(TEST_PROCESS_GROUP_KEY),
                BackendCall::Probe(TEST_PROCESS_GROUP_KEY),
            ]
        );
        assert_eq!(*lease.state.lock().await, UnixProcessGroupState::Retired);
    }

    #[tokio::test]
    async fn descendant_signal_esrch_retires_and_repeat_cleanup_is_noop() {
        let backend = Arc::new(MockProcessGroupBackend {
            probes: std::sync::Mutex::new([ProcessGroupPresence::Present].into_iter().collect()),
            signals: std::sync::Mutex::new(
                [ProcessGroupSignalResult::Missing].into_iter().collect(),
            ),
            ..MockProcessGroupBackend::default()
        });
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedDescendants {
                key: TEST_PROCESS_GROUP_KEY,
                deadline: tokio::time::Instant::now() + DESCENDANT_LEASE_DURATION,
            },
            backend.clone(),
        );

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("signal ESRCH cleanup");
        let calls_after_first_cleanup = backend.calls();

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("repeated cleanup after signal ESRCH");

        assert_eq!(backend.calls(), calls_after_first_cleanup);
        assert_eq!(
            backend.delivered_signals(),
            vec![libc::SIGINT],
            "the backend observed the attempted signal, but ESRCH delivered it to no group"
        );
        assert_eq!(*lease.state.lock().await, UnixProcessGroupState::Retired);
    }

    #[tokio::test]
    async fn expired_descendant_lease_retires_without_backend_call() {
        let backend = Arc::new(MockProcessGroupBackend::default());
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedDescendants {
                key: TEST_PROCESS_GROUP_KEY,
                deadline: tokio::time::Instant::now(),
            },
            backend.clone(),
        );

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("expire descendants lease");

        assert!(backend.calls().is_empty());
        assert_eq!(*lease.state.lock().await, UnixProcessGroupState::Retired);
    }

    #[tokio::test]
    async fn retired_lease_ignores_simulated_pgid_reuse() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Missing,
        ]));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedLeaderAlive {
                key: TEST_PROCESS_GROUP_KEY,
            },
            backend.clone(),
        );
        lease
            .observe_leader_exit(tokio::time::Instant::now())
            .await
            .expect("retire original group");
        backend.clear_calls();
        backend.push_probe(ProcessGroupPresence::Present);

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("cleanup reused numeric pgid");

        assert!(backend.calls().is_empty());
        assert!(backend.delivered_signals().is_empty());
    }

    #[tokio::test]
    async fn esrch_retires_permanently_and_repeat_cleanup_is_noop() {
        let backend = Arc::new(MockProcessGroupBackend::with_signals([
            ProcessGroupSignalResult::Missing,
        ]));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedLeaderAlive {
                key: TEST_PROCESS_GROUP_KEY,
            },
            backend.clone(),
        );
        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("first cleanup");
        let calls_after_first = backend.calls();

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("repeat cleanup");

        assert_eq!(backend.calls(), calls_after_first);
        assert_eq!(*lease.state.lock().await, UnixProcessGroupState::Retired);
    }

    #[tokio::test]
    async fn concurrent_cleanup_runs_one_signal_sequence() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Missing,
        ]));
        let lease = Arc::new(test_process_group_lease(
            UnixProcessGroupState::OwnedLeaderAlive {
                key: TEST_PROCESS_GROUP_KEY,
            },
            backend.clone(),
        ));
        let stages = zero_grace_stages();

        let (left, right) = tokio::join!(
            lease.cleanup_with_stages(&stages, None),
            lease.cleanup_with_stages(&stages, None),
        );

        left.expect("left cleanup");
        right.expect("right cleanup");
        assert_eq!(
            backend.delivered_signals(),
            vec![libc::SIGINT, libc::SIGTERM, libc::SIGKILL]
        );
    }

    #[tokio::test]
    async fn live_leader_keeps_staged_signal_sequence() {
        let backend = Arc::new(MockProcessGroupBackend::with_probes([
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Present,
            ProcessGroupPresence::Missing,
        ]));
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedLeaderAlive {
                key: TEST_PROCESS_GROUP_KEY,
            },
            backend.clone(),
        );

        lease
            .cleanup_with_stages(&zero_grace_stages(), None)
            .await
            .expect("cleanup live leader");

        assert_eq!(
            backend.delivered_signals(),
            vec![libc::SIGINT, libc::SIGTERM, libc::SIGKILL]
        );
    }

    #[tokio::test]
    async fn leader_exit_with_live_descendants_is_not_retired_immediately() {
        let backend = Arc::new(MockProcessGroupBackend::default());
        let lease = test_process_group_lease(
            UnixProcessGroupState::OwnedLeaderAlive {
                key: TEST_PROCESS_GROUP_KEY,
            },
            backend,
        );

        lease
            .observe_leader_exit(tokio::time::Instant::now())
            .await
            .expect("observe descendants");

        assert!(matches!(
            *lease.state.lock().await,
            UnixProcessGroupState::OwnedDescendants { .. }
        ));
    }

    async fn terminal_pid(
        runtime: &TerminalRuntime,
        session_id: &SessionId,
        terminal_id: &str,
    ) -> u32 {
        let terminal = runtime
            .find_terminal(terminal_id, session_id.0.as_ref())
            .await
            .expect("test terminal exists");
        let pid = terminal
            .child
            .lock()
            .await
            .as_ref()
            .and_then(tokio::process::Child::id)
            .expect("test terminal has a direct child pid");
        pid
    }

    fn pid_exists(pid: u32) -> bool {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    async fn emergency_kill_exact_tree(pid: u32, terminal: &Arc<TerminalInstance>) {
        let pid = pid as libc::pid_t;
        let _ = unsafe { libc::kill(pid, libc::SIGSTOP) };
        let config = Config {
            signal: "SIGKILL".to_string(),
            include_target: true,
        };
        let _ = kill_tree::tokio::kill_tree_with_config(pid as u32, &config).await;
        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
        let _ = tokio::time::timeout(Duration::from_secs(1), terminal.wait_for_exit()).await;
        for _ in 0..100 {
            if !pid_exists(pid as u32) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn spawn_shell(
        runtime: &TerminalRuntime,
        session_id: &SessionId,
        script: &str,
    ) -> (String, u32) {
        let mut request = CreateTerminalRequest::new(session_id.clone(), "/bin/sh".to_string());
        request.args = vec!["-c".into(), script.into()];
        let response = runtime
            .create_terminal(request)
            .await
            .expect("create test terminal");
        let terminal_id = response.terminal_id.to_string();
        let pid = terminal_pid(runtime, session_id, &terminal_id).await;
        (terminal_id, pid)
    }

    async fn wait_for_output(terminal: &Arc<TerminalInstance>, needle: &str) -> bool {
        for _ in 0..100 {
            if terminal.snapshot().await.output.contains(needle) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// Regression: when an ACP agent calls `terminal/create` (e.g. to run
    /// `git push`), the runtime's base env — populated by the connection
    /// layer with the codeg credential helper's `GIT_CONFIG_*` keys —
    /// must reach the spawned process. Per-request `env` from the agent
    /// still wins on key collision so the agent can scrub or override
    /// specific keys for individual commands.
    #[tokio::test]
    async fn base_env_propagates_and_request_env_overrides() {
        let mut base_env = BTreeMap::new();
        base_env.insert("CODEG_TEST_BASE_VAR".to_string(), "from_base".to_string());
        base_env.insert("CODEG_TEST_OVERRIDE".to_string(), "loses".to_string());
        let runtime = TerminalRuntime::with_base_env(base_env);

        let session_id = SessionId::new("test-session".to_string());
        let mut request = CreateTerminalRequest::new(session_id.clone(), "/bin/sh".to_string());
        request.args = vec![
            "-c".into(),
            // Print both vars on separate lines so we can match each
            // independently regardless of shell quoting.
            "printf '%s\\n' \"$CODEG_TEST_BASE_VAR\" \"$CODEG_TEST_OVERRIDE\"".into(),
        ];
        request.env = vec![EnvVariable::new("CODEG_TEST_OVERRIDE", "request_wins")];

        let response = runtime
            .create_terminal(request)
            .await
            .expect("create terminal");
        let terminal_id = response.terminal_id.clone();

        // Wait for the child to exit so the captured output is final.
        runtime
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                session_id.clone(),
                terminal_id.clone(),
            ))
            .await
            .expect("wait for exit");

        let out = runtime
            .terminal_output(TerminalOutputRequest::new(
                session_id.clone(),
                terminal_id.clone(),
            ))
            .await
            .expect("get output");

        assert!(
            out.output.contains("from_base"),
            "base env did not reach the spawned process; got:\n{}",
            out.output
        );
        assert!(
            out.output.contains("request_wins"),
            "per-request env did not override base on key collision; got:\n{}",
            out.output
        );
        assert!(
            !out.output.contains("loses"),
            "base value leaked through despite the request override; got:\n{}",
            out.output
        );

        // Drop terminal handle so the runtime drops its writer ends.
        runtime.release_all_for_session(session_id.0.as_ref()).await;
    }

    /// Spawn `request`, wait for it to exit, return its captured output, and
    /// release the session's terminals.
    async fn run_and_capture(
        runtime: &TerminalRuntime,
        session_id: &SessionId,
        request: CreateTerminalRequest,
    ) -> String {
        let response = runtime
            .create_terminal(request)
            .await
            .expect("create terminal");
        let terminal_id = response.terminal_id.clone();
        runtime
            .wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                session_id.clone(),
                terminal_id.clone(),
            ))
            .await
            .expect("wait for exit");
        let out = runtime
            .terminal_output(TerminalOutputRequest::new(
                session_id.clone(),
                terminal_id.clone(),
            ))
            .await
            .expect("get output");
        runtime.release_all_for_session(session_id.0.as_ref()).await;
        out.output
    }

    /// A `terminal/create` that omits `cwd` defaults to the runtime's
    /// configured working directory rather than codeg's own process cwd.
    #[tokio::test]
    async fn falls_back_to_default_cwd_when_request_omits_cwd() {
        let dir = tempfile::tempdir().expect("temp dir");
        let canonical = dir.path().canonicalize().expect("canonicalize");
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new())
            .with_default_cwd(Some(dir.path().to_path_buf()));

        let session_id = SessionId::new("cwd-default".to_string());
        // Bare `pwd` (no whitespace) → direct exec; current_dir still applies.
        let request = CreateTerminalRequest::new(session_id.clone(), "pwd".to_string());
        let output = run_and_capture(&runtime, &session_id, request).await;

        assert!(
            output.contains(canonical.to_string_lossy().as_ref()),
            "terminal did not run in the default cwd; got:\n{output}"
        );
    }

    /// An explicit absolute `cwd` in the request takes precedence over the
    /// runtime default.
    #[tokio::test]
    async fn request_cwd_overrides_default_cwd() {
        let default_dir = tempfile::tempdir().expect("default dir");
        let request_dir = tempfile::tempdir().expect("request dir");
        let request_canonical = request_dir.path().canonicalize().expect("canonicalize");
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new())
            .with_default_cwd(Some(default_dir.path().to_path_buf()));

        let session_id = SessionId::new("cwd-override".to_string());
        let mut request = CreateTerminalRequest::new(session_id.clone(), "pwd".to_string());
        request.cwd = Some(request_dir.path().to_path_buf());
        let output = run_and_capture(&runtime, &session_id, request).await;

        assert!(
            output.contains(request_canonical.to_string_lossy().as_ref()),
            "request cwd did not take precedence over the default; got:\n{output}"
        );
    }

    /// A whitespace-bearing command with empty args runs through the shell. A
    /// direct exec would ENOENT trying to run a program literally named with
    /// spaces — this is the shape CodeBuddy sends.
    #[tokio::test]
    async fn whitespace_command_runs_through_shell() {
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new());

        let session_id = SessionId::new("shell-wrap".to_string());
        let request =
            CreateTerminalRequest::new(session_id.clone(), "echo hello world".to_string());
        let output = run_and_capture(&runtime, &session_id, request).await;
        assert!(
            output.contains("hello world"),
            "shell did not run the whitespace command; got:\n{output}"
        );

        // Genuine shell operators must evaluate, not be passed as literal args.
        let session_id = SessionId::new("shell-ops".to_string());
        let request = CreateTerminalRequest::new(session_id.clone(), "true && echo OK".to_string());
        let output = run_and_capture(&runtime, &session_id, request).await;
        assert!(
            output.contains("OK"),
            "shell operators did not evaluate; got:\n{output}"
        );
    }

    /// The shell-wrapped path still honors the working directory, so a
    /// CodeBuddy-style `pnpm build` runs in the session folder.
    #[tokio::test]
    async fn shell_wrapped_command_respects_cwd() {
        let dir = tempfile::tempdir().expect("temp dir");
        let canonical = dir.path().canonicalize().expect("canonicalize");
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new())
            .with_default_cwd(Some(dir.path().to_path_buf()));

        let session_id = SessionId::new("shell-cwd".to_string());
        // Whitespace → shell-wrapped; must run in the default cwd.
        let request =
            CreateTerminalRequest::new(session_id.clone(), "pwd && echo done".to_string());
        let output = run_and_capture(&runtime, &session_id, request).await;
        assert!(
            output.contains(canonical.to_string_lossy().as_ref()) && output.contains("done"),
            "shell-wrapped command ignored the default cwd; got:\n{output}"
        );
    }

    /// When the agent supplies explicit `args`, the command is exec'd directly
    /// (no shell), so an argument containing spaces stays a single argument.
    #[tokio::test]
    async fn explicit_args_bypass_shell_wrap() {
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new());

        let session_id = SessionId::new("direct-exec".to_string());
        let mut request = CreateTerminalRequest::new(session_id.clone(), "/bin/echo".to_string());
        request.args = vec!["hello world".into()];
        let output = run_and_capture(&runtime, &session_id, request).await;
        assert!(
            output.contains("hello world"),
            "direct exec did not pass the single arg through; got:\n{output}"
        );
    }

    /// An explicit but non-existent absolute `cwd` is honored as-is and
    /// surfaces as a spawn failure — never silently downgraded to the default
    /// fallback or the inherited process cwd.
    #[tokio::test]
    async fn explicit_missing_cwd_surfaces_as_spawn_failure() {
        let default_dir = tempfile::tempdir().expect("default dir");
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new())
            .with_default_cwd(Some(default_dir.path().to_path_buf()));

        let session_id = SessionId::new("missing-cwd".to_string());
        let mut request = CreateTerminalRequest::new(session_id, "pwd".to_string());
        request.cwd = Some(PathBuf::from("/codeg-nonexistent-cwd/does/not/exist"));

        let result = runtime.create_terminal(request).await;
        assert!(
            matches!(result, Err(TerminalRuntimeError::Internal(_))),
            "expected a spawn failure for a missing explicit cwd"
        );
    }

    /// A real executable whose path contains spaces is exec'd directly: the
    /// direct spawn succeeds, so the shell fallback never fires (shell-wrapping
    /// would split the path at the space).
    #[tokio::test]
    async fn executable_path_with_spaces_is_not_shell_wrapped() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let exe = dir.path().join("my tool"); // space in the file name
        std::fs::write(&exe, "#!/bin/sh\necho ran-directly\n").expect("write script");
        let mut perms = std::fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&exe, perms).expect("chmod");

        let runtime = TerminalRuntime::with_base_env(BTreeMap::new());
        let session_id = SessionId::new("space-exe".to_string());
        // Empty args + whitespace in command, but it resolves to a real
        // executable, so it must run directly rather than via the shell.
        let request =
            CreateTerminalRequest::new(session_id.clone(), exe.to_string_lossy().to_string());
        let output = run_and_capture(&runtime, &session_id, request).await;
        assert!(
            output.contains("ran-directly"),
            "space-containing executable was not exec'd directly; got:\n{output}"
        );
    }

    /// A whitespace-only command is rejected up front rather than spawning a
    /// shell no-op.
    #[tokio::test]
    async fn whitespace_only_command_is_rejected() {
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new());
        let session_id = SessionId::new("blank-command".to_string());
        let request = CreateTerminalRequest::new(session_id, "   ".to_string());

        let result = runtime.create_terminal(request).await;
        assert!(
            matches!(result, Err(TerminalRuntimeError::InvalidParams(_))),
            "expected InvalidParams for a whitespace-only command"
        );
    }

    /// A relative, space-containing executable resolves against the terminal's
    /// effective cwd and runs directly — the shell fallback fires only on a
    /// genuine NotFound. This guards the regression where a pre-spawn `which`
    /// check, run in codeg's own cwd, would shell-wrap and split the path.
    #[tokio::test]
    async fn relative_executable_with_spaces_runs_in_effective_cwd() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let exe = dir.path().join("my tool"); // space in the file name
        let mut file = std::fs::File::create(&exe).expect("create script");
        file.write_all(b"#!/bin/sh\necho ran-relative\n")
            .expect("write script");
        file.sync_all().expect("sync script before exec");
        drop(file);
        let mut perms = std::fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&exe, perms).expect("chmod");

        let runtime = TerminalRuntime::with_base_env(BTreeMap::new())
            .with_default_cwd(Some(dir.path().to_path_buf()));
        let session_id = SessionId::new("rel-space-exe".to_string());
        // "./my tool": empty args + whitespace, resolvable only against the
        // terminal's cwd — must run directly, not via the shell.
        let request = CreateTerminalRequest::new(session_id.clone(), "./my tool".to_string());
        let output = run_and_capture(&runtime, &session_id, request).await;
        assert!(
            output.contains("ran-relative"),
            "relative space-containing exe was not run in the effective cwd; got:\n{output}"
        );
    }

    #[tokio::test]
    async fn unix_terminal_isolated_in_own_process_group() {
        init_test_tracing();
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new());
        let session_id = SessionId::new("pgid-isolation".to_string());
        let (_terminal_id, pid) = spawn_shell(&runtime, &session_id, "sleep 60").await;

        let child_pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
        let runner_pgid = unsafe { libc::getpgrp() };
        runtime.release_all_for_session(session_id.0.as_ref()).await;

        assert_eq!(
            child_pgid, pid as libc::pid_t,
            "child must lead its own process group"
        );
        assert_ne!(
            child_pgid, runner_pgid,
            "child must not share the test runner process group"
        );
    }

    #[tokio::test]
    async fn exited_leader_keeps_short_lease_for_live_same_group_descendant() {
        init_test_tracing();
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new());
        let session_id = SessionId::new("exited-leader-live-descendant".to_string());
        let (terminal_id, leader_pid) = spawn_shell(
            &runtime,
            &session_id,
            "sleep 60 & child=$!; printf 'descendant=%s\\n' \"$child\"; exit 0",
        )
        .await;
        let terminal = runtime
            .find_terminal(&terminal_id, session_id.0.as_ref())
            .await
            .expect("terminal exists");
        assert!(
            wait_for_output(&terminal, "descendant=").await,
            "shell did not report its test-owned descendant"
        );
        let output = terminal.snapshot().await.output;
        let descendant_pid = output
            .lines()
            .find_map(|line| line.strip_prefix("descendant="))
            .and_then(|pid| pid.parse::<u32>().ok())
            .expect("parse descendant pid");

        terminal
            .wait_for_exit()
            .await
            .expect("observe direct child exit");
        let descendant_pgid = unsafe { libc::getpgid(descendant_pid as libc::pid_t) };
        let state_after_leader_exit = *terminal.process_group.state.lock().await;

        assert_eq!(
            descendant_pgid, leader_pid as libc::pid_t,
            "descendant must remain in the terminal's original process group"
        );
        assert!(
            pid_exists(descendant_pid),
            "descendant must still be alive when the leader exit is observed"
        );
        assert!(matches!(
            state_after_leader_exit,
            UnixProcessGroupState::OwnedDescendants {
                key: ProcessGroupKey { pgid, .. },
                deadline,
            } if pgid == leader_pid as libc::pid_t && deadline > tokio::time::Instant::now()
        ));

        let report = runtime.release_all_for_session(session_id.0.as_ref()).await;
        if pid_exists(descendant_pid) {
            let _ = unsafe { libc::kill(descendant_pid as libc::pid_t, libc::SIGKILL) };
        }

        assert!(report.is_clean(), "descendant cleanup should succeed");
        assert!(
            !pid_exists(descendant_pid),
            "same-group descendant survived release inside its lease"
        );
    }

    #[tokio::test]
    async fn session_cleanup_kills_stubborn_group_without_touching_other_session() {
        init_test_tracing();
        let runtime = TerminalRuntime::with_base_env(BTreeMap::new());
        let victim_session = SessionId::new("stubborn-victim".to_string());
        let other_session = SessionId::new("unrelated-session".to_string());
        let (victim_terminal_id, victim_pid) = spawn_shell(
            &runtime,
            &victim_session,
            "trap '' INT TERM; printf 'ready\\n'; while :; do sleep 60 & wait $!; done",
        )
        .await;
        let victim_terminal = runtime
            .find_terminal(&victim_terminal_id, victim_session.0.as_ref())
            .await
            .expect("victim terminal exists");
        if !wait_for_output(&victim_terminal, "ready\n").await {
            emergency_kill_exact_tree(victim_pid, &victim_terminal).await;
            panic!("stubborn shell did not install signal traps in time");
        }
        let (_other_terminal, other_pid) =
            spawn_shell(&runtime, &other_session, "while :; do sleep 60; done").await;

        let started = Instant::now();
        let cleanup = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.release_all_for_session(victim_session.0.as_ref()),
        )
        .await;

        if cleanup.is_err() {
            emergency_kill_exact_tree(victim_pid, &victim_terminal).await;
        }
        let other_still_alive = pid_exists(other_pid);
        runtime
            .release_all_for_session(other_session.0.as_ref())
            .await;

        assert!(
            cleanup.is_ok(),
            "stubborn terminal cleanup exceeded five seconds"
        );
        assert!(
            !pid_exists(victim_pid),
            "victim direct child survived cleanup"
        );
        assert!(
            other_still_alive,
            "cleanup signalled a terminal from another session"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn in_flight_wait_does_not_block_session_cleanup() {
        init_test_tracing();
        let runtime = Arc::new(TerminalRuntime::with_base_env(BTreeMap::new()));
        let session_id = SessionId::new("wait-does-not-lock-cancel".to_string());
        let (terminal_id, pid) = spawn_shell(&runtime, &session_id, "sleep 60").await;
        let terminal = runtime
            .find_terminal(&terminal_id, session_id.0.as_ref())
            .await
            .expect("terminal exists");

        let waiter_runtime = Arc::clone(&runtime);
        let waiter_session = session_id.clone();
        let waiter_terminal = terminal_id.clone();
        let wait_task = tokio::spawn(async move {
            waiter_runtime
                .wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                    waiter_session,
                    waiter_terminal,
                ))
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let cleanup = tokio::time::timeout(
            Duration::from_secs(5),
            runtime.release_all_for_session(session_id.0.as_ref()),
        )
        .await;
        if cleanup.is_err() {
            emergency_kill_exact_tree(pid, &terminal).await;
        }
        let wait_result = tokio::time::timeout(Duration::from_secs(1), wait_task).await;

        assert!(
            cleanup.is_ok(),
            "terminal wait held the child mutex across cancellation"
        );
        assert!(
            wait_result.is_ok(),
            "terminal wait did not observe cancellation exit"
        );
    }
}
