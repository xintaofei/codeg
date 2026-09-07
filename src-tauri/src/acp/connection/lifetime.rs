use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, Notify, RwLock};

use crate::acp::delegation::continuation::ContinuationConnectionIdentity;
use crate::acp::session_state::SessionState;
use crate::models::agent::AgentType;

use super::{AgentConnection, ConnectionCommand};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessPhase {
    NotSpawned,
    Spawned(u32),
    Reaped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LifetimeState {
    process: ProcessPhase,
    driver_exited: bool,
    revision: u64,
}

/// Actual release evidence for one connection driver and its owned OS child.
///
/// A zero/absent pid is deliberately not a release signal while the driver can
/// still enter the transport and spawn. Release requires the driver to have
/// exited and either proof that it never spawned or the vendored Child reaper's
/// callback after a successful wait.
#[doc(hidden)]
pub struct ConnectionProcessLifetime {
    state: Mutex<LifetimeState>,
    changed: Notify,
    reap_gate: Arc<Mutex<()>>,
}

impl ConnectionProcessLifetime {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(LifetimeState {
                process: ProcessPhase::NotSpawned,
                driver_exited: false,
                revision: 0,
            }),
            changed: Notify::new(),
            reap_gate: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) fn mark_driver_exited(&self) {
        self.update(|state| state.driver_exited = true);
    }

    pub(crate) fn mark_spawned(&self, pid: u32) {
        self.update(|state| state.process = ProcessPhase::Spawned(pid));
    }

    pub(crate) fn mark_reaped(&self) {
        self.update(|state| state.process = ProcessPhase::Reaped);
    }

    fn update(&self, mutate: impl FnOnce(&mut LifetimeState)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        mutate(&mut state);
        state.revision = state.revision.wrapping_add(1);
        drop(state);
        self.changed.notify_waiters();
    }

    pub(crate) fn current_pid(&self) -> Option<u32> {
        match self.state.lock().unwrap_or_else(|e| e.into_inner()).process {
            ProcessPhase::Spawned(pid) => Some(pid),
            ProcessPhase::NotSpawned | ProcessPhase::Reaped => None,
        }
    }

    pub(crate) fn release_confirmed(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.driver_exited
            && matches!(
                state.process,
                ProcessPhase::NotSpawned | ProcessPhase::Reaped
            )
    }

    pub(crate) fn revision(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revision
    }

    pub(crate) async fn wait_for_change_since(&self, revision: u64) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.revision() != revision {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn reap_gate(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.reap_gate)
    }

    pub(crate) fn force_kill_current(
        &self,
    ) -> Option<(u32, kill_tree::Result<kill_tree::Outputs>)> {
        // The vendored wait path takes this same gate around try_wait. Holding
        // it across PID sampling and kill_tree prevents Child::wait from
        // reaping/freeing the owned PID between those actions.
        let _reap_guard = self.reap_gate.lock().unwrap_or_else(|e| e.into_inner());
        let pid = self.current_pid()?;
        let config = kill_tree::Config {
            signal: "SIGKILL".to_string(),
            ..Default::default()
        };
        Some((
            pid,
            kill_tree::blocking::kill_tree_with_config(pid, &config),
        ))
    }

    pub(crate) async fn wait_until_released(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.release_confirmed() {
                return;
            }
            notified.await;
        }
    }
}

/// Control and identity retained after the user-visible active-map entry is
/// removed. The manager releases this record only after real lifetime proof.
pub(crate) struct ConnectionResource {
    pub(crate) connection_id: String,
    pub(crate) cmd_tx: mpsc::Sender<ConnectionCommand>,
    pub(crate) lifetime: Arc<ConnectionProcessLifetime>,
    pub(crate) state: Arc<RwLock<SessionState>>,
    pub(crate) agent_type: AgentType,
    pub(crate) launch_cwd: PathBuf,
    pub(crate) requested_external_id: Option<String>,
    pub(crate) continuation_identity: Option<ContinuationConnectionIdentity>,
    retained: AtomicBool,
}

