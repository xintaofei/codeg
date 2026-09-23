/// Core memory backend trait and data types.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{MemoryRel, MemoryScope};

/// Memory backend error.
#[derive(Debug, Clone)]
pub struct MemoryError(pub String);

impl From<String> for MemoryError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for MemoryError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Provenance of a memory entry: where it came from and whether it's verified.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryProvenance {
    pub run_id: Option<i32>,
    pub step_id: Option<String>,
    pub agent_type: Option<String>,
    pub verified_by_tests: bool,
    pub source: String, // "agent" | "auto" | "user"
}

/// A memory entry (node in the graph).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryNode {
    pub id: i32,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub scope: MemoryScope,
    pub folder_id: Option<i32>,
    pub provenance: MemoryProvenance,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub stale_at: Option<DateTime<Utc>>,
}

/// New memory entry to write (without id, timestamps).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewMemoryNode {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub scope: MemoryScope,
    pub folder_id: Option<i32>,
    pub provenance: MemoryProvenance,
}

/// Search hit with score and path via edges.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryHit {
    pub node: MemoryNode,
    pub score: f64,
    pub via: Vec<i32>, // path of node ids (edges ≤2 hops)
}

/// Core memory backend trait: write, search, link, get.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Write a new memory entry and return its id.
    async fn write(&self, node: NewMemoryNode) -> Result<i32, MemoryError>;

    /// Search for entries matching the query within a scope (BM25 + expand by edges).
    /// Returns up to `limit` entries (≤50), each with a score and path via ≤2-hop edges.
    /// Results total ≤8000 characters.
    async fn search(
        &self,
        q: &str,
        scope: MemoryScope,
        folder_id: Option<i32>,
        limit: usize,
    ) -> Result<Vec<MemoryHit>, MemoryError>;

    /// Link two entries with a relationship.
    async fn link(&self, from: i32, to: i32, rel: MemoryRel) -> Result<(), MemoryError>;

    /// Get a single entry by id.
    async fn get(&self, id: i32) -> Result<Option<MemoryNode>, MemoryError>;

    /// Delete a memory entry (hard delete, not staleness).
    async fn delete(&self, id: i32) -> Result<(), MemoryError>;
}
