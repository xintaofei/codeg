use std::collections::HashMap;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::loop_issue::IssueStatus;
use crate::db::entities::{folder, loop_issue, loop_space};
use crate::db::error::DbError;
use crate::models::loops::{IssueConfig, LoopSpaceSummary};

fn not_found(id: i32) -> DbError {
    DbError::Database(sea_orm::DbErr::RecordNotFound(format!("loop_space {id}")))
}

pub async fn create_space(
    conn: &sea_orm::DatabaseConnection,
    name: &str,
    folder_id: i32,
) -> Result<loop_space::Model, DbError> {
    let now = Utc::now();
    let active = loop_space::ActiveModel {
        name: Set(name.to_string()),
        folder_id: Set(folder_id),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    };
    Ok(active.insert(conn).await?)
}

pub async fn update_space(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
    name: &str,
) -> Result<loop_space::Model, DbError> {
    let row = loop_space::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| not_found(id))?;
    let mut active = row.into_active_model();
    active.name = Set(name.to_string());
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

/// Set (or clear, with `None`) the space's default issue config (stored JSON).
/// Inheriting issues resolve their config from this at read time.
pub async fn set_default_config(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
    config: Option<&IssueConfig>,
) -> Result<(), DbError> {
    let json = config.map(|c| serde_json::to_string(c).unwrap_or_else(|_| "{}".to_string()));
    let row = loop_space::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| not_found(id))?;
    let mut active = row.into_active_model();
    active.default_config = Set(json);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn get_space(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
) -> Result<Option<loop_space::Model>, DbError> {
    Ok(loop_space::Entity::find_by_id(id).one(conn).await?)
}

/// Hard-delete a space; loop-table FKs (`ON DELETE CASCADE`) remove every issue,
/// artifact, revision, criterion, link, iteration, validation run, inbox item
/// and memory underneath. Engine worktree cleanup happens at the command layer
/// before this is called.
pub async fn delete_space(conn: &sea_orm::DatabaseConnection, id: i32) -> Result<(), DbError> {
    loop_space::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn list_spaces(
    conn: &sea_orm::DatabaseConnection,
) -> Result<Vec<LoopSpaceSummary>, DbError> {
    let spaces = loop_space::Entity::find()
        .order_by_desc(loop_space::Column::CreatedAt)
        .all(conn)
        .await?;
    if spaces.is_empty() {
        return Ok(Vec::new());
    }

    let space_ids: Vec<i32> = spaces.iter().map(|s| s.id).collect();
    let folder_ids: Vec<i32> = spaces.iter().map(|s| s.folder_id).collect();

    let folders: HashMap<i32, folder::Model> = folder::Entity::find()
        .filter(folder::Column::Id.is_in(folder_ids))
        .all(conn)
        .await?
        .into_iter()
        .map(|f| (f.id, f))
        .collect();

    let issues = loop_issue::Entity::find()
        .filter(loop_issue::Column::SpaceId.is_in(space_ids))
        .all(conn)
        .await?;

    let summaries = spaces
        .into_iter()
        .map(|s| {
            let folder = folders.get(&s.folder_id);
            // Folder join does NOT filter deleted_at — a soft-deleted/missing
            // folder still yields the space (read-only) and flips `detached`.
            let detached = folder.map(|f| f.deleted_at.is_some()).unwrap_or(true);
            let folder_path = folder.map(|f| f.path.clone());
            let mine: Vec<&loop_issue::Model> =
                issues.iter().filter(|i| i.space_id == s.id).collect();
            let issue_count = mine.len() as i64;
            let running_count = mine
                .iter()
                .filter(|i| i.status == IssueStatus::Running)
                .count() as i64;
            let last_activity_at = mine.iter().map(|i| i.updated_at).max();
            LoopSpaceSummary {
                id: s.id,
                name: s.name,
                folder_id: s.folder_id,
                folder_path,
                detached,
                issue_count,
                running_count,
                last_activity_at,
                created_at: s.created_at,
                default_config: s
                    .default_config
                    .as_deref()
                    .and_then(|j| serde_json::from_str(j).ok()),
            }
        })
        .collect();

    Ok(summaries)
}
