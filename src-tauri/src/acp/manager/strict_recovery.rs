use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::acp::connection::ConnectionCommand;
use crate::acp::error::AcpError;
use crate::models::agent::AgentType;
use crate::web::event_bridge::EventEmitter;

use super::ConnectionManager;

impl ConnectionManager {
    /// Forcibly reclaim one stuck connection (reacceptance R7): the graceful
    /// `Disconnect` command is only consumed by the connection's conversation
    /// loop, so a driver still parked in the resume/load handshake
    /// (`block_task().await`) never reads it — plain [`Self::disconnect`]
    /// would deregister the connection and leave the agent process (and its
    /// tree) running with nothing owning it. This mirrors
    /// [`Self::disconnect_all`]'s ladder for a single connection: fire the
    /// graceful Disconnect, give the driver a short grace window, then
    /// hard-kill the agent process tree via its pid cell and confirm the
    /// exit through the `on_exit` zeroing. The confirmed exit is what makes
    /// the release a fact rather than an assertion.
    pub async fn disconnect_and_reclaim(&self, conn_id: &str) -> Result<(), AcpError> {
        const RECLAIM_GRACE: Duration = Duration::from_millis(500);
        const RECLAIM_EXIT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);
        let reclaimed = {
            let mut connections = self.connections.lock().await;
            connections
                .remove(conn_id)
                .map(|conn| (conn.cmd_tx, conn.child_pid))
        };
        let Some((cmd_tx, child_pid)) = reclaimed else {
            return Err(AcpError::ConnectionNotFound(conn_id.into()));
        };
        tracing::info!("[ACP] disconnect_and_reclaim connection={}", conn_id);
        // try_send: a wedged command queue (32 deep) must not park the
        // reclaim — the backstop kill below is exactly for that connection.
        let _ = cmd_tx.try_send(ConnectionCommand::Disconnect);
        tokio::time::sleep(RECLAIM_GRACE).await;
        let pid = child_pid.load(std::sync::atomic::Ordering::SeqCst);
        if pid != 0 {
            // Blocking kill_tree off the async runtime, same as
            // disconnect_all's backstop.
            let _ = tokio::task::spawn_blocking(move || kill_tree::blocking::kill_tree(pid)).await;
            // Confirm the exit via the on_exit zeroing of the pid cell
            // (bounded): a confirmed death is the only honest "released".
            let deadline = std::time::Instant::now() + RECLAIM_EXIT_CONFIRM_TIMEOUT;
            while std::time::Instant::now() < deadline {
                if child_pid.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    tracing::info!(
                        "[ACP] disconnect_and_reclaim confirmed exit for connection={}",
                        conn_id
                    );
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            return Err(AcpError::protocol(format!(
                "connection {conn_id}: process tree kill issued but exit was not \
                 confirmed within {}ms",
                RECLAIM_EXIT_CONFIRM_TIMEOUT.as_millis()
            )));
        }
        Ok(())
    }

