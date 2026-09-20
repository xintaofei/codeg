//! Listener-facing access for the `pipeline_verdict` MCP tool, injected only
//! into spawns launched by the pipeline engine for a Reviewer/Tests step
//! (gated by the `pipeline` companion feature group — see
//! `acp::delegation::companion::CompanionFeatures`). Mirrors
//! [`crate::acp::work_task_tools::WorkTaskToolAccess`]: the listener resolves
//! the caller's parent connection from its per-launch token and hands the
//! verdict here. A production pipeline engine implementation would map the
//! connection to the running attempt and record it via
//! `pipeline_service::cas_attempt_status`. Kept as a trait so the listener
//! stays decoupled from the engine (and tests can stub it); for now a
//! rejecting stub is wired since the engine is not yet available.

use async_trait::async_trait;

pub use crate::acp::work_task_tools::TaskReportAck;

#[async_trait]
pub trait PipelineToolAccess: Send + Sync {
    /// Record the verdict (+ optional notes) for the pipeline step attempt
    /// driven by `parent_connection_id`. `verdict` is one of
    /// `pass` | `changes_requested` | `inconclusive` — validated by the
    /// companion against the MCP schema before this is ever called.
    async fn record_verdict(
        &self,
        parent_connection_id: &str,
        verdict: &str,
        notes: Option<&str>,
    ) -> TaskReportAck;
}
