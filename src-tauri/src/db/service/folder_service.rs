use chrono::Utc;
use sea_orm::DatabaseConnection;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set, Statement,
};

use crate::db::entities::folder;
use crate::db::entities::folder::FolderKind;
use crate::db::error::DbError;
use crate::models::agent::AgentType;
use crate::models::{FolderDetail, FolderHistoryEntry};

/// Theme color sentinel stored in the DB. The frontend leaves the folder group
/// unscoped so it inherits the app-wide appearance theme color.
pub const DEFAULT_FOLDER_COLOR: &str = "inherit";

fn to_entry(m: folder::Model) -> FolderHistoryEntry {
    FolderHistoryEntry {
        id: m.id,
        path: m.path,
        name: m.name,
        last_opened_at: m.last_opened_at,
    }
}

fn parse_agent_type(s: &Option<String>) -> Option<AgentType> {
    s.as_deref()
        .and_then(|v| serde_json::from_value(serde_json::Value::String(v.to_string())).ok())
}

fn to_detail(m: folder::Model) -> FolderDetail {
    let default_agent_type = parse_agent_type(&m.default_agent_type);
    FolderDetail {
        id: m.id,
        name: m.name,
        path: m.path,
        git_branch: m.git_branch,
        default_agent_type,
        last_opened_at: m.last_opened_at,
        sort_order: m.sort_order,
        color: m.color,
        parent_id: m.parent_id,
        kind: m.kind,
        alias: m.alias,
    }
}

