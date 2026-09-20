/// LocalSqlite backend: FTS5 search + edge graph expansion.
use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use super::backend::{
    MemoryBackend, MemoryError, MemoryHit, MemoryNode, MemoryProvenance, NewMemoryNode,
};
use crate::models::{MemoryRel, MemoryScope};

/// SQLite-backed memory graph using FTS5 for full-text search.
pub struct LocalSqliteBackend {
    #[allow(dead_code)]
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

fn format_fts5_query(q: &str) -> String {
    let tokens: Vec<String> = q
        .split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{}\"*", s.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        String::new()
    } else {
        tokens.join(" OR ")
    }
}

fn node_matches_scope(node: &MemoryNode, scope: MemoryScope, folder_id: Option<i32>) -> bool {
    if node.scope != scope {
        return false;
    }
    if scope == MemoryScope::Project {
        if let Some(fid) = folder_id {
            if node.folder_id != Some(fid) {
                return false;
            }
        }
    }
    true
}

impl LocalSqliteBackend {
    /// Open or create the memory database at the given path.
    pub fn new(db_path: PathBuf) -> Result<Self, MemoryError> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&db_path)
            .map_err(|e| MemoryError(format!("Cannot open memory.db: {}", e)))?;

        let backend = Self {
            db_path,
            conn: Mutex::new(conn),
        };

        backend.init_schema()?;
        Ok(backend)
    }

    fn init_schema(&self) -> Result<(), MemoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MemoryError("lock poisoned".to_string()))?;

        // Enable FTS5 and other extensions.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(|e| MemoryError(format!("pragma failed: {}", e)))?;

        // Main nodes table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS mem_node (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                scope TEXT NOT NULL, -- 'project' | 'global'
                folder_id INTEGER,
                provenance TEXT NOT NULL, -- JSON
                created_at TEXT NOT NULL, -- ISO 8601 UTC
                updated_at TEXT NOT NULL,
                stale_at TEXT
            )",
            [],
        )
        .map_err(|e| MemoryError(format!("create mem_node failed: {}", e)))?;

        // Edges: relationships between nodes
        conn.execute(
            "CREATE TABLE IF NOT EXISTS mem_edge (
                from_id INTEGER NOT NULL,
                to_id INTEGER NOT NULL,
                rel TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (from_id, to_id, rel),
                FOREIGN KEY (from_id) REFERENCES mem_node(id) ON DELETE CASCADE,
                FOREIGN KEY (to_id) REFERENCES mem_node(id) ON DELETE CASCADE
            )",
            [],
        )
        .map_err(|e| MemoryError(format!("create mem_edge failed: {}", e)))?;

        // FTS5 table for full-text search
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS mem_node_fts USING fts5(
                title, body,
                content='mem_node',
                content_rowid='id'
            )",
            [],
        )
        .map_err(|e| MemoryError(format!("create FTS5 failed: {}", e)))?;

        // Triggers to keep FTS5 in sync
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS mem_node_ai AFTER INSERT ON mem_node BEGIN
                INSERT INTO mem_node_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
            END",
            [],
        )
        .map_err(|e| MemoryError(format!("create insert trigger failed: {}", e)))?;

        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS mem_node_ad AFTER DELETE ON mem_node BEGIN
                DELETE FROM mem_node_fts WHERE rowid = old.id;
            END",
            [],
        )
        .map_err(|e| MemoryError(format!("create delete trigger failed: {}", e)))?;

        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS mem_node_au AFTER UPDATE ON mem_node BEGIN
                DELETE FROM mem_node_fts WHERE rowid = old.id;
                INSERT INTO mem_node_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
            END",
            [],
        )
        .map_err(|e| MemoryError(format!("create update trigger failed: {}", e)))?;

        Ok(())
    }

    fn load_node(&self, row: &rusqlite::Row) -> Result<MemoryNode, rusqlite::Error> {
        let provenance_json: String = row.get(6)?;
        let provenance: MemoryProvenance =
            serde_json::from_str(&provenance_json).unwrap_or(MemoryProvenance {
                run_id: None,
                step_id: None,
                agent_type: None,
                verified_by_tests: false,
                source: "unknown".to_string(),
            });

        Ok(MemoryNode {
            id: row.get(0)?,
            kind: row.get(1)?,
            title: row.get(2)?,
            body: row.get(3)?,
            scope: match row.get::<_, String>(4)?.as_str() {
                "project" => MemoryScope::Project,
                _ => MemoryScope::Global,
            },
            folder_id: row.get(5)?,
            provenance,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            stale_at: row.get::<_, Option<String>>(9)?.and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            }),
        })
    }
}

