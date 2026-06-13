//! Loop engineering schema (M2): 10 tables backing spaces, issues, the per-issue
//! artifact DAG, iterations (agent runs), deterministic validation runs, the
//! two-category inbox and the memory layer.
//!
//! Raw SQLite DDL is used deliberately: the project is SQLite-only, the four
//! *partial* unique indexes (the dispatch leases + pending-inbox dedupe) cannot
//! be expressed through SeaORM's `Index` builder, and 10 tables read far more
//! clearly as DDL than as builder chains. Cross-subsystem refs (`folder_id`,
//! `conversation_id`) are plain columns. The artifact↔iteration cycle
//! (`produced_by_iteration_id` / `target_artifact_id`) is intentionally left
//! without FK constraints to avoid a circular dependency; every other loop-table
//! reference is a real FK enforced in the test pool.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP: &[&str] = &[
    "CREATE TABLE loop_space (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        folder_id INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE TABLE loop_issue (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        space_id INTEGER NOT NULL REFERENCES loop_space(id) ON DELETE CASCADE,
        seq_no INTEGER NOT NULL,
        title TEXT NOT NULL,
        description TEXT NOT NULL,
        priority TEXT NOT NULL DEFAULT 'medium',
        status TEXT NOT NULL DEFAULT 'pending',
        pause_reason TEXT,
        route TEXT NOT NULL DEFAULT 'undecided',
        config TEXT NOT NULL,
        worktree_folder_id INTEGER,
        base_branch TEXT,
        base_commit TEXT,
        active_task_artifact_id INTEGER,
        token_used BIGINT NOT NULL DEFAULT 0,
        token_budget BIGINT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        triggered_at TEXT,
        ended_at TEXT
    )",
    "CREATE UNIQUE INDEX uniq_loop_issue_seq ON loop_issue(space_id, seq_no)",
    "CREATE INDEX idx_loop_issue_space_status ON loop_issue(space_id, status)",
    "CREATE TABLE loop_artifact (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        space_id INTEGER NOT NULL REFERENCES loop_space(id) ON DELETE CASCADE,
        issue_id INTEGER NOT NULL REFERENCES loop_issue(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        title TEXT NOT NULL,
        status TEXT NOT NULL,
        origin TEXT NOT NULL,
        produced_by_iteration_id INTEGER,
        verdict TEXT,
        attempt INTEGER NOT NULL DEFAULT 0,
        last_failure_sig TEXT,
        sort INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
    "CREATE INDEX idx_loop_artifact_issue_kind ON loop_artifact(issue_id, kind)",
    "CREATE INDEX idx_loop_artifact_space ON loop_artifact(space_id)",
    "CREATE TABLE loop_artifact_revision (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        artifact_id INTEGER NOT NULL REFERENCES loop_artifact(id) ON DELETE CASCADE,
        seq INTEGER NOT NULL,
        content TEXT NOT NULL,
        actor_kind TEXT NOT NULL,
        iteration_id INTEGER,
        created_at TEXT NOT NULL
    )",
    "CREATE UNIQUE INDEX uniq_loop_revision ON loop_artifact_revision(artifact_id, seq)",
    "CREATE TABLE loop_link (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        space_id INTEGER NOT NULL REFERENCES loop_space(id) ON DELETE CASCADE,
        from_artifact_id INTEGER NOT NULL REFERENCES loop_artifact(id) ON DELETE CASCADE,
        to_artifact_id INTEGER NOT NULL REFERENCES loop_artifact(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        created_at TEXT NOT NULL
    )",
    "CREATE UNIQUE INDEX uniq_loop_link ON loop_link(from_artifact_id, to_artifact_id, kind)",
    "CREATE TABLE loop_criterion (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        artifact_id INTEGER NOT NULL REFERENCES loop_artifact(id) ON DELETE CASCADE,
        label TEXT NOT NULL,
        text TEXT NOT NULL,
        sort INTEGER NOT NULL DEFAULT 0
    )",
    "CREATE TABLE loop_iteration (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        space_id INTEGER NOT NULL REFERENCES loop_space(id) ON DELETE CASCADE,
        issue_id INTEGER NOT NULL REFERENCES loop_issue(id) ON DELETE CASCADE,
        stage TEXT NOT NULL,
        target_artifact_id INTEGER,
        slot_no INTEGER,
        conversation_id INTEGER,
        capability_token TEXT NOT NULL,
        status TEXT NOT NULL,
        launched_by TEXT NOT NULL,
        attempt INTEGER NOT NULL DEFAULT 0,
        tokens_used BIGINT NOT NULL DEFAULT 0,
        context_manifest TEXT,
        created_at TEXT NOT NULL,
        started_at TEXT,
        ended_at TEXT
    )",
    "CREATE UNIQUE INDEX uniq_loop_iteration_token ON loop_iteration(capability_token)",
    "CREATE INDEX idx_loop_iteration_issue ON loop_iteration(issue_id)",
    "CREATE INDEX idx_loop_iteration_space ON loop_iteration(space_id)",
    "CREATE INDEX idx_loop_iteration_conv ON loop_iteration(conversation_id)",
    // Dispatch leases (DB-authoritative double-dispatch guards). Partial unique
    // indexes — SeaORM's Index builder can't express the WHERE clause.
    "CREATE UNIQUE INDEX uniq_active_write ON loop_iteration(issue_id) \
     WHERE stage IN ('implement','finalize') AND status IN ('queued','running')",
    "CREATE UNIQUE INDEX uniq_active_node ON loop_iteration(target_artifact_id, stage) \
     WHERE status IN ('queued','running') AND stage <> 'review'",
    "CREATE UNIQUE INDEX uniq_review_slot ON loop_iteration(target_artifact_id, slot_no) \
     WHERE stage = 'review' AND status IN ('queued','running')",
    "CREATE TABLE loop_validation_run (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        space_id INTEGER NOT NULL REFERENCES loop_space(id) ON DELETE CASCADE,
        issue_id INTEGER NOT NULL REFERENCES loop_issue(id) ON DELETE CASCADE,
        task_artifact_id INTEGER NOT NULL REFERENCES loop_artifact(id) ON DELETE CASCADE,
        iteration_id INTEGER,
        commands TEXT NOT NULL,
        exit_codes TEXT NOT NULL,
        output TEXT NOT NULL,
        passed BOOLEAN NOT NULL,
        created_at TEXT NOT NULL
    )",
    "CREATE TABLE loop_inbox_item (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        space_id INTEGER NOT NULL REFERENCES loop_space(id) ON DELETE CASCADE,
        issue_id INTEGER NOT NULL REFERENCES loop_issue(id) ON DELETE CASCADE,
        iteration_id INTEGER,
        kind TEXT NOT NULL,
        subject_key TEXT NOT NULL,
        payload TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending',
        resolution TEXT,
        created_at TEXT NOT NULL,
        handled_at TEXT
    )",
    "CREATE UNIQUE INDEX uniq_inbox_pending ON loop_inbox_item(issue_id, kind, subject_key) \
     WHERE status = 'pending'",
    "CREATE TABLE loop_memory (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        space_id INTEGER NOT NULL REFERENCES loop_space(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        source TEXT NOT NULL,
        title TEXT NOT NULL,
        content TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'active',
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )",
];

