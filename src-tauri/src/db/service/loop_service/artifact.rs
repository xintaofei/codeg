use std::collections::HashMap;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::{
    loop_artifact, loop_artifact_revision, loop_criterion, loop_issue, loop_iteration, loop_link,
};
use crate::db::error::DbError;
use crate::models::loops::{
    LoopArtifactDetail, LoopArtifactRow, LoopCriterionRow, LoopDagView, LoopLinkRow, LoopRevision,
};

use super::link::to_link_row;

pub fn to_artifact_row(m: &loop_artifact::Model, issue_seq: i32) -> LoopArtifactRow {
    LoopArtifactRow {
        id: m.id,
        issue_id: m.issue_id,
        issue_seq,
        kind: m.kind,
        title: m.title.clone(),
        status: m.status,
        origin: m.origin,
        produced_by_iteration_id: m.produced_by_iteration_id,
        verdict: m.verdict,
        attempt: m.attempt,
        sort: m.sort,
        updated_at: m.updated_at,
    }
}

fn to_revision(m: loop_artifact_revision::Model) -> LoopRevision {
    LoopRevision {
        id: m.id,
        seq: m.seq,
        content: m.content,
        actor_kind: m.actor_kind,
        iteration_id: m.iteration_id,
        created_at: m.created_at,
    }
}

