//! Listener-facing access for the `memory_write` / `memory_search` /
//! `memory_link` MCP tools, gated by the `memory` companion feature group
//! (see `acp::delegation::companion::CompanionFeatures`). Mirrors
//! [`crate::acp::work_task_tools::WorkTaskToolAccess`]: the listener resolves
//! the caller's parent connection from its per-launch token and hands the
//! call here.
//!
//! A production memory backend implementation doesn't exist yet, and neither do
//! its `MemoryNode` / `MemoryHit` model types — so this trait uses its own
//! minimal [`MemoryToolHit`] shape to decouple from the backend. Once a real
//! backend is implemented, these types can be reconciled. For now, a rejecting
//! stub is wired since there is no backend available.

use async_trait::async_trait;
use serde::Serialize;

/// Ack for a `memory_write` / `memory_link` call.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryToolAck {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl MemoryToolAck {
    pub fn written(id: i32) -> Self {
        Self {
            ok: true,
            id: Some(id),
            note: None,
        }
    }

    pub fn linked() -> Self {
        Self {
            ok: true,
            id: None,
            note: None,
        }
    }

    pub fn rejected(note: &str) -> Self {
        Self {
            ok: false,
            id: None,
            note: Some(note.to_string()),
        }
    }
}

/// One `memory_search` hit. Deliberately not `crate::memory::MemoryHit` (see
/// module docs) — a minimal stand-in until that module exists.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryToolHit {
    pub id: i32,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub score: f64,
}

#[async_trait]
pub trait MemoryToolAccess: Send + Sync {
    /// Write a memory entry of `kind` for the caller driven by
    /// `parent_connection_id`. `links` are `(to_id, rel)` pairs, `rel` one of
    /// `caused_by | fixed_by | relates_to | part_of | supersedes`.
    /// `user_requested` indicates whether the user explicitly asked to save this
    /// entry, required for on_request memory kind modes.
    #[allow(clippy::too_many_arguments)]
    async fn write(
        &self,
        parent_connection_id: &str,
        kind: &str,
        title: &str,
        body: &str,
        links: &[(i32, String)],
        user_requested: bool,
    ) -> MemoryToolAck;

    /// Search the memory graph visible to `parent_connection_id`.
    async fn search(
        &self,
        parent_connection_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryToolHit>, String>;

    /// Link two existing memory entries.
    async fn link(
        &self,
        parent_connection_id: &str,
        from_id: i32,
        to_id: i32,
        rel: &str,
    ) -> MemoryToolAck;
}