/// Reverse dependency order (children before parents).
const DOWN: &[&str] = &[
    "DROP TABLE IF EXISTS loop_validation_run",
    "DROP TABLE IF EXISTS loop_inbox_item",
    "DROP TABLE IF EXISTS loop_memory",
    "DROP TABLE IF EXISTS loop_criterion",
    "DROP TABLE IF EXISTS loop_link",
    "DROP TABLE IF EXISTS loop_artifact_revision",
    "DROP TABLE IF EXISTS loop_iteration",
    "DROP TABLE IF EXISTS loop_artifact",
    "DROP TABLE IF EXISTS loop_issue",
    "DROP TABLE IF EXISTS loop_space",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for stmt in UP {
            db.execute_unprepared(stmt).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for stmt in DOWN {
            db.execute_unprepared(stmt).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::migration::Migrator;

    fn sql(s: &str) -> Statement {
        Statement::from_string(DbBackend::Sqlite, s.to_owned())
    }

    async fn count(conn: &sea_orm::DatabaseConnection, kind: &str, name: &str) -> i32 {
        let row = conn
            .query_one(sql(&format!(
                "SELECT COUNT(*) AS n FROM sqlite_master WHERE type='{kind}' AND name='{name}'"
            )))
            .await
            .expect("query")
            .expect("row");
        row.try_get::<i32>("", "n").expect("n")
    }

    #[tokio::test]
    async fn creates_all_loop_tables_and_partial_indexes() {
        let conn = Database::connect("sqlite::memory:").await.expect("db");
        Migrator::up(&conn, None).await.expect("migrations");

        for table in [
            "loop_space",
            "loop_issue",
            "loop_artifact",
            "loop_artifact_revision",
            "loop_link",
            "loop_criterion",
            "loop_iteration",
            "loop_validation_run",
            "loop_inbox_item",
            "loop_memory",
        ] {
            assert_eq!(count(&conn, "table", table).await, 1, "table {table} missing");
        }

        for index in [
            "uniq_active_write",
            "uniq_active_node",
            "uniq_review_slot",
            "uniq_inbox_pending",
        ] {
            assert_eq!(count(&conn, "index", index).await, 1, "index {index} missing");
        }
    }
}
