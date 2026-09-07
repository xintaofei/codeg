use std::sync::Arc;

use sacp_tokio::AcpAgent;

/// Transport hook for the OS-pid observers the manager stores on the
/// connection entry (shutdown kill-tree backstop). A real process transport
/// publishes its pid on spawn and clears it on reap; an in-memory test
/// transport has no process and does nothing.
pub trait PidObservable: Sized {
    fn observe_process_pid(self, _pid: Arc<std::sync::atomic::AtomicU32>) -> Self {
        self
    }
}

impl PidObservable for AcpAgent {
    fn observe_process_pid(self, pid: Arc<std::sync::atomic::AtomicU32>) -> Self {
        self.on_spawn({
            let cell = Arc::clone(&pid);
            move |os_pid| cell.store(os_pid, std::sync::atomic::Ordering::SeqCst)
        })
        // Paired with `on_spawn`: publish 0 again once the process has been
        // reaped, so the shutdown backstop can never `kill_tree` a pid the OS
        // has already handed to someone else. Fires ONLY on a real reap — a
        // connection that merely ended keeps its pid published, because the
        // vendored `ChildGuard` signals the tree without waiting and the agent
        // may still be running.
        .on_exit(move || pid.store(0, std::sync::atomic::Ordering::SeqCst))
    }
}