pub async fn get_folder_by_id(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Option<FolderDetail>, DbError> {
    let row = folder::Entity::find_by_id(folder_id)
        .filter(folder::Column::DeletedAt.is_null())
        .one(conn)
        .await?;

    Ok(row.map(to_detail))
}

/// How [`add_folder_inner`] writes the `parent_id` column. The two callers want
/// different semantics on reopen of an existing path, which a bare `Option<i32>`
/// could not express (it conflates "no parent" with "don't touch the parent").
enum ParentWrite {
    /// Plain open: leave an existing row's `parent_id` untouched (insert NULL).
    /// A plain reopen must never clear a worktree's recorded root.
    Preserve,
    /// Worktree open: write this exact value on both insert and reopen — so the
    /// stored relationship always reflects the latest call (including `None` to
    /// demote to a top-level folder) and can never go stale.
    Set(Option<i32>),
}

pub async fn add_folder(
    conn: &DatabaseConnection,
    path: &str,
) -> Result<FolderHistoryEntry, DbError> {
    add_folder_inner(conn, path, ParentWrite::Preserve).await
}

/// Like [`add_folder`] but authoritatively sets `parent_id` — the *root* folder
/// this path was created under (used by the worktree flow so a worktree folder
/// remembers its originating repo folder). The value is written on both insert
/// and reopen, so it always reflects the latest worktree relationship and never
/// a stale one.
pub async fn add_folder_with_parent(
    conn: &DatabaseConnection,
    path: &str,
    parent_id: Option<i32>,
) -> Result<FolderHistoryEntry, DbError> {
    add_folder_inner(conn, path, ParentWrite::Set(parent_id)).await
}

async fn add_folder_inner(
    conn: &DatabaseConnection,
    path: &str,
    parent: ParentWrite,
) -> Result<FolderHistoryEntry, DbError> {
    let now = Utc::now();
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    let existing = folder::Entity::find()
        .filter(folder::Column::Path.eq(path))
        .one(conn)
        .await?;

    let model = if let Some(row) = existing {
        let mut active = row.into_active_model();
        active.name = Set(name);
        active.last_opened_at = Set(now);
        active.updated_at = Set(now);
        active.deleted_at = Set(None);
        active.is_open = Set(true);
        // Plain reopen leaves the relationship as-is; the worktree flow writes
        // the authoritative value (including NULL) so it can never go stale.
        if let ParentWrite::Set(parent_id) = parent {
            active.parent_id = Set(parent_id);
        }
        active.update(conn).await?
    } else {
        let max_order = folder::Entity::find()
            .order_by_desc(folder::Column::SortOrder)
            .one(conn)
            .await?
            .map(|m| m.sort_order)
            .unwrap_or(0);
        let active = folder::ActiveModel {
            id: NotSet,
            name: Set(name.clone()),
            path: Set(path.to_string()),
            git_branch: Set(None),
            default_agent_type: Set(None),
            last_opened_at: Set(now),
            created_at: Set(now),
            updated_at: Set(now),
            deleted_at: Set(None),
            is_open: Set(true),
            sort_order: Set(max_order + 1),
            color: Set(DEFAULT_FOLDER_COLOR.to_string()),
            parent_id: Set(match parent {
                ParentWrite::Preserve => None,
                ParentWrite::Set(parent_id) => parent_id,
            }),
            kind: Set(FolderKind::Regular),
            alias: Set(None),
        };
        active.insert(conn).await?
    };

    Ok(to_entry(model))
}

/// Create a dedicated hidden folder backing a single chat-mode conversation.
///
/// Unlike [`add_folder`], the display name is a fixed sentinel ("Chat") rather
/// than derived from the path, and `kind = chat` is set so the frontend routes
/// this folder's conversations to the sidebar "Chat" group and hides
/// folder-bound chrome. `path` is a freshly generated per-conversation scratch dir, so it
/// never collides on the `UNIQUE(path)` constraint. Returns the full
/// [`FolderDetail`] so the caller can hand it straight to the frontend.
pub async fn add_chat_folder(
    conn: &DatabaseConnection,
    path: &str,
) -> Result<FolderDetail, DbError> {
    let now = Utc::now();
    let max_order = folder::Entity::find()
        .order_by_desc(folder::Column::SortOrder)
        .one(conn)
        .await?
        .map(|m| m.sort_order)
        .unwrap_or(0);
    let active = folder::ActiveModel {
        id: NotSet,
        name: Set("Chat".to_string()),
        path: Set(path.to_string()),
        git_branch: Set(None),
        default_agent_type: Set(None),
        last_opened_at: Set(now),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        is_open: Set(true),
        sort_order: Set(max_order + 1),
        color: Set(DEFAULT_FOLDER_COLOR.to_string()),
        parent_id: Set(None),
        kind: Set(FolderKind::Chat),
        alias: Set(None),
    };
    let model = active.insert(conn).await?;
    Ok(to_detail(model))
}

pub async fn update_folder_color(
    conn: &DatabaseConnection,
    folder_id: i32,
    color: &str,
) -> Result<Option<FolderDetail>, DbError> {
    let row = folder::Entity::find_by_id(folder_id)
        .filter(folder::Column::DeletedAt.is_null())
        .one(conn)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let mut active = row.into_active_model();
    active.color = Set(color.to_string());
    active.updated_at = Set(Utc::now());
    let updated = active.update(conn).await?;
    Ok(Some(to_detail(updated)))
}

/// Sets (or clears) a folder's display alias. `alias = None` clears it; callers
/// are expected to have already normalized empty/whitespace input to `None`.
pub async fn update_folder_alias(
    conn: &DatabaseConnection,
    folder_id: i32,
    alias: Option<String>,
) -> Result<Option<FolderDetail>, DbError> {
    let row = folder::Entity::find_by_id(folder_id)
        .filter(folder::Column::DeletedAt.is_null())
        .one(conn)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let mut active = row.into_active_model();
    active.alias = Set(alias);
    active.updated_at = Set(Utc::now());
    let updated = active.update(conn).await?;
    Ok(Some(to_detail(updated)))
}

pub async fn update_folder_default_agent(
    conn: &DatabaseConnection,
    folder_id: i32,
    default_agent_type: Option<AgentType>,
) -> Result<Option<FolderDetail>, DbError> {
    let row = folder::Entity::find_by_id(folder_id)
        .filter(folder::Column::DeletedAt.is_null())
        .one(conn)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    // Serialize AgentType to its snake_case wire form (e.g. "claude_code").
    // Mirrors `parse_agent_type`'s round-trip through serde_json.
    let serialized = default_agent_type
        .map(|t| serde_json::to_value(t).ok())
        .and_then(|v| v.and_then(|val| val.as_str().map(|s| s.to_string())));

    let mut active = row.into_active_model();
    active.default_agent_type = Set(serialized);
    active.updated_at = Set(Utc::now());
    let updated = active.update(conn).await?;
    Ok(Some(to_detail(updated)))
}

pub async fn list_folders(conn: &DatabaseConnection) -> Result<Vec<FolderHistoryEntry>, DbError> {
    let rows = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        // Only regular folders are user-facing in folder history / open-folder
        // pickers — hidden chat folders (and future engine-created kinds) are an
        // implementation detail.
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .order_by_desc(folder::Column::LastOpenedAt)
        .all(conn)
        .await?;

    Ok(rows.into_iter().map(to_entry).collect())
}

pub async fn remove_folder(conn: &DatabaseConnection, path: &str) -> Result<(), DbError> {
    let now = Utc::now();
    let row = folder::Entity::find()
        .filter(folder::Column::Path.eq(path))
        .filter(folder::Column::DeletedAt.is_null())
        .one(conn)
        .await?;

    if let Some(row) = row {
        let mut active = row.into_active_model();
        active.deleted_at = Set(Some(now));
        active.updated_at = Set(now);
        active.update(conn).await?;
    }
    Ok(())
}

pub async fn set_folder_open(
    conn: &DatabaseConnection,
    folder_id: i32,
    is_open: bool,
) -> Result<(), DbError> {
    let row = folder::Entity::find_by_id(folder_id).one(conn).await?;

    if let Some(row) = row {
        let mut active = row.into_active_model();
        active.is_open = Set(is_open);
        active.updated_at = Set(Utc::now());
        active.update(conn).await?;
    }
    Ok(())
}

pub async fn list_open_folders(
    conn: &DatabaseConnection,
) -> Result<Vec<FolderHistoryEntry>, DbError> {
    let rows = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::IsOpen.eq(true))
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .order_by_desc(folder::Column::LastOpenedAt)
        .all(conn)
        .await?;

    Ok(rows.into_iter().map(to_entry).collect())
}

