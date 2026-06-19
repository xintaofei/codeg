use std::collections::HashMap;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_criterion::CriterionKind;
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::{
    conversation, loop_artifact, loop_artifact_revision, loop_criterion, loop_issue,
    loop_iteration, loop_link,
};
use crate::db::error::DbError;
use crate::models::loops::{
    ArtifactIterationRef, LoopArtifactDetail, LoopArtifactRow, LoopCriterionRow, LoopDagView,
    LoopLinkRow, LoopRevision,
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
        contribution_kind: m.contribution_kind,
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
        kind: m.kind,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_artifact(
    conn: &impl sea_orm::ConnectionTrait,
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
    conn: &impl sea_orm::ConnectionTrait,
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

/// The id of the artifact's most recent revision (highest seq), if any. Used to
/// bind a design→requirement lineage edge to the exact requirement content the
/// design derived from.
pub async fn latest_revision_id(
    conn: &sea_orm::DatabaseConnection,
    artifact_id: i32,
) -> Result<Option<i32>, DbError> {
    Ok(loop_artifact_revision::Entity::find()
        .filter(loop_artifact_revision::Column::ArtifactId.eq(artifact_id))
        .order_by_desc(loop_artifact_revision::Column::Seq)
        .one(conn)
        .await?
        .map(|m| m.id))
}

/// Auto-labels `AC-{n}` and appends at the end. `kind` types the criterion
/// (acceptance for requirements/tasks; constraint/invariant/obligation for
/// designs) — ingest enforces the per-artifact-kind allow-set before calling.
pub async fn add_criterion(
    conn: &impl sea_orm::ConnectionTrait,
    artifact_id: i32,
    kind: CriterionKind,
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
        kind: Set(kind),
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
    // Read the whole view inside one transaction so every slice — artifacts,
    // links, coverage, checks, gate decisions, and in-flight iterations — is a
    // single consistent snapshot. Without it, an iteration settling mid-read
    // could be captured as NEITHER a ghost (already gone from the live set) nor
    // its landed artifact (read before it appeared), making the node blink out of
    // the DAG/board for one poll. The frontend dedups the "both present" overlap
    // (ghost vs. landed artifact) by `produced_by_iteration_id`, so a snapshot
    // that errs toward showing both is safe; one that shows neither is not.
    let txn = conn.begin().await?;

    let issue_seq = loop_issue::Entity::find_by_id(issue_id)
        .one(&txn)
        .await?
        .map(|i| i.seq_no)
        .unwrap_or(0);

    let artifact_models = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::IssueId.eq(issue_id))
        .order_by_asc(loop_artifact::Column::Id)
        .all(&txn)
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
            .all(&txn)
            .await?
            .into_iter()
            .map(to_link_row)
            .collect()
    };

    let coverage = super::coverage::list_for_issue(&txn, issue_id).await?;
    let criterion_checks = super::criterion_check::list_for_issue(&txn, issue_id).await?;
    let gate_decisions = super::gate_decision::list_for_issue(&txn, issue_id).await?;
    let live_iterations = super::iteration::list_live_for_issue(&txn, issue_id).await?;

    // P3 agent facet: resolve each artifact's producing iteration WITHIN this issue
    // so the graph can overlay the agent/session + a per-artifact attempt count.
    // Bounded — one pass over the issue's iterations plus two batched queries. Only
    // artifacts whose `produced_by_iteration_id` resolves to an in-issue iteration
    // get a ref; orphan / cross-issue references are omitted, and the frontend
    // infers an unresolved producer from "facet on, but no ref for this node".
    let iterations = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::IssueId.eq(issue_id))
        .all(&txn)
        .await?;
    let iter_by_id: HashMap<i32, &loop_iteration::Model> =
        iterations.iter().map(|m| (m.id, m)).collect();
    // Per-target attempt counts (task/requirement/design/reflection) and the
    // finalize count (result). Review's count is always 1 (its single producer).
    let mut count_by_target: HashMap<i32, i32> = HashMap::new();
    let mut finalize_count = 0i32;
    for it in &iterations {
        if let Some(tid) = it.target_artifact_id {
            *count_by_target.entry(tid).or_insert(0) += 1;
        }
        if it.stage == loop_iteration::Stage::Finalize {
            finalize_count += 1;
        }
    }
    // agent_type for the producing iterations' conversations (one batched query).
    let producing_conv_ids: Vec<i32> = artifact_models
        .iter()
        .filter_map(|a| a.produced_by_iteration_id)
        .filter_map(|iid| iter_by_id.get(&iid).copied())
        .filter_map(|it| it.conversation_id)
        .collect();
    let conv_agent: HashMap<i32, String> = if producing_conv_ids.is_empty() {
        HashMap::new()
    } else {
        conversation::Entity::find()
            .filter(conversation::Column::Id.is_in(producing_conv_ids))
            .all(&txn)
            .await?
            .into_iter()
            .map(|c| (c.id, c.agent_type))
            .collect()
    };
    let artifact_iteration_refs: Vec<ArtifactIterationRef> = artifact_models
        .iter()
        .filter_map(|a| {
            let it = *iter_by_id.get(&a.produced_by_iteration_id?)?;
            let attempt_count = match a.kind {
                ArtifactKind::Result => finalize_count,
                ArtifactKind::Review => 1,
                // An issue artifact is produced by triage (which targets NULL),
                // so nothing ever targets it — its attempt count is always 0.
                ArtifactKind::Issue => 0,
                _ => count_by_target.get(&a.id).copied().unwrap_or(0),
            };
            let agent_type = it
                .conversation_id
                .and_then(|cid| conv_agent.get(&cid).cloned());
            Some(ArtifactIterationRef {
                artifact_id: a.id,
                iteration_id: it.id,
                stage: it.stage,
                status: it.status,
                outcome: it.outcome,
                agent_type,
                conversation_id: it.conversation_id,
                attempt_count,
            })
        })
        .collect();

    txn.commit().await?;

    Ok(LoopDagView {
        artifacts,
        links,
        coverage,
        criterion_checks,
        gate_decisions,
        live_iterations,
        artifact_iteration_refs,
    })
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