fn to_criterion_row(m: loop_criterion::Model) -> LoopCriterionRow {
    LoopCriterionRow {
        id: m.id,
        label: m.label,
        text: m.text,
        sort: m.sort,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_artifact(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
    issue_id: i32,
    kind: ArtifactKind,
    title: &str,
    status: ArtifactStatus,
    origin: ActorKind,
    produced_by_iteration_id: Option<i32>,
) -> Result<loop_artifact::Model, DbError> {
    let now = Utc::now();
    let sort = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::IssueId.eq(issue_id))
        .filter(loop_artifact::Column::Kind.eq(kind))
        .order_by_desc(loop_artifact::Column::Sort)
        .one(conn)
        .await?
        .map(|m| m.sort + 1)
        .unwrap_or(0);
    Ok(loop_artifact::ActiveModel {
        space_id: Set(space_id),
        issue_id: Set(issue_id),
        kind: Set(kind),
        title: Set(title.to_string()),
        status: Set(status),
        origin: Set(origin),
        produced_by_iteration_id: Set(produced_by_iteration_id),
        verdict: Set(None),
        attempt: Set(0),
        last_failure_sig: Set(None),
        sort: Set(sort),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(conn)
    .await?)
}

pub async fn add_revision(
    conn: &sea_orm::DatabaseConnection,
    artifact_id: i32,
    content: &str,
    actor_kind: ActorKind,
    iteration_id: Option<i32>,
) -> Result<loop_artifact_revision::Model, DbError> {
    let seq = loop_artifact_revision::Entity::find()
        .filter(loop_artifact_revision::Column::ArtifactId.eq(artifact_id))
        .order_by_desc(loop_artifact_revision::Column::Seq)
        .one(conn)
        .await?
        .map(|m| m.seq + 1)
        .unwrap_or(1);
    Ok(loop_artifact_revision::ActiveModel {
        artifact_id: Set(artifact_id),
        seq: Set(seq),
        content: Set(content.to_string()),
        actor_kind: Set(actor_kind),
        iteration_id: Set(iteration_id),
        created_at: Set(Utc::now()),
        ..Default::default()
    }
    .insert(conn)
    .await?)
}

/// Auto-labels `AC-{n}` and appends at the end.
pub async fn add_criterion(
    conn: &sea_orm::DatabaseConnection,
    artifact_id: i32,
    text: &str,
) -> Result<loop_criterion::Model, DbError> {
    let next = loop_criterion::Entity::find()
        .filter(loop_criterion::Column::ArtifactId.eq(artifact_id))
        .order_by_desc(loop_criterion::Column::Sort)
        .one(conn)
        .await?
        .map(|m| m.sort + 1)
        .unwrap_or(0);
    Ok(loop_criterion::ActiveModel {
        artifact_id: Set(artifact_id),
        label: Set(format!("AC-{}", next + 1)),
        text: Set(text.to_string()),
        sort: Set(next),
        ..Default::default()
    }
    .insert(conn)
    .await?)
}

pub async fn get_artifact_detail(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
) -> Result<Option<LoopArtifactDetail>, DbError> {
    let Some(artifact) = loop_artifact::Entity::find_by_id(id).one(conn).await? else {
        return Ok(None);
    };
    let issue_seq = loop_issue::Entity::find_by_id(artifact.issue_id)
        .one(conn)
        .await?
        .map(|i| i.seq_no)
        .unwrap_or(0);

    let revisions = loop_artifact_revision::Entity::find()
        .filter(loop_artifact_revision::Column::ArtifactId.eq(id))
        .order_by_asc(loop_artifact_revision::Column::Seq)
        .all(conn)
        .await?
        .into_iter()
        .map(to_revision)
        .collect();

    let criteria = loop_criterion::Entity::find()
        .filter(loop_criterion::Column::ArtifactId.eq(id))
        .order_by_asc(loop_criterion::Column::Sort)
        .all(conn)
        .await?
        .into_iter()
        .map(to_criterion_row)
        .collect();

    // Edges touching this node in either direction.
    let links: Vec<LoopLinkRow> = loop_link::Entity::find()
        .filter(
            loop_link::Column::FromArtifactId
                .eq(id)
                .or(loop_link::Column::ToArtifactId.eq(id)),
        )
        .all(conn)
        .await?
        .into_iter()
        .map(to_link_row)
        .collect();

    Ok(Some(LoopArtifactDetail {
        row: to_artifact_row(&artifact, issue_seq),
        revisions,
        criteria,
        links,
    }))
}

pub async fn list_dag(
    conn: &sea_orm::DatabaseConnection,
    issue_id: i32,
) -> Result<LoopDagView, DbError> {
    let issue_seq = loop_issue::Entity::find_by_id(issue_id)
        .one(conn)
        .await?
        .map(|i| i.seq_no)
        .unwrap_or(0);

    let artifact_models = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::IssueId.eq(issue_id))
        .order_by_asc(loop_artifact::Column::Id)
        .all(conn)
        .await?;
    let artifact_ids: Vec<i32> = artifact_models.iter().map(|m| m.id).collect();
    let artifacts = artifact_models
        .iter()
        .map(|m| to_artifact_row(m, issue_seq))
        .collect();

    // Every edge of this issue's DAG has its `from` node inside the issue.
    let links = if artifact_ids.is_empty() {
        Vec::new()
    } else {
        loop_link::Entity::find()
            .filter(loop_link::Column::FromArtifactId.is_in(artifact_ids))
            .all(conn)
            .await?
            .into_iter()
            .map(to_link_row)
            .collect()
    };

    Ok(LoopDagView { artifacts, links })
}

pub async fn list_artifacts_for_space(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
) -> Result<Vec<LoopArtifactRow>, DbError> {
    let seqs: HashMap<i32, i32> = loop_issue::Entity::find()
        .filter(loop_issue::Column::SpaceId.eq(space_id))
        .all(conn)
        .await?
        .into_iter()
        .map(|i| (i.id, i.seq_no))
        .collect();

    Ok(loop_artifact::Entity::find()
        .filter(loop_artifact::Column::SpaceId.eq(space_id))
        .order_by_desc(loop_artifact::Column::Id)
        .all(conn)
        .await?
        .iter()
        .map(|m| to_artifact_row(m, *seqs.get(&m.issue_id).unwrap_or(&0)))
        .collect())
}

/// Findings text from the most recent FAIL-verdict reviews of a task — the round
/// that triggered rework — newest first. Fed into the next implement briefing so
/// the re-attempt addresses the reviewers' objections instead of repeating them.
///
/// "Most recent round" is scoped by the producing review iteration's `attempt`
/// (reviews are dispatched at the task's attempt), so stale findings from an
/// earlier round are excluded.
pub async fn latest_failed_review_findings(
    conn: &sea_orm::DatabaseConnection,
    task_artifact_id: i32,
) -> Result<Vec<String>, DbError> {
    // Review artifacts that point at this task.
    let review_ids: Vec<i32> = loop_link::Entity::find()
        .filter(loop_link::Column::ToArtifactId.eq(task_artifact_id))
        .filter(loop_link::Column::Kind.eq(LinkKind::Reviews))
        .all(conn)
        .await?
        .into_iter()
        .map(|l| l.from_artifact_id)
        .collect();
    if review_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut reviews = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::Id.is_in(review_ids))
        .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Review))
        .filter(loop_artifact::Column::Verdict.eq(ReviewVerdict::Fail))
        .all(conn)
        .await?;
    if reviews.is_empty() {
        return Ok(Vec::new());
    }

    // Scope to the latest round = highest producing-iteration attempt.
    let iter_ids: Vec<i32> = reviews
        .iter()
        .filter_map(|r| r.produced_by_iteration_id)
        .collect();
    let attempt_of: HashMap<i32, i32> = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::Id.is_in(iter_ids))
        .all(conn)
        .await?
        .into_iter()
        .map(|i| (i.id, i.attempt))
        .collect();
    let attempt = |r: &loop_artifact::Model| {
        r.produced_by_iteration_id
            .and_then(|id| attempt_of.get(&id).copied())
            .unwrap_or(0)
    };
    let max_attempt = reviews.iter().map(attempt).max().unwrap_or(0);
    reviews.retain(|r| attempt(r) == max_attempt);
    reviews.sort_by(|a, b| b.id.cmp(&a.id));

    let mut out = Vec::new();
    for r in reviews {
        if let Some(rev) = loop_artifact_revision::Entity::find()
            .filter(loop_artifact_revision::Column::ArtifactId.eq(r.id))
            .order_by_desc(loop_artifact_revision::Column::Id)
            .one(conn)
            .await?
        {
            let findings = rev.content.trim();
            if !findings.is_empty() {
                out.push(findings.to_string());
            }
        }
    }
    Ok(out)
}
