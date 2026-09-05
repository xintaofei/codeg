//! Domain / protocol types for the strict recovery boundary.
//!
//! `SessionRecoveryPolicy` names the two recovery strategies; the
//! [`StrictAttachGate`] carries the `RequireExisting` verdict channel through
//! the spawn path so `run_connection` can report a REAL readiness (or a typed
//! failure) instead of the implicit "a connection id exists" signal.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;

/// Which recovery behavior a connection launch uses.
///
/// * `AllowNewFallback` — the ordinary spawn chain (resume → load → **new**):
///   a failed recovery of a session the caller merely suggested may legally
///   produce a brand-new agent session. This is the production default for
///   every pre-existing entry point and must stay untouched.
/// * `RequireExisting` — the strict boundary: only `session/resume` /
///   `session/load` against the caller-supplied external session id may
///   establish the session; a failure is a typed error and **never** falls
///   through to `session/new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoveryPolicy {
    AllowNewFallback,
    RequireExisting,
}

/// The launch-time recovery strategy handed to `run_connection`: the policy
/// plus, for the strict one, the gate that receives the readiness verdict.
#[derive(Debug)]
pub enum SessionRecovery {
    AllowNewFallback,
    RequireExisting(StrictAttachGate),
}

impl SessionRecovery {
    /// Whether this launch is allowed to mint a new agent session when
    /// recovery fails. Every `session/new` call site must consult this BEFORE
    /// sending the request — that ordering is the whole strict guarantee.
    pub fn allows_new_fallback(&self) -> bool {
        matches!(self, SessionRecovery::AllowNewFallback)
    }
}

/// Typed strict-recovery failures. Any of these means NO usable session was
/// established and nothing may be sent over the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrictAttachErrorCode {
    /// The recorded binding (cwd / execution-config identity) does not match
    /// what this launch would actually run, or is not reliably verifiable.
    BindingMismatch,
    /// The agent (or its recorded capabilities) supports neither
    /// `session/resume` nor `session/load` — there is no existing session to
    /// attach to.
    ResumeUnsupported,
    /// Both recovery methods were tried and failed, or a required setup step
    /// (config application) failed after recovery.
    ResumeFailed,
    /// The recovery handshake never completed within the allowed time.
    ResumeTimeout,
}

/// A successful strict attach: the connection is bound to the requested
/// external session, recovery + replay drain + config application all
/// succeeded, and the connection is ready for the caller's FIRST prompt.
/// Returned readiness is the ONLY signal a strict caller may act on —
/// `Connected` / `SessionStarted` events fire earlier and prove nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrictReady {
    pub connection_id: String,
    pub external_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code:?}: {message}")]
pub struct StrictAttachError {
    pub code: StrictAttachErrorCode,
    pub message: String,
}

impl StrictAttachError {
    pub fn new(code: StrictAttachErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// One-shot verdict channel for a strict launch. The sender half lives with
/// the connection; the receiver is what the strict caller awaits (bounded by
/// its own timeout → [`StrictAttachErrorCode::ResumeTimeout`]).
///
/// Both [`Self::ready`] and [`Self::fail`] are idempotent: whichever verdict
/// lands first wins and later calls are logged no-ops, mirroring the
/// first-terminal-wins rule the rest of the delegation machinery follows.
#[derive(Clone)]
pub struct StrictAttachGate {
    tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<StrictOutcome>>>>,
    policy: SessionRecoveryPolicy,
    /// The canonical cwd the outcome's binding recorded. A launch resolving to
    /// any other directory is refused BEFORE the agent process starts.
    expected_cwd: PathBuf,
    /// The execution-config identity the outcome's binding recorded (the
    /// canonical config fingerprint captured at the original success). A
    /// launch whose freshly computed fingerprint differs is refused — codeg
    /// must not silently move a continued session onto a changed environment.
    expected_config_fingerprint: String,
}

/// What the gate's receiver resolves to.
#[derive(Debug)]
pub enum StrictOutcome {
    Ready(StrictReady),
    Failed(StrictAttachError),
}

impl std::fmt::Debug for StrictAttachGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrictAttachGate")
            .field("policy", &self.policy)
            .field("expected_cwd", &self.expected_cwd)
            // The fingerprint is a non-sensitive identity hash, but it is
            // still a config identifier — show only its presence/length.
            .field(
                "expected_config_fingerprint",
                &self.expected_config_fingerprint.len(),
            )
            .finish()
    }
}

impl StrictAttachGate {
    /// Create the gate + receiver pair. `expected_cwd` /
    /// `expected_config_fingerprint` come from the frozen outcome's resume
    /// binding and are verified against the actual launch before the agent
    /// process starts.
    pub fn channel(
        expected_cwd: PathBuf,
        expected_config_fingerprint: String,
    ) -> (Self, oneshot::Receiver<StrictOutcome>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                tx: Arc::new(tokio::sync::Mutex::new(Some(tx))),
                policy: SessionRecoveryPolicy::RequireExisting,
                expected_cwd,
                expected_config_fingerprint,
            },
            rx,
        )
    }

    /// Verify the launch parameters against the recorded binding. Runs in
    /// `spawn_agent_connection` BEFORE the agent process starts, so a binding
    /// mismatch never even boots a binary — and therefore can never fall back
    /// to anything.
    pub fn verify_launch(
        &self,
        launch_cwd: &std::path::Path,
        config_fingerprint: &str,
    ) -> Result<(), StrictAttachError> {
        if !self.expected_cwd.as_path().eq(launch_cwd) {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::BindingMismatch,
                format!(
                    "recorded cwd {:?} does not match the launch cwd {:?}; refusing to \
                     continue the session elsewhere",
                    self.expected_cwd, launch_cwd
                ),
            ));
        }
        if self.expected_config_fingerprint != config_fingerprint {
            return Err(StrictAttachError::new(
                StrictAttachErrorCode::BindingMismatch,
                "the execution configuration (environment / model provider config) \
                 changed since the recorded success; refusing to continue the session \
                 under a different configuration"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Deliver the Ready verdict (after recovery + drain + config all truly
    /// succeeded). A no-op when a verdict already landed.
    pub async fn ready(&self, ready: StrictReady) {
        let mut slot = self.tx.lock().await;
        if let Some(tx) = slot.take() {
            let _ = tx.send(StrictOutcome::Ready(ready));
        } else {
            tracing::warn!("[strict-attach] ready verdict arrived after resolution; ignored");
        }
    }

    /// Deliver a failure verdict. A no-op when a verdict already landed.
    pub async fn fail(&self, err: StrictAttachError) {
        let mut slot = self.tx.lock().await;
        if let Some(tx) = slot.take() {
            let _ = tx.send(StrictOutcome::Failed(err));
        } else {
            tracing::warn!("[strict-attach] failure verdict arrived after resolution; ignored");
        }
    }
}
