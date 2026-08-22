//! PK arena round CRUD commands. The `*_core` fns are mode-agnostic and
//! shared by the Tauri wrappers and the Axum handlers.

use std::path::{Path, PathBuf};

use crate::app_error::AppCommandError;
use crate::db::entities::pk_round::PkRoundStatus;
use crate::db::error::DbError;
use crate::db::service::pk_round_service;
use crate::db::AppDatabase;
use crate::models::{PkRoundConfig, PkRoundInfo};

const PK_REPORT_SNAPSHOT_DIR: &str = "pk-report-snapshots";
const PK_REPORT_SNAPSHOT_MAX_BYTES: usize = 48 * 1024 * 1024;

fn report_snapshot_path(data_dir: &Path, id: i32) -> PathBuf {
    data_dir
        .join(PK_REPORT_SNAPSHOT_DIR)
        .join(format!("{id}.json"))
}

// -- shared business logic (both modes) --

pub async fn pk_round_list_core(
    db: &AppDatabase,
    folder_id: Option<i32>,
) -> Result<Vec<PkRoundInfo>, DbError> {
    pk_round_service::list(&db.conn, folder_id).await
}

pub async fn pk_round_get_core(db: &AppDatabase, id: i32) -> Result<PkRoundInfo, DbError> {
    pk_round_service::get_info(&db.conn, id).await
}

pub async fn pk_round_create_core(
    db: &AppDatabase,
    folder_id: i32,
    task: String,
    config: PkRoundConfig,
) -> Result<PkRoundInfo, DbError> {
    let row = pk_round_service::create(&db.conn, folder_id, task, config).await?;
    pk_round_service::get_info(&db.conn, row.id).await
}

pub async fn pk_round_update_status_core(
    db: &AppDatabase,
    id: i32,
    status: String,
) -> Result<(), DbError> {
    let parsed = match status.as_str() {
        "ready" => PkRoundStatus::Ready,
        "running" => PkRoundStatus::Running,
        "finished" => PkRoundStatus::Finished,
        "canceled" => PkRoundStatus::Canceled,
        "interrupted" => PkRoundStatus::Interrupted,
        other => {
            return Err(DbError::Validation(format!(
                "unknown pk_round status: {other}"
            )));
        }
    };
    pk_round_service::update_status(&db.conn, id, parsed).await
}

pub async fn pk_round_delete_core(db: &AppDatabase, id: i32) -> Result<(), DbError> {
    pk_round_service::soft_delete(&db.conn, id).await
}

pub async fn pk_round_update_judge_core(
    db: &AppDatabase,
    id: i32,
    judge_result: Option<String>,
    judge_status: String,
) -> Result<(), DbError> {
    pk_round_service::update_judge(&db.conn, id, judge_result, judge_status).await
}

/// Persist the self-contained artifacts and runtime metrics needed to export a
/// round after its disposable git worktrees have been removed. The payload is
/// versioned JSON owned by the frontend; the backend deliberately treats it as
/// opaque data and only enforces a bounded size and a stable per-round path.
pub async fn pk_round_save_report_snapshot_core(
    data_dir: &Path,
    id: i32,
    snapshot: String,
) -> Result<(), AppCommandError> {
    if snapshot.len() > PK_REPORT_SNAPSHOT_MAX_BYTES {
        return Err(AppCommandError::invalid_input(format!(
            "PK report snapshot exceeds {} MiB",
            PK_REPORT_SNAPSHOT_MAX_BYTES / 1024 / 1024
        )));
    }
    let path = report_snapshot_path(data_dir, id);
    let parent = path
        .parent()
        .ok_or_else(|| AppCommandError::invalid_input("invalid PK report snapshot path"))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(AppCommandError::io)?;
    tokio::fs::write(path, snapshot)
        .await
        .map_err(AppCommandError::io)
}

pub async fn pk_round_get_report_snapshot_core(
    data_dir: &Path,
    id: i32,
) -> Result<Option<String>, AppCommandError> {
    let path = report_snapshot_path(data_dir, id);
    match tokio::fs::read_to_string(path).await {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppCommandError::io(error)),
    }
}

pub async fn pk_round_delete_report_snapshot_core(
    data_dir: &Path,
    id: i32,
) -> Result<(), AppCommandError> {
    match tokio::fs::remove_file(report_snapshot_path(data_dir, id)).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppCommandError::io(error)),
    }
}

// -- Tauri command wrappers (desktop mode only) --

