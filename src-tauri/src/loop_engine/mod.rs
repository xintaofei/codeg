//! Loop engineering engine: drives each running issue through triage → refine →
//! design → plan → implement → verify → review → finalize, autonomously.
//!
//! The engine holds cheap clones of the shared runtime handles (database,
//! connection manager, event emitter) plus a per-issue driver registry. A
//! single instance lives per process — desktop manages it as Tauri state, the
//! web/server `AppState` reuses (or builds) the same `Arc<LoopEngine>` — so a
//! trigger from either entry point drives the same drivers.
//!
//! State is DB-authoritative (§4.1a dispatch leases); the in-process registry
//! below is only a single-instance guard, never the concurrency authority.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use tokio::task::AbortHandle;

use crate::acp::manager::ConnectionManager;
use crate::db::AppDatabase;
use crate::web::event_bridge::EventEmitter;

pub mod error;
pub mod ingest;
pub mod transitions;
pub mod worktree;

pub use error::LoopError;

/// Registry entry for a running per-issue driver task. `abort` tears the task
/// down on stop/cancel; `wake` nudges it to re-tick without polling.
pub struct DriverHandle {
    pub abort: AbortHandle,
    pub wake: Arc<Notify>,
}

/// The loop engineering engine. Cheaply shareable via `Arc`; all fields are
/// either `Arc`-backed handles or cloned connection refs.
pub struct LoopEngine {
    // Read by dispatch / driver / recovery / worktree from Task 1.2 onward.
    #[allow(dead_code)]
    db: AppDatabase,
    #[allow(dead_code)]
    manager: ConnectionManager,
    #[allow(dead_code)]
    data_dir: PathBuf,
    #[allow(dead_code)]
    emitter: EventEmitter,
    /// Process-internal single-instance guard: at most one driver task per
    /// issue. NOT the concurrency authority — that is the DB dispatch lease.
    #[allow(dead_code)]
    drivers: Mutex<HashMap<i32, DriverHandle>>,
}

impl LoopEngine {
    pub fn new(
        db: AppDatabase,
        manager: ConnectionManager,
        data_dir: PathBuf,
        emitter: EventEmitter,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            manager,
            data_dir,
            emitter,
            drivers: Mutex::new(HashMap::new()),
        })
    }

    /// Start the per-issue driver task (no-op if one is already registered).
    /// The tick loop lands in Task 1.6.
    pub async fn start_issue(self: &Arc<Self>, _issue_id: i32) {}

    /// Wake a running driver to re-tick after an iteration settles or a human
    /// action lands. No-op when the issue has no driver. Implemented in Task 1.6.
    pub fn wake(&self, _issue_id: i32) {}

    /// Stop a running driver and drop its registry entry. Implemented alongside
    /// cancel in Task 1.8.
    pub async fn stop_issue(&self, _issue_id: i32) {}

    /// On boot, reconcile interrupted iterations and restart a driver for every
    /// still-`running` issue. Idempotent. Reconciliation lands in Task 1.7.
    pub async fn recover_on_boot(self: &Arc<Self>) {}
}