    /// Strictly attach an EXISTING external agent session (v2 design §5.3).
    ///
    /// Unlike [`Self::spawn_agent`] this entry:
    /// * verifies the recorded resume binding BEFORE any agent process starts
    ///   (cwd must exist and match the binding; the freshly computed config
    ///   fingerprint must match the recorded one — a changed environment is a
    ///   typed `binding_mismatch`, never a silent relaunch);
    /// * refuses to reuse a live connection hosting the same external session
    ///   (a continuation round must own its connection; stealing one the user
    ///   or another driver holds would let a teardown kill it mid-run);
    /// * passes the strict recovery policy down the wire so the
    ///   `resume → load → new` chain can NEVER reach `session/new`; and
    /// * returns only a typed verdict: [`StrictReady`] after recovery, replay
    ///   drain, and config application all truly succeeded, or a
    ///   [`StrictAttachError`] (`resume_timeout` when the handshake exceeds
    ///   `timeout`).
    ///
    /// The connection this call creates is registered under the manager like
    /// any other and tears itself down on failure; the caller sends no prompt
    /// until it has seen `Ready`.
    #[allow(clippy::too_many_arguments)]
    pub async fn attach_existing_session_strict(
        &self,
        agent_type: AgentType,
        working_dir: String,
        external_session_id: String,
        runtime_env: BTreeMap<String, String>,
        owner_window_label: String,
        emitter: EventEmitter,
        preferred_mode_id: Option<String>,
        preferred_config_values: BTreeMap<String, String>,
        expected_cwd: PathBuf,
        expected_config_fingerprint: String,
        timeout: Duration,
        turn_id: &str,
        execution_id: &str,
    ) -> Result<
        crate::acp::delegation::continuation::StrictReady,
        crate::acp::delegation::continuation::StrictAttachError,
    > {
        use crate::acp::connection::spawn_agent_connection_with_transport;
        use crate::acp::delegation::continuation::{
            SessionRecovery, StrictAttachError, StrictAttachErrorCode, StrictOutcome,
        };

        // --- Binding sanity (cheap, pre-spawn) --------------------------------
        if external_session_id.trim().is_empty() {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::BindingMismatch,
                "the recorded binding carries no external session id",
            ));
        }
        // cwd 指向既有目录/既有 worktree — never a fresh directory conjured for
        // the continuation.
        if !expected_cwd.is_dir() {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::BindingMismatch,
                format!(
                    "the recorded working directory {} no longer exists",
                    expected_cwd.display()
                ),
            ));
        }

        // --- No live connection may be hijacked -------------------------------
        let working_dir_path = std::path::PathBuf::from(&working_dir);
        if self
            .find_connection_for_reuse(
                agent_type,
                Some(&working_dir_path),
                Some(external_session_id.as_str()),
            )
            .await
            .is_some()
        {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::ResumeFailed,
                "a live connection already hosts this session; a continuation \
                 round must not share it"
                    .to_string(),
            ));
        }

        // --- Launch the connection under the strict policy --------------------
        // The same launch the ordinary path performs: resolve the cwd, compute
        // the config fingerprint, build the agent transport. The gate verifies
        // the binding against these BEFORE the driver thread starts.
        let launch_cwd = crate::acp::connection::resolve_working_dir(Some(&working_dir));
        let config_fingerprint = crate::commands::acp::fingerprint_config(agent_type, &runtime_env);

        let (gate, mut verdict_rx) =
            crate::acp::delegation::continuation::StrictAttachGate::channel_for_continuation(
                expected_cwd,
                expected_config_fingerprint,
                turn_id.to_string(),
                execution_id.to_string(),
            );
        if let Err(err) = gate.verify_launch(&launch_cwd, &config_fingerprint) {
            gate.fail(err.clone()).await;
            return Err(err);
        }

        let stderr_tail = Arc::new(crate::acp::stderr_tail::StderrTail::new());
        let agent = crate::acp::connection::build_agent(
            agent_type,
            &runtime_env,
            &launch_cwd,
            &stderr_tail,
        )
        .await
        .map_err(|e| {
            StrictAttachError::new(
                StrictAttachErrorCode::ResumeFailed,
                format!("agent launch failed: {e}"),
            )
        })?;

        let connection_id = uuid::Uuid::new_v4().to_string();
        let spawn_result = spawn_agent_connection_with_transport(
            agent,
            connection_id.clone(),
            agent_type,
            Some(working_dir),
            Some(external_session_id),
            runtime_env,
            owner_window_label,
            emitter,
            self.connections.clone(),
            preferred_mode_id,
            preferred_config_values,
            self.delegation_snapshot(),
            self.terminal_shell_config.clone(),
            config_fingerprint,
            stderr_tail,
            SessionRecovery::RequireExisting(gate),
        )
        .await;
        if let Err(spawn_err) = spawn_result {
            // Binding verification (or registration) failed before the driver
            // thread started. The gate usually already carries the precise
            // typed failure — prefer it over the opaque AcpError.
            match tokio::time::timeout(Duration::from_secs(1), &mut verdict_rx).await {
                Ok(Ok(StrictOutcome::Failed(e))) => return Err(e),
                _ => {
                    return Err(StrictAttachError::new(
                        StrictAttachErrorCode::ResumeFailed,
                        format!("strict attach failed to launch: {spawn_err}"),
                    ));
                }
            }
        }

        // --- Await the typed verdict (bounded) --------------------------------
        match tokio::time::timeout(timeout, &mut verdict_rx).await {
            Ok(Ok(StrictOutcome::Ready(ready))) => Ok(ready),
            Ok(Ok(StrictOutcome::Failed(e))) => {
                // The strict path rejected itself before or after launch; the
                // driver thread tears the connection down on its own error
                // unwind, but a typed failure delivered while the connection
                // is still alive needs an explicit reclaim here.
                let _ = self.disconnect(&connection_id).await;
                Err(e)
            }
            // The connection ended without ever reporting readiness — treat it
            // as a failed recovery, never as success.
            Ok(Err(_receiver_dropped)) => Err(StrictAttachError::new(
                StrictAttachErrorCode::ResumeFailed,
                "the connection ended before reporting strict readiness",
            )),
            Err(_elapsed) => {
                // Timed out waiting for the handshake. The driver is parked
                // in `block_task()` and NEVER reads the queued Disconnect,
                // so the reclaim must go through the forced ladder —
                // graceful command, grace window, hard `kill_tree`, confirmed
                // exit — or the agent process would outlive its registration
                // and block every future strict resume of the same session
                // (acceptance F9, reacceptance R7). A late Ready lands on a
                // dead connection and is harmless.
                let reclaim = self.disconnect_and_reclaim(&connection_id).await;
                Err(StrictAttachError::new(
                    StrictAttachErrorCode::ResumeTimeout,
                    match reclaim {
                        Ok(()) => format!(
                            "strict attach did not become ready within {}ms; the launched \
                             connection's process tree was killed and its exit confirmed; \
                             no prompt was sent",
                            timeout.as_millis()
                        ),
                        Err(reclaim_err) => format!(
                            "strict attach did not become ready within {}ms and the \
                             launched connection could not be fully reclaimed ({reclaim_err}); \
                             no prompt was sent",
                            timeout.as_millis()
                        ),
                    },
                ))
            }
        }
    }

    /// Capture the non-sensitive resume-binding facts (external session id,
    /// resolved launch cwd, execution-config fingerprint) for a live
    /// connection. Consumed by the delegation broker when freezing a
    /// successful outcome's resume binding; identities only — never tokens,
    /// API keys, or environment variables. `None` when the connection is gone.
    pub async fn resume_binding_facts(
        &self,
        conn_id: &str,
    ) -> Option<crate::acp::delegation::spawner::ResumeBindingFacts> {
        let connections = self.connections.lock().await;
        let entry = connections.get(conn_id)?;
        let cwd = entry.working_dir.clone();
        let config_fingerprint = entry.config_fingerprint.clone();
        let state = entry.state.read().await;
        Some(crate::acp::delegation::spawner::ResumeBindingFacts {
            external_session_id: state.external_id.clone(),
            cwd,
            config_fingerprint: Some(config_fingerprint),
        })
    }
}
