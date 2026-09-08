//! `ChildLiveReplyLookup` — broker capability that peeks at a still-running
//! delegation child's **in-memory** session to pull a coherent snapshot of its
//! latest reply, real blocking prompt, and last agent-output time. The broker
//! uses it to enrich `get_delegation_status` without treating silence or elapsed
//! time as proof of progress. Content-only hosts still anchor running status to
//! the standalone first line `"Running."`.
//!
//! Kept behind a trait for the same reason as [`super::meta_writer`] /
//! [`super::broker::ChildStatusLookup`]: the broker stays decoupled from
//! `ConnectionManager`, and unit tests can inject a deterministic reply without
//! a live ACP session. Production wires [`ConnectionManagerLiveReplyLookup`] via
//! [`super::broker::DelegationBroker::with_live_reply_lookup`].
//!
//! Lock-ordering note: the broker calls this **after** releasing its pending
//! mutex, so reading the child's `SessionState` (a separate `RwLock`) can never
//! invert lock order against the broker's own state.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

use crate::acp::delegation::types::BlockedOn;
use crate::acp::manager::ConnectionManager;

/// Char budget for inline live-reply evidence — one tidy line, not a transcript.
/// Read by [`ConnectionManagerLiveReplyLookup`] and passed to
/// [`crate::acp::SessionState::latest_live_reply`].
pub const LIVE_REPLY_CAP: usize = 120;

/// Cap for the blocking prompt's label. Same reasoning as [`LIVE_REPLY_CAP`] —
/// this is one line of a tool report, not a transcript.
pub const BLOCKED_TITLE_CAP: usize = 120;

/// Coherent point-in-time evidence from one child session-state read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChildLiveSnapshot {
    pub latest_reply: Option<String>,
    pub blocked_on: Option<BlockedOn>,
    pub last_output_at: Option<DateTime<Utc>>,
}

/// Capability the broker uses to fetch a running child's live evidence.
#[async_trait]
pub trait ChildLiveReplyLookup: Send + Sync {
    /// `None` means the connection disappeared before it could be read.
    async fn snapshot(&self, child_connection_id: &str) -> Option<ChildLiveSnapshot>;
}

/// Default lookup — always `None`. Used by `DelegationBroker::new` / `with_writers`
/// (test callsites that don't exercise the live-reply enrichment); production
/// replaces it via `with_live_reply_lookup`.
#[derive(Default, Clone)]
pub struct NoopChildLiveReplyLookup;

#[async_trait]
impl ChildLiveReplyLookup for NoopChildLiveReplyLookup {
    async fn snapshot(&self, _child_connection_id: &str) -> Option<ChildLiveSnapshot> {
        None
    }
}

/// Production impl backed by `ConnectionManager`. A missing connection (child
/// torn down between the status read and this call) collapses to `None` — the
/// report just stays `"Running."`.
#[derive(Clone)]
pub struct ConnectionManagerLiveReplyLookup {
    pub manager: Arc<ConnectionManager>,
}

#[async_trait]
impl ChildLiveReplyLookup for ConnectionManagerLiveReplyLookup {
    async fn snapshot(&self, child_connection_id: &str) -> Option<ChildLiveSnapshot> {
        let state = self.manager.get_state(child_connection_id).await?;
        let guard = state.read().await;
        Some(ChildLiveSnapshot {
            latest_reply: guard.latest_live_reply(LIVE_REPLY_CAP),
            blocked_on: guard.blocking_prompt(BLOCKED_TITLE_CAP),
            last_output_at: guard.last_output_at,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod mock {
    use super::*;

    use std::sync::Mutex;

    /// Returns a fixed reply for every lookup so broker tests can assert the
    /// running-status message composition without a live child session.
    ///
    /// `blocked` is interior-mutable so a test can flip a child into (and out
    /// of) a blocked state *while a status long-poll is parked* — which is
    /// exactly the transition `get_tasks_status` must wake on.
    #[derive(Default)]
    pub struct MockChildLiveReplyLookup {
        snapshot: Mutex<ChildLiveSnapshot>,
    }

    impl MockChildLiveReplyLookup {
        pub fn new(reply: Option<String>) -> Self {
            Self {
                snapshot: Mutex::new(ChildLiveSnapshot {
                    latest_reply: reply,
                    ..ChildLiveSnapshot::default()
                }),
            }
        }

        /// Set (or clear) what every child looks blocked on from now on.
        pub fn set_blocked(&self, blocked: Option<BlockedOn>) {
            self.snapshot.lock().unwrap().blocked_on = blocked;
        }

        pub fn set_last_output_at(&self, last_output_at: Option<DateTime<Utc>>) {
            self.snapshot.lock().unwrap().last_output_at = last_output_at;
        }
    }

    #[async_trait]
    impl ChildLiveReplyLookup for MockChildLiveReplyLookup {
        async fn snapshot(&self, _child_connection_id: &str) -> Option<ChildLiveSnapshot> {
            Some(self.snapshot.lock().unwrap().clone())
        }
    }
}
