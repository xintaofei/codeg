use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Set, TransactionTrait,
};

use crate::db::entities::{conversation, pk_round};
use crate::db::error::DbError;
use crate::models::{PkRoundConfig, PkRoundInfo};

/// Map a DB row to a frontend-facing `PkRoundInfo`, deserialising the JSON
/// config blob. A corrupt blob falls back to empty defaults rather than
/// failing the whole list — one bad round should not blank the board.
fn to_info(m: pk_round::Model) -> PkRoundInfo {
    let config = serde_json::from_str(&m.config).unwrap_or(PkRoundConfig {
        agents: Vec::new(),
        permission_mode: "default".into(),
        bare_mode: false,
        effort: "default".into(),
        judge_agent: None,
        judge_dimensions: Vec::new(),
        base_commit: None,
    });
    let status = serde_json::to_value(m.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{:?}", m.status));
    let judge_result = m
        .judge_result
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let judge_status = m.judge_status.unwrap_or_else(|| "idle".into());
    PkRoundInfo {
        id: m.id,
        folder_id: m.folder_id,
        task: m.task,
        config,
        status,
        failure_reason: m.failure_reason,
        judge_result,
        judge_status,
        created_at: m.created_at,
        updated_at: m.updated_at,
        finished_at: m.finished_at,
    }
}

pub async fn create(
    conn: &DatabaseConnection,
    folder_id: i32,
    task: String,
    config: PkRoundConfig,
) -> Result<pk_round::Model, DbError> {
    let now = Utc::now();
    let config_json = serde_json::to_string(&config).unwrap_or_else(|_| "{}".into());
    let model = pk_round::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        task: Set(task),
        config: Set(config_json),
        status: Set(pk_round::PkRoundStatus::Ready),
        failure_reason: Set(None),
        judge_result: Set(None),
        judge_status: Set(Some("idle".into())),
        created_at: Set(now),
        updated_at: Set(now),
        finished_at: Set(None),
        deleted_at: Set(None),
    };
    Ok(model.insert(conn).await?)
}

pub async fn get(conn: &DatabaseConnection, id: i32) -> Result<Option<pk_round::Model>, DbError> {
    Ok(pk_round::Entity::find_by_id(id).one(conn).await?)
}

/// List all non-deleted rounds, optionally filtered by folder. Returns
/// `PkRoundInfo` ready for the frontend.
pub async fn list(
    conn: &DatabaseConnection,
    folder_id: Option<i32>,
) -> Result<Vec<PkRoundInfo>, DbError> {
    let mut query = pk_round::Entity::find()
        .filter(pk_round::Column::DeletedAt.is_null())
        .order_by_desc(pk_round::Column::CreatedAt);
    if let Some(fid) = folder_id {
        query = query.filter(pk_round::Column::FolderId.eq(fid));
    }
    let rows = query.all(conn).await?;
    Ok(rows.into_iter().map(to_info).collect())
}

/// Get a single round as `PkRoundInfo`.
pub async fn get_info(conn: &DatabaseConnection, id: i32) -> Result<PkRoundInfo, DbError> {
    let row = pk_round::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("PK round not found: {id}")))?;
    Ok(to_info(row))
}

pub async fn list_by_folder(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Vec<pk_round::Model>, DbError> {
    Ok(pk_round::Entity::find()
        .filter(pk_round::Column::FolderId.eq(folder_id))
        .filter(pk_round::Column::DeletedAt.is_null())
        .order_by_desc(pk_round::Column::CreatedAt)
        .all(conn)
        .await?)
}

pub async fn update_status(
    conn: &DatabaseConnection,
    id: i32,
    status: pk_round::PkRoundStatus,
) -> Result<(), DbError> {
    let round = pk_round::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("PK round not found: {id}")))?;
    let mut active: pk_round::ActiveModel = round.into();
    active.status = Set(status);
    if status == pk_round::PkRoundStatus::Finished
        || status == pk_round::PkRoundStatus::Canceled
        || status == pk_round::PkRoundStatus::Interrupted
    {
        active.finished_at = Set(Some(Utc::now()));
    }
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn soft_delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    use sea_orm::sea_query::Expr;

    let txn = conn.begin().await?;
    let round = pk_round::Entity::find_by_id(id)
        .filter(pk_round::Column::DeletedAt.is_null())
        .one(&txn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("PK round not found: {id}")))?;
    let now = Utc::now();
    let mut active: pk_round::ActiveModel = round.into();
    active.deleted_at = Set(Some(now));
    active.update(&txn).await?;
    conversation::Entity::update_many()
        .col_expr(conversation::Column::DeletedAt, Expr::value(Some(now)))
        .col_expr(conversation::Column::UpdatedAt, Expr::value(now))
        .filter(conversation::Column::PkRoundId.eq(id))
        .filter(conversation::Column::DeletedAt.is_null())
        .exec(&txn)
        .await?;
    txn.commit().await?;
    Ok(())
}

/// Persist the judge verdict and status. `judge_result` is a pre-serialized
/// JSON string (the frontend sends the full PkJudgeResult object); passing
/// None clears it.
pub async fn update_judge(
    conn: &DatabaseConnection,
    id: i32,
    judge_result: Option<String>,
    judge_status: String,
) -> Result<(), DbError> {
    let round = pk_round::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("PK round not found: {id}")))?;
    let mut active: pk_round::ActiveModel = round.into();
    active.judge_result = Set(judge_result);
    active.judge_status = Set(Some(judge_status));
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}
