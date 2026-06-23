use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
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

#[derive(Debug, Default, Clone)]
struct TerminalSnapshot {
    output: String,
    output_base_offset: u64,
    truncated: bool,
    exit_status: Option<TerminalExitStatus>,
}

struct TerminalInstance {
    session_id: String,
    output_limit: Option<usize>,
    child: Mutex<Option<tokio::process::Child>>,
    snapshot: Mutex<TerminalSnapshot>,
    reader_handles: Mutex<Vec<JoinHandle<()>>>,
}

impl TerminalInstance {
    fn new(session_id: String, output_limit: Option<u64>, child: tokio::process::Child) -> Self {
        Self {
            session_id,
            output_limit: output_limit.and_then(|v| usize::try_from(v).ok()),
            child: Mutex::new(Some(child)),
            snapshot: Mutex::new(TerminalSnapshot::default()),
            reader_handles: Mutex::new(Vec::new()),
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
        }

        Ok(())
    }

    async fn wait_for_exit(&self) -> Result<TerminalExitStatus, TerminalRuntimeError> {
        self.refresh_exit_status().await?;
        let cached_exit = self.snapshot.lock().await.exit_status.clone();
        if let Some(exit_status) = cached_exit {
            self.drain_readers().await;
            return Ok(exit_status);
        }

        let exit_status = {
            let mut child_guard = self.child.lock().await;
            let Some(child) = child_guard.as_mut() else {
                return Err(TerminalRuntimeError::Internal(
                    "terminal process missing while waiting for exit".to_string(),
                ));
            };
            let status = child.wait().await.map_err(|err| {
                TerminalRuntimeError::Internal(format!(
                    "failed waiting for terminal process to exit: {err}"
                ))
            })?;
            *child_guard = None;
            map_exit_status(status)
        };

        self.drain_readers().await;

        let mut snapshot = self.snapshot.lock().await;
        snapshot.exit_status = Some(exit_status.clone());
        Ok(exit_status)
    }

    async fn kill_command(&self) -> Result<(), TerminalRuntimeError> {
        self.refresh_exit_status().await?;
        let already_exited = self.snapshot.lock().await.exit_status.is_some();
        if already_exited {
            self.drain_readers().await;
            return Ok(());
        }

        let exit_status = {
            let mut child_guard = self.child.lock().await;
            let Some(child) = child_guard.as_mut() else {
                return Ok(());
            };

            if let Some(pid) = child.id() {
                if let Err(err) = kill_tree::tokio::kill_tree(pid).await {
                    tracing::error!("[ACP] kill_tree failed for pid {pid}: {err}");
                }
            }

            let status = child.wait().await.map_err(|err| {
                TerminalRuntimeError::Internal(format!(
                    "failed to wait for killed terminal process: {err}"
                ))
            })?;
            *child_guard = None;
            map_exit_status(status)
        };

        self.drain_readers().await;

        let mut snapshot = self.snapshot.lock().await;
        snapshot.exit_status = Some(exit_status);
        Ok(())
    }

    async fn snapshot(&self) -> TerminalSnapshot {
        self.snapshot.lock().await.clone()
    }
}

pub struct TerminalRuntime {
    terminals: Mutex<TerminalMap>,
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
            base_env,
            default_cwd: None,
        }
    }

    /// Set the fallback working directory used when a `terminal/create` request
    /// does not specify its own `cwd`. Chainable after `with_base_env`.
    pub fn with_default_cwd(mut self, default_cwd: Option<PathBuf>) -> Self {
        self.default_cwd = default_cwd;
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

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let terminal_id = format!("term_{}", uuid::Uuid::new_v4().simple());
        let terminal = Arc::new(TerminalInstance::new(
            request.session_id.to_string(),
            Some(output_byte_limit),
            child,
        ));

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

    pub async fn release_all_for_session(&self, session_id: &str) {
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

        for terminal in removed {
            if let Err(err) = terminal.kill_command().await {
                tracing::error!("[ACP] Failed to release terminal during cleanup: {err:?}");
            }
        }
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
    use sacp::schema::{EnvVariable, SessionId, WaitForTerminalExitRequest};

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
        let request =
            CreateTerminalRequest::new(session_id.clone(), "true && echo OK".to_string());
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
        let mut request =
            CreateTerminalRequest::new(session_id.clone(), "/bin/echo".to_string());
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
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let exe = dir.path().join("my tool"); // space in the file name
        std::fs::write(&exe, "#!/bin/sh\necho ran-relative\n").expect("write script");
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
}
