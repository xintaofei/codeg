use async_trait::async_trait;
/// MemoryToolAccess: bridges listener memory tool calls to the backend.
/// Implements access control: backend Off -> reject; kind disabled -> reject;
/// on_request -> requires user_requested flag; sanitizes secrets; persists provenance.
use std::sync::Arc;

use super::backend::{MemoryBackend, MemoryProvenance, NewMemoryNode};
use super::sanitize::sanitize_secrets;
use crate::acp::memory_tools::{MemoryToolAccess, MemoryToolAck, MemoryToolHit};
use crate::db::service::memory_kind_service;
use crate::db::AppDatabase;
use crate::models::{MemoryBackendKind, MemoryMode, MemoryRel, MemoryScope, MemorySettings};

/// Request to write a memory entry with full context (provenance, user_requested flag, folder_id).
#[derive(Debug, Clone)]
pub struct MemoryWriteRequest<'a> {
    pub parent_connection_id: &'a str,
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub links: &'a [(i32, String)],
    pub user_requested: bool,
    pub provenance: Option<MemoryProvenance>,
    pub folder_id: Option<i32>,
}

/// Pluggable access layer that wraps a backend and enforces access control and rules.
pub struct MemoryBackendAccess {
    db: Option<AppDatabase>,
    backend: Option<Arc<dyn MemoryBackend>>,
    settings: Option<MemorySettings>,
}

impl MemoryBackendAccess {
    pub fn new(backend: Box<dyn MemoryBackend>) -> Self {
        Self {
            db: None,
            backend: Some(Arc::from(backend)),
            settings: None,
        }
    }

    pub fn with_arc(backend: Arc<dyn MemoryBackend>) -> Self {
        Self {
            db: None,
            backend: Some(backend),
            settings: None,
        }
    }

    pub fn with_db_and_backend(
        db: AppDatabase,
        backend: Option<Arc<dyn MemoryBackend>>,
        settings: Option<MemorySettings>,
    ) -> Self {
        Self {
            db: Some(db),
            backend,
            settings,
        }
    }

    /// Core entry point for writing memory entries with access control, validation, and secret sanitization.
    pub async fn write_entry(&self, req: MemoryWriteRequest<'_>) -> MemoryToolAck {
        // 1. Check if backend is configured and not Off
        if let Some(settings) = &self.settings {
            if settings.backend == MemoryBackendKind::Off {
                return MemoryToolAck::rejected("memory backend is off");
            }
        }
        let Some(backend) = &self.backend else {
            return MemoryToolAck::rejected("memory backend is off");
        };

        // 2. Validate memory kind mode and enabled status
        if let Some(db) = &self.db {
            let kinds = match memory_kind_service::list(&db.conn).await {
                Ok(k) => k,
                Err(e) => {
                    return MemoryToolAck::rejected(&format!(
                        "db error querying memory kinds: {}",
                        e
                    ))
                }
            };

            let matched = kinds.into_iter().find(|k| k.key == req.kind);
            let Some(kind_info) = matched else {
                return MemoryToolAck::rejected(&format!(
                    "memory kind '{}' is not registered",
                    req.kind
                ));
            };

            if !kind_info.enabled || kind_info.mode == MemoryMode::Off {
                return MemoryToolAck::rejected(&format!("memory kind '{}' is disabled", req.kind));
            }

            if kind_info.mode == MemoryMode::OnRequest && !req.user_requested {
                return MemoryToolAck::rejected(&format!(
                    "memory kind '{}' is on_request; user request flag is required",
                    req.kind
                ));
            }
        } else {
            // Default built-in kinds behavior when DB is not injected (e.g. unit tests with mock backend)
            let on_request_kinds = ["task_summary", "preference"];
            let off_kinds = ["disabled_kind"];
            if off_kinds.contains(&req.kind) {
                return MemoryToolAck::rejected(&format!("memory kind '{}' is disabled", req.kind));
            }
            if on_request_kinds.contains(&req.kind) && !req.user_requested {
                return MemoryToolAck::rejected(&format!(
                    "memory kind '{}' is on_request; user request flag is required",
                    req.kind
                ));
            }
        }

        // 3. Sanitize secrets from title and body
        let (clean_title, _) = sanitize_secrets(req.title);
        let (clean_body, _) = sanitize_secrets(req.body);

        // 4. Resolve scope & folder_id
        let scope = self
            .settings
            .as_ref()
            .map(|s| s.scope)
            .unwrap_or(MemoryScope::Global);
        let folder_id = if scope == MemoryScope::Project {
            req.folder_id
        } else {
            None
        };

        // 5. Construct provenance
        let provenance = req.provenance.unwrap_or_else(|| MemoryProvenance {
            run_id: None,
            step_id: None,
            agent_type: None,
            verified_by_tests: false,
            source: if req.user_requested {
                "user".to_string()
            } else {
                "agent".to_string()
            },
        });

        // 6. Write to backend
        let new_node = NewMemoryNode {
            kind: req.kind.to_string(),
            title: clean_title,
            body: clean_body,
            scope,
            folder_id,
            provenance,
        };

        let node_id = match backend.write(new_node).await {
            Ok(id) => id,
            Err(e) => return MemoryToolAck::rejected(&e.0),
        };

        // 7. Process any links
        for (to_id, rel_str) in req.links {
            let rel = match rel_str.as_str() {
                "caused_by" => MemoryRel::CausedBy,
                "fixed_by" => MemoryRel::FixedBy,
                "relates_to" => MemoryRel::RelatesTo,
                "part_of" => MemoryRel::PartOf,
                "supersedes" => MemoryRel::Supersedes,
                _ => continue,
            };
            let _ = backend.link(node_id, *to_id, rel).await;
        }

        MemoryToolAck::written(node_id)
    }
}