pub async fn list_open_folder_details(
    conn: &DatabaseConnection,
) -> Result<Vec<FolderDetail>, DbError> {
    // Excludes hidden chat folders from the workspace "open folders" surface.
    // `list_all_folder_details` (below) intentionally keeps them so the frontend
    // can still resolve an active chat conversation's cwd / active folder by id.
    let rows = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::IsOpen.eq(true))
        .filter(folder::Column::Kind.eq(FolderKind::Regular))
        .order_by_asc(folder::Column::SortOrder)
        .order_by_desc(folder::Column::LastOpenedAt)
        .all(conn)
        .await?;

    Ok(rows.into_iter().map(to_detail).collect())
}

pub async fn list_all_folder_details(
    conn: &DatabaseConnection,
) -> Result<Vec<FolderDetail>, DbError> {
    let rows = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .order_by_asc(folder::Column::SortOrder)
        .order_by_desc(folder::Column::LastOpenedAt)
        .all(conn)
        .await?;

    Ok(rows.into_iter().map(to_detail).collect())
}

/// Paths of all *live* (non-deleted) chat scratch folders. Consumed by the
/// startup orphan-scratch-dir GC to spare directories still bound to a chat
/// conversation, while reclaiming pre-send drafts (no row at all) and
/// post-delete dirs (soft-deleted row → `DeletedAt` set → excluded here).
pub async fn list_live_chat_folder_paths(
    conn: &DatabaseConnection,
) -> Result<Vec<String>, DbError> {
    let rows = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .filter(folder::Column::Kind.eq(FolderKind::Chat))
        .all(conn)
        .await?;

    Ok(rows.into_iter().map(|m| m.path).collect())
}

pub async fn reorder_folders(conn: &DatabaseConnection, ids: Vec<i32>) -> Result<(), DbError> {
    if ids.is_empty() {
        return Ok(());
    }

    let now = Utc::now();
    let now_str = now.format("%Y-%m-%d %H:%M:%S %:z").to_string();
    let case_expr = ids
        .iter()
        .enumerate()
        .map(|(idx, id)| format!("WHEN {} THEN {}", id, idx + 1))
        .collect::<Vec<_>>()
        .join(" ");
    let id_list = ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "UPDATE folder SET sort_order = CASE id {case_expr} END, updated_at = '{now_str}' WHERE id IN ({id_list})"
    );
    conn.execute(Statement::from_string(DbBackend::Sqlite, sql))
        .await?;

    Ok(())
}
