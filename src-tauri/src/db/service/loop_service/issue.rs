use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};

use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_issue::{IssuePriority, IssueRoute, IssueStatus};
use crate::db::entities::{loop_artifact, loop_artifact_revision, loop_issue};
use crate::db::error::DbError;
use crate::models::loops::{IssueConfig, LoopIssueDetail, LoopIssueRow};

fn not_found(id: i32) -> DbError {
    DbError::Database(sea_orm::DbErr::RecordNotFound(format!("loop_issue {id}")))
}

fn parse_config(raw: &str) -> IssueConfig {
    serde_json::from_str(raw).unwrap_or_default()
}

pub fn to_issue_row(m: &loop_issue::Model) -> LoopIssueRow {
    LoopIssueRow {
        id: m.id,
        space_id: m.space_id,
        seq_no: m.seq_no,
        title: m.title.clone(),
        priority: m.priority.clone(),
        status: m.status.clone(),
        pause_reason: m.pause_reason.clone(),
        route: m.route,
        token_used: m.token_used,
        token_budget: m.token_budget,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

pub fn to_issue_detail(m: loop_issue::Model) -> LoopIssueDetail {
    let config = parse_config(&m.config);
    let row = to_issue_row(&m);
    LoopIssueDetail {
        row,
        description: m.description,
        config,
        worktree_folder_id: m.worktree_folder_id,
        base_branch: m.base_branch,
        base_commit: m.base_commit,
        active_task_artifact_id: m.active_task_artifact_id,
        config_inherits: m.config_inherits,
    }
}

/// Create an issue and its root `kind = issue` artifact (with a first revision
/// holding the description) in one transaction. The issue starts `pending`
/// (awaiting an explicit human trigger) with route `undecided`.
pub async fn create_issue(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
    title: &str,
    description: &str,
    priority: IssuePriority,
    config: &IssueConfig,
) -> Result<LoopIssueDetail, DbError> {
    let now = Utc::now();
    let config_json = serde_json::to_string(config).unwrap_or_else(|_| "{}".to_string());

    let txn = conn.begin().await?;

    let seq_no = loop_issue::Entity::find()
        .filter(loop_issue::Column::SpaceId.eq(space_id))
        .order_by_desc(loop_issue::Column::SeqNo)
        .one(&txn)
        .await?
        .map(|m| m.seq_no + 1)
        .unwrap_or(1);

    let issue = loop_issue::ActiveModel {
        space_id: Set(space_id),
        seq_no: Set(seq_no),
        title: Set(title.to_string()),
        description: Set(description.to_string()),
        priority: Set(priority),
        status: Set(IssueStatus::Pending),
        pause_reason: Set(None),
        route: Set(IssueRoute::Undecided),
        config: Set(config_json),
        worktree_folder_id: Set(None),
        base_branch: Set(None),
        base_commit: Set(None),
        active_task_artifact_id: Set(None),
        token_used: Set(0),
        token_budget: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        triggered_at: Set(None),
        ended_at: Set(None),
        ..Default::default()
    }
    .insert(&txn)
    .await?;

    let root = loop_artifact::ActiveModel {
        space_id: Set(space_id),
        issue_id: Set(issue.id),
        kind: Set(ArtifactKind::Issue),
        title: Set(title.to_string()),
        status: Set(ArtifactStatus::Done),
        origin: Set(ActorKind::Human),
        produced_by_iteration_id: Set(None),
        verdict: Set(None),
        attempt: Set(0),
        last_failure_sig: Set(None),
        sort: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&txn)
    .await?;

    loop_artifact_revision::ActiveModel {
        artifact_id: Set(root.id),
        seq: Set(1),
        content: Set(description.to_string()),
        actor_kind: Set(ActorKind::Human),
        iteration_id: Set(None),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(&txn)
    .await?;

    txn.commit().await?;
    Ok(to_issue_detail(issue))
}

pub async fn get_issue(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
) -> Result<Option<loop_issue::Model>, DbError> {
    Ok(loop_issue::Entity::find_by_id(id).one(conn).await?)
}

pub async fn get_issue_detail(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
) -> Result<Option<LoopIssueDetail>, DbError> {
    Ok(loop_issue::Entity::find_by_id(id)
        .one(conn)
        .await?
        .map(to_issue_detail))
}

pub async fn list_issues(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
    statuses: Option<Vec<IssueStatus>>,
) -> Result<Vec<LoopIssueRow>, DbError> {
    let mut query = loop_issue::Entity::find()
        .filter(loop_issue::Column::SpaceId.eq(space_id))
        .order_by_desc(loop_issue::Column::SeqNo);
    if let Some(statuses) = statuses {
        if !statuses.is_empty() {
            query = query.filter(loop_issue::Column::Status.is_in(statuses));
        }
    }
    Ok(query
        .all(conn)
        .await?
        .iter()
        .map(to_issue_row)
        .collect())
}

pub async fn delete_issue(conn: &sea_orm::DatabaseConnection, id: i32) -> Result<(), DbError> {
    loop_issue::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn update_issue_config(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
    config: &IssueConfig,
    token_budget: Option<i64>,
    config_inherits: bool,
) -> Result<(), DbError> {
    let row = loop_issue::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| not_found(id))?;
    let mut active = row.into_active_model();
    // When inheriting, keep the stored `config` as the last custom value so a
    // later switch back to custom restores it; otherwise overwrite it.
    if !config_inherits {
        active.config = Set(serde_json::to_string(config).unwrap_or_else(|_| "{}".to_string()));
    }
    active.config_inherits = Set(config_inherits);
    active.token_budget = Set(token_budget);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

/// Flip an issue's `config_inherits` flag without touching its stored config.
/// Used at creation to mark a no-explicit-config issue as inheriting the space
/// default; the settings dialog uses [`update_issue_config`] instead.
pub async fn set_config_inherits(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
    inherits: bool,
) -> Result<(), DbError> {
    let row = loop_issue::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| not_found(id))?;
    let mut active = row.into_active_model();
    active.config_inherits = Set(inherits);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}