#[cfg(feature = "tauri-runtime")]
fn resolve_desktop_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, AppCommandError> {
    use tauri::Manager;

    let fallback = app
        .path()
        .app_data_dir()
        .map_err(|error| {
            AppCommandError::io_error("Resolve app data dir").with_detail(error.to_string())
        })?;
    Ok(crate::paths::resolve_effective_data_dir(&fallback))
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_list(
    db: tauri::State<'_, AppDatabase>,
    folder_id: Option<i32>,
) -> Result<Vec<PkRoundInfo>, DbError> {
    pk_round_list_core(&db, folder_id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_get(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
) -> Result<PkRoundInfo, DbError> {
    pk_round_get_core(&db, id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_create(
    db: tauri::State<'_, AppDatabase>,
    folder_id: i32,
    task: String,
    config: PkRoundConfig,
) -> Result<PkRoundInfo, DbError> {
    pk_round_create_core(&db, folder_id, task, config).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_update_status(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    status: String,
) -> Result<(), DbError> {
    pk_round_update_status_core(&db, id, status).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_delete(
    db: tauri::State<'_, AppDatabase>,
    app: tauri::AppHandle,
    id: i32,
) -> Result<(), AppCommandError> {
    pk_round_delete_core(&db, id)
        .await
        .map_err(AppCommandError::from)?;
    let data_dir = resolve_desktop_data_dir(&app)?;
    pk_round_delete_report_snapshot_core(&data_dir, id).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_update_judge(
    db: tauri::State<'_, AppDatabase>,
    id: i32,
    judge_result: Option<String>,
    judge_status: String,
) -> Result<(), DbError> {
    pk_round_update_judge_core(&db, id, judge_result, judge_status).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_save_report_snapshot(
    app: tauri::AppHandle,
    id: i32,
    snapshot: String,
) -> Result<(), AppCommandError> {
    let data_dir = resolve_desktop_data_dir(&app)?;
    pk_round_save_report_snapshot_core(&data_dir, id, snapshot).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn pk_round_get_report_snapshot(
    app: tauri::AppHandle,
    id: i32,
) -> Result<Option<String>, AppCommandError> {
    let data_dir = resolve_desktop_data_dir(&app)?;
    pk_round_get_report_snapshot_core(&data_dir, id).await
}

#[cfg(test)]
mod tests {
    use sea_orm::EntityTrait;

    use super::*;
    use crate::db::entities::conversation;
    use crate::db::service::conversation_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::models::AgentType;

    fn requires_unmanaged_app_state(source: &str) -> bool {
        let forbidden = [
            "tauri::State<'_, crate::app_state::",
            "AppState>",
        ]
        .concat();
        source.contains(&forbidden)
    }

    #[test]
    fn detects_the_unmanaged_state_signature_that_breaks_tauri_invocation() {
        let broken = [
            "state: tauri::State<'_, crate::app_state::",
            "AppState>,",
        ]
        .concat();
        assert!(requires_unmanaged_app_state(&broken));
    }

    #[test]
    fn desktop_pk_commands_do_not_require_unmanaged_app_state() {
        assert!(
            !requires_unmanaged_app_state(include_str!("pk.rs")),
            "desktop PK commands must use automatically injected AppHandle or a registered state"
        );
    }

    #[tokio::test]
    async fn archiving_round_also_hides_its_pk_conversations() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/pk-archive").await;
        let round = pk_round_service::create(
            &db.conn,
            folder_id,
            "test task".into(),
            PkRoundConfig {
                agents: Vec::new(),
                permission_mode: "default".into(),
                bare_mode: false,
                effort: "default".into(),
                judge_agent: None,
                judge_dimensions: Vec::new(),
                base_commit: None,
            },
        )
        .await
        .unwrap();
        let conversation = conversation_service::create_pk(
            &db.conn,
            folder_id,
            AgentType::Qoder,
            Some("PK contestant".into()),
            None,
            round.id,
        )
        .await
        .unwrap();

        pk_round_delete_core(&db, round.id).await.unwrap();

        assert!(pk_round_service::list(&db.conn, None)
            .await
            .unwrap()
            .is_empty());
        let archived = conversation::Entity::find_by_id(conversation.id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(archived.deleted_at.is_some());
    }

    #[tokio::test]
    async fn saved_report_snapshot_survives_worktree_cleanup() {
        let data_dir = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::fs::write(worktree.path().join("index.html"), "<h1>entry</h1>").unwrap();
        let snapshot = r#"{"version":1,"artifactsBySlot":{"0":[{"path":"index.html","contentBase64":"PGgxPmVudHJ5PC9oMT4="}]}}"#;

        pk_round_save_report_snapshot_core(data_dir.path(), 7, snapshot.into())
            .await
            .unwrap();
        worktree.close().unwrap();

        assert_eq!(
            pk_round_get_report_snapshot_core(data_dir.path(), 7)
                .await
                .unwrap()
                .as_deref(),
            Some(snapshot)
        );

        pk_round_delete_report_snapshot_core(data_dir.path(), 7)
            .await
            .unwrap();
        assert!(pk_round_get_report_snapshot_core(data_dir.path(), 7)
            .await
            .unwrap()
            .is_none());
    }
}
