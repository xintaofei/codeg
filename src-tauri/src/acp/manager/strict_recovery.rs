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
    /// exit through both the owned-child reaper callback and the driver-exit
    /// guard. That joint proof makes release a fact rather than an assertion.
    pub async fn disconnect_and_reclaim(&self, conn_id: &str) -> Result<(), AcpError> {
        self.disconnect_and_reclaim_with_timing(
            conn_id,
            Duration::from_millis(500),
            Duration::from_secs(5),
        )
        .await
    }

    pub(super) async fn disconnect_and_reclaim_with_timing(
        &self,
        conn_id: &str,
        grace: Duration,
        confirmation_timeout: Duration,
    ) -> Result<(), AcpError> {
        let Some(resource) = self.resources.get_and_retain(conn_id).await else {
            return Err(AcpError::ConnectionNotFound(conn_id.into()));
        };
        tracing::info!("[ACP] disconnect_and_reclaim connection={}", conn_id);
        let _ = resource.cmd_tx.try_send(ConnectionCommand::Disconnect);

        if tokio::time::timeout(grace, resource.lifetime.wait_until_released())
            .await
            .is_ok()
        {
            return self.finish_confirmed_reclaim(&resource).await;
        }

        let deadline = tokio::time::Instant::now() + confirmation_timeout;
        let mut forced_pid = None;
        loop {
            let revision = resource.lifetime.revision();
            if resource.lifetime.release_confirmed() {
                return self.finish_confirmed_reclaim(&resource).await;
            }

            if forced_pid.is_none() && resource.lifetime.current_pid().is_some() {
                let lifetime = Arc::clone(&resource.lifetime);
                let kill_result =
                    tokio::task::spawn_blocking(move || lifetime.force_kill_current()).await;
                match kill_result {
                    Ok(Some((pid, Ok(_)))) => {
                        forced_pid = Some(pid);
                        tracing::info!(
                            "[ACP] disconnect_and_reclaim forced process tree pid={pid}"
                        );
                    }
                    Ok(Some((pid, Err(e)))) => {
                        forced_pid = Some(pid);
                        tracing::debug!(
                            "[ACP] disconnect_and_reclaim force raced exit pid={pid}: {e}"
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("[ACP] disconnect_and_reclaim force worker failed: {e}")
                    }
                }
            }

            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(AcpError::protocol(format!(
                    "connection {conn_id}: release was not confirmed by both the driver and \
                     owned process reaper within {}ms; retained for retry",
                    confirmation_timeout.as_millis()
                )));
            }
            let remaining = deadline - now;
            let _ =
                tokio::time::timeout(remaining, resource.lifetime.wait_for_change_since(revision))
                    .await;
        }
    }

    pub(super) async fn finish_confirmed_reclaim(
        &self,
        resource: &Arc<crate::acp::connection::lifetime::ConnectionResource>,
    ) -> Result<(), AcpError> {
        debug_assert!(resource.lifetime.release_confirmed());
        {
            let mut active = self.connections.lock().await;
            if active
                .get(&resource.connection_id)
                .is_some_and(|conn| Arc::ptr_eq(&conn.state, &resource.state))
            {
                active.remove(&resource.connection_id);
            }
        }
        self.resources
            .remove_exact(&resource.connection_id, resource)
            .await;
        tracing::info!(
            "[ACP] disconnect_and_reclaim confirmed driver exit and process reap connection={}",
            resource.connection_id
        );
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
        use crate::acp::connection::spawn_agent_connection_with_transport_managed;
        use crate::acp::delegation::continuation::{
            SessionRecovery, StrictAttachError, StrictAttachErrorCode, StrictOutcome,
        };

        // Share ordinary spawn's exclusion against restoring agent transcripts.
        let _restore_guard = self.external_restore_lock.read().await;

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

        // Serialize exact-target inspection, registration, verdict, and any
        // immediate cleanup with ordinary resume launches. Requested external
        // identity in the retained resource covers the pre-SessionStarted gap.
        let _target_guard = self
            .lock_spawn_target(agent_type, launch_cwd.clone(), external_session_id.clone())
            .await;
        if let Some(existing) = self
            .resources
            .find_target(agent_type, &launch_cwd, &external_session_id)
            .await
        {
            if crate::acp::connection::lifetime::ConnectionResourceRegistry::active_entry_is_exact(
                &self.connections,
                &existing,
            )
            .await
            {
                return Err(StrictAttachError::new(
                    StrictAttachErrorCode::ResumeFailed,
                    "a live connection already hosts this session; a continuation \
                     round must not share it",
                ));
            }
            let retained = self.resources.retain_exact(&existing).await;
            if !retained && !existing.lifetime.release_confirmed() {
                return Err(StrictAttachError::new(
                    StrictAttachErrorCode::ResumeFailed,
                    "a previous connection changed while its release was being inspected",
                ));
            } else if retained && !existing.lifetime.release_confirmed() {
                if let Err(cleanup_err) = self.disconnect_and_reclaim(&existing.connection_id).await
                {
                    return Err(StrictAttachError::new(
                        StrictAttachErrorCode::ResumeFailed,
                        "a previous connection for this session is still being reclaimed",
                    )
                    .with_retained_cleanup(
                        existing.connection_id.clone(),
                        cleanup_err.to_string(),
                    ));
                }
            } else if retained {
                self.finish_confirmed_reclaim(&existing)
                    .await
                    .map_err(|e| {
                        StrictAttachError::new(
                            StrictAttachErrorCode::ResumeFailed,
                            "a previous connection for this session could not be retired",
                        )
                        .with_retained_cleanup(existing.connection_id.clone(), e.to_string())
                    })?;
            }
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
        let spawn_result = spawn_agent_connection_with_transport_managed(
            agent,
            connection_id.clone(),
            agent_type,
            Some(working_dir),
            Some(external_session_id),
            runtime_env,
            owner_window_label,
            emitter,
            self.connections.clone(),
            self.resources.clone(),
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
            let primary = match tokio::time::timeout(Duration::from_secs(1), &mut verdict_rx).await
            {
                Ok(Ok(StrictOutcome::Failed(e))) => e,
                _ => StrictAttachError::new(
                    StrictAttachErrorCode::ResumeFailed,
                    format!("strict attach failed to launch: {spawn_err}"),
                ),
            };
            return Err(self.cleanup_strict_failure(&connection_id, primary).await);
        }

        // --- Await the typed verdict (bounded) --------------------------------
        match tokio::time::timeout(timeout, &mut verdict_rx).await {
            Ok(Ok(StrictOutcome::Ready(ready))) => Ok(ready),
            Ok(Ok(StrictOutcome::Failed(e))) => {
                Err(self.cleanup_strict_failure(&connection_id, e).await)
            }
            // The connection ended without ever reporting readiness — treat it
            // as a failed recovery, never as success.
            Ok(Err(_receiver_dropped)) => Err(self
                .cleanup_strict_failure(
                    &connection_id,
                    StrictAttachError::new(
                        StrictAttachErrorCode::ResumeFailed,
                        "the connection ended before reporting strict readiness",
                    ),
                )
                .await),
            Err(_elapsed) => {
                // Timed out waiting for the handshake. The driver is parked
                // in `block_task()` and NEVER reads the queued Disconnect,
                // so the reclaim must go through the forced ladder —
                // graceful command, grace window, hard `kill_tree`, confirmed
                // exit — or the agent process would outlive its registration
                // and block every future strict resume of the same session
                // (acceptance F9, reacceptance R7). A late Ready lands on a
                // dead connection and is harmless.
                let primary = StrictAttachError::new(
                    StrictAttachErrorCode::ResumeTimeout,
                    format!(
                        "strict attach did not become ready within {}ms; no prompt was sent",
                        timeout.as_millis()
                    ),
                );
                Err(self.cleanup_strict_failure(&connection_id, primary).await)
            }
        }
    }

    pub(super) async fn cleanup_strict_failure(
        &self,
        connection_id: &str,
        primary: crate::acp::delegation::continuation::StrictAttachError,
    ) -> crate::acp::delegation::continuation::StrictAttachError {
        match self.disconnect_and_reclaim(connection_id).await {
            Ok(()) => primary,
            Err(cleanup_err) => {
                if self.resources.get(connection_id).await.is_some() {
                    primary.with_retained_cleanup(connection_id, cleanup_err.to_string())
                } else {
                    // Registration can fail before a driver/resource exists.
                    primary
                }
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