#[async_trait]
impl MemoryBackend for LocalSqliteBackend {
    async fn write(&self, node: NewMemoryNode) -> Result<i32, MemoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MemoryError("lock poisoned".to_string()))?;
        let now = Utc::now().to_rfc3339();
        let provenance_json = serde_json::to_string(&node.provenance).unwrap_or_default();
        let scope_str = match node.scope {
            MemoryScope::Project => "project",
            MemoryScope::Global => "global",
        };

        // Deduplication: matching kind + title in the same scope and folder_id updates the existing node.
        let existing_id: Option<i32> = conn.query_row(
            "SELECT id FROM mem_node WHERE kind = ? AND title = ? AND scope = ? AND (? IS NULL OR folder_id = ?)",
            params![&node.kind, &node.title, scope_str, node.folder_id, node.folder_id],
            |row| row.get(0),
        ).optional().map_err(|e| MemoryError(format!("dedup query failed: {}", e)))?;

        if let Some(id) = existing_id {
            conn.execute(
                "UPDATE mem_node SET body = ?, provenance = ?, updated_at = ? WHERE id = ?",
                params![&node.body, &provenance_json, &now, id],
            )
            .map_err(|e| MemoryError(format!("update failed: {}", e)))?;
            return Ok(id);
        }

        conn.execute(
            "INSERT INTO mem_node (kind, title, body, scope, folder_id, provenance, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                &node.kind,
                &node.title,
                &node.body,
                scope_str,
                node.folder_id,
                provenance_json,
                &now,
                &now,
            ],
        ).map_err(|e| MemoryError(format!("write failed: {}", e)))?;

        let id = conn.last_insert_rowid() as i32;
        Ok(id)
    }

    async fn search(
        &self,
        q: &str,
        scope: MemoryScope,
        folder_id: Option<i32>,
        limit: usize,
    ) -> Result<Vec<MemoryHit>, MemoryError> {
        let limit = limit.min(50);
        let conn = self
            .conn
            .lock()
            .map_err(|_| MemoryError("lock poisoned".to_string()))?;

        let scope_str = match scope {
            MemoryScope::Project => "project",
            MemoryScope::Global => "global",
        };

        let fts_query = format_fts5_query(q);
        let mut hits: Vec<MemoryHit> = Vec::new();
        let mut visited_ids: HashSet<i32> = HashSet::new();

        if !fts_query.is_empty() {
            // FTS5 search with BM25 scoring
            let mut stmt = conn.prepare(
                "SELECT mem_node.id, mem_node.kind, mem_node.title, mem_node.body,
                        mem_node.scope, mem_node.folder_id, mem_node.provenance,
                        mem_node.created_at, mem_node.updated_at, mem_node.stale_at,
                        bm25(mem_node_fts) as score
                 FROM mem_node_fts
                 JOIN mem_node ON mem_node_fts.rowid = mem_node.id
                 WHERE mem_node_fts MATCH ? AND mem_node.scope = ? AND (? IS NULL OR mem_node.folder_id = ?)
                 ORDER BY score ASC
                 LIMIT ?"
            ).map_err(|e| MemoryError(format!("prepare search failed: {}", e)))?;

            let direct_results = stmt
                .query_map(
                    params![&fts_query, scope_str, folder_id, folder_id, limit],
                    |row| {
                        let node = self.load_node(row)?;
                        let raw_score: f64 = row.get(10)?;
                        let normalized_score = 1.0 / (1.0 + raw_score.abs());
                        Ok(MemoryHit {
                            node,
                            score: normalized_score,
                            via: vec![],
                        })
                    },
                )
                .map_err(|e| MemoryError(format!("query search failed: {}", e)))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MemoryError(format!("collect search failed: {}", e)))?;

            for hit in direct_results {
                visited_ids.insert(hit.node.id);
                hits.push(hit);
            }
        }

        // If FTS5 yielded no results (e.g. substring/LIKE query fallback)
        if hits.is_empty() {
            let like_pattern = format!("%{}%", q.trim());
            let mut stmt = conn.prepare(
                "SELECT id, kind, title, body, scope, folder_id, provenance, created_at, updated_at, stale_at
                 FROM mem_node
                 WHERE scope = ? AND (? IS NULL OR folder_id = ?)
                   AND (title LIKE ? OR body LIKE ?)
                 ORDER BY updated_at DESC
                 LIMIT ?"
            ).map_err(|e| MemoryError(format!("prepare fallback search failed: {}", e)))?;

            let fallback_results = stmt
                .query_map(
                    params![
                        scope_str,
                        folder_id,
                        folder_id,
                        &like_pattern,
                        &like_pattern,
                        limit
                    ],
                    |row| {
                        let node = self.load_node(row)?;
                        Ok(MemoryHit {
                            node,
                            score: 1.0,
                            via: vec![],
                        })
                    },
                )
                .map_err(|e| MemoryError(format!("query fallback search failed: {}", e)))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MemoryError(format!("collect fallback search failed: {}", e)))?;

            for hit in fallback_results {
                visited_ids.insert(hit.node.id);
                hits.push(hit);
            }
        }

        // 2-hop graph expansion along mem_edge
        let direct_node_ids: Vec<(i32, f64)> = hits.iter().map(|h| (h.node.id, h.score)).collect();
        let mut hop1_nodes: Vec<(i32, Vec<i32>, f64)> = Vec::new();

        for (direct_id, base_score) in direct_node_ids {
            let mut edge_stmt = conn
                .prepare("SELECT from_id, to_id FROM mem_edge WHERE from_id = ? OR to_id = ?")
                .map_err(|e| MemoryError(format!("prepare edge search failed: {}", e)))?;

            let neighbors: Vec<i32> = edge_stmt
                .query_map(params![direct_id, direct_id], |row| {
                    let from: i32 = row.get(0)?;
                    let to: i32 = row.get(1)?;
                    Ok(if from == direct_id { to } else { from })
                })
                .map_err(|e| MemoryError(format!("query edges failed: {}", e)))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MemoryError(format!("collect edges failed: {}", e)))?;

            for neighbor_id in neighbors {
                if !visited_ids.contains(&neighbor_id) {
                    visited_ids.insert(neighbor_id);
                    if let Ok(Some(node)) = conn.query_row(
                        "SELECT id, kind, title, body, scope, folder_id, provenance, created_at, updated_at, stale_at FROM mem_node WHERE id = ?",
                        params![neighbor_id],
                        |row| self.load_node(row),
                    ).optional() {
                        if node_matches_scope(&node, scope, folder_id) {
                            let score = base_score * 0.5;
                            let path = vec![direct_id];
                            hop1_nodes.push((neighbor_id, path.clone(), score));
                            hits.push(MemoryHit {
                                node,
                                score,
                                via: path,
                            });
                        }
                    }
                }
            }
        }

        // Hop 2
        for (hop1_id, path_to_hop1, base_score) in hop1_nodes {
            let mut edge_stmt = conn
                .prepare("SELECT from_id, to_id FROM mem_edge WHERE from_id = ? OR to_id = ?")
                .map_err(|e| MemoryError(format!("prepare hop2 edge search failed: {}", e)))?;

            let neighbors: Vec<i32> = edge_stmt
                .query_map(params![hop1_id, hop1_id], |row| {
                    let from: i32 = row.get(0)?;
                    let to: i32 = row.get(1)?;
                    Ok(if from == hop1_id { to } else { from })
                })
                .map_err(|e| MemoryError(format!("query hop2 edges failed: {}", e)))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| MemoryError(format!("collect hop2 edges failed: {}", e)))?;

            for neighbor_id in neighbors {
                if !visited_ids.contains(&neighbor_id) {
                    visited_ids.insert(neighbor_id);
                    if let Ok(Some(node)) = conn.query_row(
                        "SELECT id, kind, title, body, scope, folder_id, provenance, created_at, updated_at, stale_at FROM mem_node WHERE id = ?",
                        params![neighbor_id],
                        |row| self.load_node(row),
                    ).optional() {
                        if node_matches_scope(&node, scope, folder_id) {
                            let score = base_score * 0.5;
                            let mut path = path_to_hop1.clone();
                            path.push(hop1_id);
                            hits.push(MemoryHit {
                                node,
                                score,
                                via: path,
                            });
                        }
                    }
                }
            }
        }

        // Limit results to ≤ 8000 characters and limit count ≤ 50
        let mut total_chars = 0;
        let mut final_hits = Vec::new();
        for hit in hits {
            let char_len = hit.node.title.len() + hit.node.body.len();
            if total_chars + char_len > 8000 && !final_hits.is_empty() {
                break;
            }
            total_chars += char_len;
            final_hits.push(hit);
            if final_hits.len() >= limit {
                break;
            }
        }

        Ok(final_hits)
    }

    async fn link(&self, from: i32, to: i32, rel: MemoryRel) -> Result<(), MemoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MemoryError("lock poisoned".to_string()))?;
        let now = Utc::now().to_rfc3339();
        let rel_str = match rel {
            MemoryRel::CausedBy => "caused_by",
            MemoryRel::FixedBy => "fixed_by",
            MemoryRel::RelatesTo => "relates_to",
            MemoryRel::PartOf => "part_of",
            MemoryRel::Supersedes => "supersedes",
        };

        conn.execute(
            "INSERT OR IGNORE INTO mem_edge (from_id, to_id, rel, created_at) VALUES (?, ?, ?, ?)",
            params![from, to, rel_str, &now],
        )
        .map_err(|e| MemoryError(format!("link failed: {}", e)))?;

        Ok(())
    }

    async fn get(&self, id: i32) -> Result<Option<MemoryNode>, MemoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MemoryError("lock poisoned".to_string()))?;

        let node = conn.query_row(
            "SELECT id, kind, title, body, scope, folder_id, provenance, created_at, updated_at, stale_at
             FROM mem_node WHERE id = ?",
            params![id],
            |row| self.load_node(row),
        ).optional()
            .map_err(|e| MemoryError(format!("get failed: {}", e)))?;

        Ok(node)
    }

    async fn delete(&self, id: i32) -> Result<(), MemoryError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| MemoryError("lock poisoned".to_string()))?;

        conn.execute("DELETE FROM mem_node WHERE id = ?", params![id])
            .map_err(|e| MemoryError(format!("delete failed: {}", e)))?;

        Ok(())
    }
}
