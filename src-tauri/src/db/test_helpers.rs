//! Test scaffolding: fresh in-memory SQLite database + minimal seed helpers.
//! Used by manager + lifecycle tests that need a real DB without touching the
//! filesystem.

use std::path::Path;

use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

use crate::db::error::DbError;
use crate::db::migration::Migrator;
use crate::db::service::{conversation_service, folder_service};
use crate::db::AppDatabase;
use crate::models::agent::AgentType;

/// On-disk SQLite DB mirroring `init_database` essentials. Backup tests need a
/// real file because `VACUUM INTO` against a `sqlite::memory:` pool routes the
/// snapshot to a separate, empty connection.
pub async fn fresh_disk_db(dir: &Path) -> AppDatabase {
    let path = dir.join("source.db");
    let url = format!("sqlite:{}?mode=rwc", path.to_string_lossy());
    let conn = Database::connect(url).await.expect("open disk db");
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "PRAGMA journal_mode=WAL;".to_owned(),
    ))
    .await
    .expect("wal pragma");
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "PRAGMA foreign_keys=ON;".to_owned(),
    ))
    .await
    .expect("foreign_keys pragma");
    Migrator::up(&conn, None)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))
        .expect("run migrations");
    AppDatabase { conn }
}

pub async fn fresh_in_memory_db() -> AppDatabase {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("open in-memory sqlite");
    // Match the production pragma set as closely as needed for migrations.
    conn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "PRAGMA foreign_keys=ON;".to_owned(),
    ))
    .await
    .expect("foreign_keys pragma");
    Migrator::up(&conn, None)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))
        .expect("run migrations");
    AppDatabase { conn }
}

pub async fn seed_folder(db: &AppDatabase, path: &str) -> i32 {
    folder_service::add_folder(&db.conn, path)
        .await
        .expect("seed folder")
        .id
}

pub async fn seed_conversation(db: &AppDatabase, folder_id: i32, agent_type: AgentType) -> i32 {
    conversation_service::create(&db.conn, folder_id, agent_type, None, None)
        .await
        .expect("seed conversation")
        .id
}