#[async_trait]
impl MemoryToolAccess for MemoryBackendAccess {
    async fn write(
        &self,
        parent_connection_id: &str,
        kind: &str,
        title: &str,
        body: &str,
        links: &[(i32, String)],
        user_requested: bool,
    ) -> MemoryToolAck {
        self.write_entry(MemoryWriteRequest {
            parent_connection_id,
            kind,
            title,
            body,
            links,
            user_requested,
            provenance: None,
            folder_id: None,
        })
        .await
    }

    async fn search(
        &self,
        _parent_connection_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryToolHit>, String> {
        let Some(backend) = &self.backend else {
            return Ok(vec![]);
        };
        let scope = self
            .settings
            .as_ref()
            .map(|s| s.scope)
            .unwrap_or(MemoryScope::Global);

        let hits = backend
            .search(query, scope, None, limit.min(50))
            .await
            .map_err(|e| e.0)?;

        let tool_hits = hits
            .into_iter()
            .map(|hit| MemoryToolHit {
                id: hit.node.id,
                kind: hit.node.kind,
                title: hit.node.title,
                body: hit.node.body,
                score: hit.score,
            })
            .collect();

        Ok(tool_hits)
    }

    async fn link(
        &self,
        _parent_connection_id: &str,
        from_id: i32,
        to_id: i32,
        rel: &str,
    ) -> MemoryToolAck {
        let Some(backend) = &self.backend else {
            return MemoryToolAck::rejected("memory backend is off");
        };

        let rel = match rel {
            "caused_by" => MemoryRel::CausedBy,
            "fixed_by" => MemoryRel::FixedBy,
            "relates_to" => MemoryRel::RelatesTo,
            "part_of" => MemoryRel::PartOf,
            "supersedes" => MemoryRel::Supersedes,
            _ => return MemoryToolAck::rejected("unknown relationship type"),
        };

        match backend.link(from_id, to_id, rel).await {
            Ok(_) => MemoryToolAck::linked(),
            Err(e) => MemoryToolAck::rejected(&e.0),
        }
    }
}