impl ConnectionResource {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        connection_id: String,
        cmd_tx: mpsc::Sender<ConnectionCommand>,
        lifetime: Arc<ConnectionProcessLifetime>,
        state: Arc<RwLock<SessionState>>,
        agent_type: AgentType,
        launch_cwd: PathBuf,
        requested_external_id: Option<String>,
        continuation_identity: Option<ContinuationConnectionIdentity>,
        retained: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            connection_id,
            cmd_tx,
            lifetime,
            state,
            agent_type,
            launch_cwd,
            requested_external_id,
            continuation_identity,
            retained: AtomicBool::new(retained),
        })
    }

    fn retain(&self) {
        self.retained.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_retained(&self) -> bool {
        self.retained.load(Ordering::SeqCst)
    }

    pub(crate) async fn matches_target(
        &self,
        agent_type: AgentType,
        cwd: &Path,
        external_id: &str,
    ) -> bool {
        if self.agent_type != agent_type || self.launch_cwd != cwd {
            return false;
        }
        let observed = self.state.read().await.external_id.clone();
        observed
            .as_deref()
            .or(self.requested_external_id.as_deref())
            == Some(external_id)
    }
}

#[derive(Clone, Default)]
pub(crate) struct ConnectionResourceRegistry {
    resources: Arc<tokio::sync::Mutex<HashMap<String, Arc<ConnectionResource>>>>,
}

impl ConnectionResourceRegistry {
    pub(crate) async fn insert(&self, resource: Arc<ConnectionResource>) {
        self.resources
            .lock()
            .await
            .insert(resource.connection_id.clone(), resource);
    }

    pub(crate) async fn get(&self, connection_id: &str) -> Option<Arc<ConnectionResource>> {
        self.resources.lock().await.get(connection_id).cloned()
    }

    pub(crate) async fn get_and_retain(
        &self,
        connection_id: &str,
    ) -> Option<Arc<ConnectionResource>> {
        let resources = self.resources.lock().await;
        let resource = resources.get(connection_id)?.clone();
        resource.retain();
        Some(resource)
    }

    pub(crate) async fn retain_exact(&self, resource: &Arc<ConnectionResource>) -> bool {
        let resources = self.resources.lock().await;
        let exact = resources
            .get(&resource.connection_id)
            .is_some_and(|found| Arc::ptr_eq(found, resource));
        if exact {
            resource.retain();
        }
        exact
    }

    pub(crate) async fn remove_exact(
        &self,
        connection_id: &str,
        expected: &Arc<ConnectionResource>,
    ) -> bool {
        let mut resources = self.resources.lock().await;
        let matches = resources
            .get(connection_id)
            .is_some_and(|found| Arc::ptr_eq(found, expected));
        if matches {
            resources.remove(connection_id);
        }
        matches
    }

    pub(crate) async fn find_target(
        &self,
        agent_type: AgentType,
        cwd: &Path,
        external_id: &str,
    ) -> Option<Arc<ConnectionResource>> {
        let snapshot: Vec<_> = self.resources.lock().await.values().cloned().collect();
        for resource in snapshot {
            if resource.matches_target(agent_type, cwd, external_id).await {
                return Some(resource);
            }
        }
        None
    }

    pub(crate) async fn active_entry_is_exact(
        connections: &Arc<tokio::sync::Mutex<HashMap<String, AgentConnection>>>,
        resource: &Arc<ConnectionResource>,
    ) -> bool {
        connections
            .lock()
            .await
            .get(&resource.connection_id)
            .is_some_and(|conn| Arc::ptr_eq(&conn.state, &resource.state))
    }

    pub(crate) fn retire_when_confirmed(&self, resource: Arc<ConnectionResource>) {
        let registry = self.clone();
        tokio::spawn(async move {
            resource.lifetime.wait_until_released().await;
            let mut resources = registry.resources.lock().await;
            let exact = resources
                .get(&resource.connection_id)
                .is_some_and(|found| Arc::ptr_eq(found, &resource));
            if exact && !resource.is_retained() {
                resources.remove(&resource.connection_id);
            }
        });
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.resources.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn release_requires_driver_exit_and_real_process_reap() {
        let lifetime = ConnectionProcessLifetime::new();
        assert!(!lifetime.release_confirmed());

        lifetime.mark_driver_exited();
        assert!(
            lifetime.release_confirmed(),
            "exited + never spawned is proof"
        );

        let lifetime = ConnectionProcessLifetime::new();
        lifetime.mark_spawned(42);
        lifetime.mark_driver_exited();
        assert!(!lifetime.release_confirmed());
        lifetime.mark_reaped();
        assert!(lifetime.release_confirmed());
    }
}
