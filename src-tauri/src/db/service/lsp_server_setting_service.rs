use chrono::Utc;
use sea_orm::DatabaseConnection;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set, Statement,
};

use crate::db::entities::lsp_server_setting;
use crate::db::error::DbError;

#[derive(Debug, Clone)]
pub struct LspServerDefaultInput {
    pub server_id: String,
    pub default_sort_order: i32,
}

#[derive(Debug, Clone)]
pub struct LspServerSettingsUpdate {
    pub enabled: bool,
    pub config_json: Option<String>,
}

pub async fn ensure_defaults(
    conn: &DatabaseConnection,
    defaults: &[LspServerDefaultInput],
) -> Result<(), DbError> {
    for default in defaults {
        let existing = lsp_server_setting::Entity::find()
            .filter(lsp_server_setting::Column::ServerId.eq(default.server_id.clone()))
            .one(conn)
            .await?;

        if existing.is_some() {
            continue;
        }

        let now = Utc::now();
        let active = lsp_server_setting::ActiveModel {
            id: NotSet,
            server_id: Set(default.server_id.clone()),
            enabled: Set(true),
            sort_order: Set(default.default_sort_order),
            installed_version: Set(None),
            config_json: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        };
        match active.insert(conn).await {
            Ok(_) => {}
            Err(e) if e.to_string().contains("UNIQUE constraint failed") => {
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

pub async fn list(
    conn: &DatabaseConnection,
) -> Result<Vec<lsp_server_setting::Model>, DbError> {
    let rows = lsp_server_setting::Entity::find()
        .order_by_asc(lsp_server_setting::Column::SortOrder)
        .all(conn)
        .await?;
    Ok(rows)
}

pub async fn get_by_server_id(
    conn: &DatabaseConnection,
    server_id: &str,
) -> Result<Option<lsp_server_setting::Model>, DbError> {
    let model = lsp_server_setting::Entity::find()
        .filter(lsp_server_setting::Column::ServerId.eq(server_id))
        .one(conn)
        .await?;
    Ok(model)
}

pub async fn update(
    conn: &DatabaseConnection,
    server_id: &str,
    patch: LspServerSettingsUpdate,
) -> Result<(), DbError> {
    let model = lsp_server_setting::Entity::find()
        .filter(lsp_server_setting::Column::ServerId.eq(server_id))
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("lsp server setting not found: {server_id}")))?;

    let mut active = model.into_active_model();
    active.enabled = Set(patch.enabled);
    active.config_json = Set(patch.config_json);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn set_installed_version(
    conn: &DatabaseConnection,
    server_id: &str,
    installed_version: Option<String>,
) -> Result<(), DbError> {
    if let Some(model) = lsp_server_setting::Entity::find()
        .filter(lsp_server_setting::Column::ServerId.eq(server_id))
        .one(conn)
        .await?
    {
        let mut active = model.into_active_model();
        active.installed_version = Set(installed_version);
        active.updated_at = Set(Utc::now());
        active.update(conn).await?;
    }
    Ok(())
}

pub async fn reorder(conn: &DatabaseConnection, server_ids: &[String]) -> Result<(), DbError> {
    if server_ids.is_empty() {
        return Ok(());
    }

    match reorder_once(conn, server_ids).await {
        Ok(()) => Ok(()),
        Err(err) if is_sqlite_full_error(&err) => {
            conn.execute(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA wal_checkpoint(TRUNCATE);".to_owned(),
            ))
            .await?;
            reorder_once(conn, server_ids).await
        }
        Err(err) => Err(err),
    }
}

async fn reorder_once(conn: &DatabaseConnection, server_ids: &[String]) -> Result<(), DbError> {
    let now = Utc::now();
    for (index, server_id) in server_ids.iter().enumerate() {
        if let Some(model) = lsp_server_setting::Entity::find()
            .filter(lsp_server_setting::Column::ServerId.eq(server_id.as_str()))
            .one(conn)
            .await?
        {
            if model.sort_order == index as i32 {
                continue;
            }
            let mut active = model.into_active_model();
            active.sort_order = Set(index as i32);
            active.updated_at = Set(now);
            active.update(conn).await?;
        }
    }
    Ok(())
}

fn is_sqlite_full_error(err: &DbError) -> bool {
    let message = err.to_string();
    message.contains("database or disk is full") || message.contains("(code: 13)")
}
