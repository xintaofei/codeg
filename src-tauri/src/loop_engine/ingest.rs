//! Host-side trust boundary for codeg-mcp loop submissions.
//!
//! A loop agent is handed ONLY an opaque `capability_token`; it never sends ids
//! the host would trust. Every submission is reverse-looked-up to its iteration
//! by that token (rejecting unknown / non-running tokens — so stale, cancelled,
//! or already-settled iterations can't write), checked against a strict
//! stage→tool allow-table, validated to target the iteration's own issue, and
//! written idempotently (a replay from a retry or crash recovery produces the
//! same rows, never duplicates).
//!
//! This module is the authority for what an agent may persist; the companion /
//! transport / listener layers only ferry the `(token, tool, payload)` triple
//! here.

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set,
};
use serde_json::{json, Value};

use crate::acp::delegation::listener::LoopIngestAccess;
use crate::db::entities::loop_artifact::{self, ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_artifact_revision::{self, ActorKind};
use crate::db::entities::loop_inbox_item::InboxKind;
use crate::db::entities::loop_issue::{self, IssuePriority, IssueRoute};
use crate::db::entities::loop_iteration::{self, IterationStatus, Stage};
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::loop_memory::MemoryKind;
use crate::db::service::loop_service;
use crate::loop_engine::LoopError;

/// Hard ceiling on any single persisted text field (defense against a runaway
/// agent flooding the DB). Counted in characters.
const MAX_CONTENT: usize = 200_000;

/// Per-iteration write safety thresholds (§2.9). Generous — these defend the DB
/// against a runaway agent, not against legitimate large runs ("no artificial
/// limits"): a sane iteration produces a handful of artifacts well under these.
/// `MAX_BYTES_PER_ITERATION` (4 MB) exceeds `MAX_CONTENT` (200 KB) so per-field
/// truncation and the per-iteration budget stay independent guards.
const MAX_ARTIFACTS_PER_ITERATION: usize = 200;
const MAX_BYTES_PER_ITERATION: usize = 4_000_000;

fn invalid(msg: impl Into<String>) -> LoopError {
    LoopError::InvalidInput(msg.into())
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_CONTENT {
        s.to_string()
    } else {
        s.chars().take(MAX_CONTENT).collect::<String>() + "\n…[truncated]"
    }
}

/// Reverse-look-up the iteration backing a capability token. Rejects unknown
/// tokens and any iteration not currently `running` (the agent's window to
/// write is exactly its live turn).
async fn running_iteration(
    conn: &DatabaseConnection,
    token: &str,
) -> Result<loop_iteration::Model, LoopError> {
    let it = loop_iteration::Entity::find()
        .filter(loop_iteration::Column::CapabilityToken.eq(token))
        .one(conn)
        .await?
        .ok_or_else(|| invalid("unknown capability token"))?;
    if it.status != IterationStatus::Running {
        return Err(invalid("iteration is not accepting submissions"));
    }
    Ok(it)
}

/// Which artifact kind a stage is allowed to produce. The read stages produce
/// their pipeline node; finalize produces the issue's `result`. Other stages
/// have no `loop_submit_artifacts` capability.
fn artifact_kind_for_stage(stage: Stage) -> Result<ArtifactKind, LoopError> {
    match stage {
        Stage::Refine => Ok(ArtifactKind::Requirement),
        Stage::Design => Ok(ArtifactKind::Design),
        Stage::Plan => Ok(ArtifactKind::Task),
        Stage::Finalize => Ok(ArtifactKind::Result),
        other => Err(invalid(format!("stage {other:?} cannot submit artifacts"))),
    }
}

/// Initial status by kind: tasks land `pending` (awaiting implement); a design
/// lands `awaiting_approval` (the human design gate — the driver files the inbox
/// card and planning waits until a person approves); requirement / result are
/// accepted outputs (`done`).
fn default_status_for_kind(kind: ArtifactKind) -> ArtifactStatus {
    match kind {
        ArtifactKind::Task => ArtifactStatus::Pending,
        ArtifactKind::Design => ArtifactStatus::AwaitingApproval,
        _ => ArtifactStatus::Done,
    }
}

/// Entry point: validate `(token, tool, payload)` and persist. Returns a small
/// JSON outcome the companion relays back to the agent.
pub async fn ingest(
    conn: &DatabaseConnection,
    token: &str,
    tool: &str,
    payload: &Value,
) -> Result<Value, LoopError> {
    let it = running_iteration(conn, token).await?;
    match tool {
        "loop_submit_route" => submit_route(conn, &it, payload).await,
        "loop_submit_artifacts" => submit_artifacts(conn, &it, payload).await,
        "loop_submit_review" => submit_review(conn, &it, payload).await,
        "loop_report_blocked" => report_blocked(conn, &it, payload).await,
        "loop_record_memory" => record_memory(conn, &it, payload).await,
        other => Err(invalid(format!("unknown loop tool: {other}"))),
    }
}

/// Production [`LoopIngestAccess`] over the shared database — the bridge the
/// delegation listener calls for `loop_submit_*` traffic. Holds a cheap
/// `DatabaseConnection` clone (a connection-pool handle) and wraps [`ingest`],
/// flattening `LoopError` to its display string so the listener boundary stays
/// free of the loop error type.
pub struct DbLoopIngest {
    pub conn: DatabaseConnection,
}

#[async_trait]
impl LoopIngestAccess for DbLoopIngest {
    async fn loop_ingest(
        &self,
        token: &str,
        tool: &str,
        payload: &Value,
    ) -> Result<Value, String> {
        ingest(&self.conn, token, tool, payload)
            .await
            .map_err(|e| e.to_string())
    }
}

async fn submit_route(
    conn: &DatabaseConnection,
    it: &loop_iteration::Model,
    payload: &Value,
) -> Result<Value, LoopError> {
    if it.stage != Stage::Triage {
        return Err(invalid("loop_submit_route is only valid during triage"));
    }
    let route_str = payload
        .get("route")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid("missing route"))?;
    let route: IssueRoute =
        serde_json::from_value(json!(route_str)).map_err(|_| invalid("invalid route"))?;
    let priority = match payload.get("priority").and_then(|v| v.as_str()) {
        Some(p) => Some(
            serde_json::from_value::<IssuePriority>(json!(p))
                .map_err(|_| invalid("invalid priority"))?,
        ),
        None => None,
    };

    let issue = loop_issue::Entity::find_by_id(it.issue_id)
        .one(conn)
        .await?
        .ok_or_else(|| LoopError::NotFound(format!("issue {}", it.issue_id)))?;
    let mut active = issue.into_active_model();
    active.route = Set(route);
    if let Some(p) = priority {
        active.priority = Set(p);
    }
    active.update(conn).await?;

    Ok(json!({ "ok": true, "route": route_str }))
}

/// Current persisted footprint of an iteration: (#artifacts it produced, total
/// chars across all their revisions). Backs the §2.9 per-iteration write budget.
async fn iteration_footprint(
    conn: &DatabaseConnection,
    iteration_id: i32,
) -> Result<(usize, usize), LoopError> {
    let arts = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::ProducedByIterationId.eq(iteration_id))
        .all(conn)
        .await?;
    if arts.is_empty() {
        return Ok((0, 0));
    }
    let ids: Vec<i32> = arts.iter().map(|a| a.id).collect();
    let bytes: usize = loop_artifact_revision::Entity::find()
        .filter(loop_artifact_revision::Column::ArtifactId.is_in(ids))
        .all(conn)
        .await?
        .iter()
        .map(|r| r.content.chars().count())
        .sum();
    Ok((arts.len(), bytes))
}

async fn submit_artifacts(
    conn: &DatabaseConnection,
    it: &loop_iteration::Model,
    payload: &Value,
) -> Result<Value, LoopError> {
    let kind = artifact_kind_for_stage(it.stage)?;

    // Idempotency: this iteration already produced its batch → return it.
    let existing: Vec<i32> = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::ProducedByIterationId.eq(it.id))
        .filter(loop_artifact::Column::Kind.eq(kind))
        .all(conn)
        .await?
        .into_iter()
        .map(|a| a.id)
        .collect();
    if !existing.is_empty() {
        return Ok(json!({ "ok": true, "idempotent": true, "ids": existing }));
    }

    let items = payload
        .get("artifacts")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid("missing artifacts array"))?;
    if items.is_empty() {
        return Err(invalid("artifacts array is empty"));
    }

    // §2.9 write-budget guard: reject a batch that would push this iteration over
    // the per-iteration safety threshold, and surface it as a blocked card so the
    // human sees it. Generous bounds — runaway defense, not a perf cap.
    let (have_count, have_bytes) = iteration_footprint(conn, it.id).await?;
    let add_bytes: usize = items
        .iter()
        .map(|i| {
            i.get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.chars().count())
                .unwrap_or(0)
        })
        .sum();
    if have_count + items.len() > MAX_ARTIFACTS_PER_ITERATION
        || have_bytes + add_bytes > MAX_BYTES_PER_ITERATION
    {
        loop_service::inbox::upsert_inbox(
            conn,
            it.space_id,
            it.issue_id,
            Some(it.id),
            InboxKind::Blocked,
            &format!("write_budget_exceeded:{}", it.id),
            json!({
                "v": 1,
                "reason": "write_budget_exceeded",
                "have_artifacts": have_count,
                "have_bytes": have_bytes,
                "add_artifacts": items.len(),
                "add_bytes": add_bytes,
            }),
        )
        .await?;
        return Err(invalid("iteration write budget exceeded"));
    }

    // Edge wiring depends on kind: a `result` (finalize) fans out `results_from`
    // to every task of the issue; read artifacts derive from the iteration's
    // single target node.
    let derive_target = if kind == ArtifactKind::Result {
        None
    } else {
        let target = it
            .target_artifact_id
            .ok_or_else(|| invalid("iteration has no target node"))?;
        let target_row = loop_artifact::Entity::find_by_id(target)
            .one(conn)
            .await?
            .ok_or_else(|| invalid("target node not found"))?;
        if target_row.issue_id != it.issue_id {
            return Err(invalid("target node belongs to another issue"));
        }
        Some(target)
    };
    let result_targets: Vec<i32> = if kind == ArtifactKind::Result {
        loop_artifact::Entity::find()
            .filter(loop_artifact::Column::IssueId.eq(it.issue_id))
            .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Task))
            .all(conn)
            .await?
            .into_iter()
            .map(|t| t.id)
            .collect()
    } else {
        Vec::new()
    };

    let status = default_status_for_kind(kind);
    let mut ids = Vec::new();
    for item in items {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Untitled");
        let content = truncate(item.get("content").and_then(|v| v.as_str()).unwrap_or(""));

        let art = loop_service::artifact::create_artifact(
            conn,
            it.space_id,
            it.issue_id,
            kind,
            title,
            status,
            ActorKind::Agent,
            Some(it.id),
        )
        .await?;
        loop_service::artifact::add_revision(conn, art.id, &content, ActorKind::Agent, Some(it.id))
            .await?;
        if let Some(criteria) = item.get("criteria").and_then(|v| v.as_array()) {
            for c in criteria {
                if let Some(text) = c.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    loop_service::artifact::add_criterion(conn, art.id, text).await?;
                }
            }
        }
        // Canonical edge direction: from = derived/result node, to = its source.
        if let Some(target) = derive_target {
            loop_service::link::create_link(
                conn,
                it.space_id,
                art.id,
                target,
                LinkKind::DerivesFrom,
            )
            .await?;
        }
        for task_id in &result_targets {
            loop_service::link::create_link(
                conn,
                it.space_id,
                art.id,
                *task_id,
                LinkKind::ResultsFrom,
            )
            .await?;
        }
        ids.push(art.id);
    }

    Ok(json!({ "ok": true, "ids": ids }))
}

async fn submit_review(
    conn: &DatabaseConnection,
    it: &loop_iteration::Model,
    payload: &Value,
) -> Result<Value, LoopError> {
    if it.stage != Stage::Review {
        return Err(invalid("loop_submit_review is only valid during review"));
    }
    let target = it
        .target_artifact_id
        .ok_or_else(|| invalid("review iteration has no target task"))?;

    // Idempotency: this review slot already submitted its verdict.
    if let Some(existing) = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::ProducedByIterationId.eq(it.id))
        .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Review))
        .one(conn)
        .await?
    {
        return Ok(json!({ "ok": true, "idempotent": true, "id": existing.id }));
    }

    let verdict_str = payload
        .get("verdict")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid("missing verdict"))?;
    let verdict: ReviewVerdict =
        serde_json::from_value(json!(verdict_str)).map_err(|_| invalid("invalid verdict"))?;
    let findings = truncate(payload.get("findings").and_then(|v| v.as_str()).unwrap_or(""));

    let title = format!("Review (slot {})", it.slot_no.unwrap_or(0));
    let art = loop_service::artifact::create_artifact(
        conn,
        it.space_id,
        it.issue_id,
        ArtifactKind::Review,
        &title,
        ArtifactStatus::Done,
        ActorKind::Agent,
        Some(it.id),
    )
    .await?;
    let mut active = art.clone().into_active_model();
    active.verdict = Set(Some(verdict));
    active.update(conn).await?;

    loop_service::artifact::add_revision(conn, art.id, &findings, ActorKind::Agent, Some(it.id))
        .await?;
    loop_service::link::create_link(conn, it.space_id, art.id, target, LinkKind::Reviews).await?;

    Ok(json!({ "ok": true, "id": art.id, "verdict": verdict_str }))
}

async fn report_blocked(
    conn: &DatabaseConnection,
    it: &loop_iteration::Model,
    payload: &Value,
) -> Result<Value, LoopError> {
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("agent reported blocked");
    let subject = match it.target_artifact_id {
        Some(target) => format!("artifact:{target}"),
        None => format!("issue:{}", it.issue_id),
    };
    let payload_json = json!({
        "v": 1,
        "reason": truncate(reason),
        "iteration_id": it.id,
    });
    loop_service::inbox::upsert_inbox(
        conn,
        it.space_id,
        it.issue_id,
        Some(it.id),
        InboxKind::Blocked,
        &subject,
        payload_json,
    )
    .await?;
    Ok(json!({ "ok": true }))
}

async fn record_memory(
    conn: &DatabaseConnection,
    it: &loop_iteration::Model,
    payload: &Value,
) -> Result<Value, LoopError> {
    // Agents may propose constraint/decision/preference/pitfall memories — never
    // the space constitution (human-authored only); anything unrecognized falls
    // back to a pitfall note.
    let kind = match payload.get("kind").and_then(|v| v.as_str()) {
        Some(k) => serde_json::from_value::<MemoryKind>(json!(k)).unwrap_or(MemoryKind::Pitfall),
        None => MemoryKind::Pitfall,
    };
    let kind = if matches!(kind, MemoryKind::Constitution) {
        MemoryKind::Pitfall
    } else {
        kind
    };
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Note");
    let content = payload.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if content.trim().is_empty() {
        return Err(invalid("memory content is empty"));
    }
    let m = loop_service::memory::create_memory(
        conn,
        it.space_id,
        kind,
        ActorKind::Agent,
        title,
        &truncate(content),
    )
    .await?;
    Ok(json!({ "ok": true, "id": m.id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::loop_issue::IssuePriority as Prio;
    use crate::db::service::loop_service;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::loop_engine::transitions::{self, IterationClaim};
    use crate::models::loops::IssueConfig;

    /// Create space + issue, returning (conn-owning db, space_id, issue_id,
    /// root_artifact_id).
    async fn seed() -> (crate::db::AppDatabase, i32, i32, i32) {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/repo").await;
        let space = loop_service::space::create_space(&db.conn, "S", folder_id)
            .await
            .unwrap();
        let issue = loop_service::issue::create_issue(
            &db.conn,
            space.id,
            "Build",
            "desc",
            Prio::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        // The root issue artifact is the only kind=issue node.
        let dag = loop_service::artifact::list_dag(&db.conn, issue.row.id)
            .await
            .unwrap();
        let root = dag
            .artifacts
            .iter()
            .find(|a| matches!(a.kind, ArtifactKind::Issue))
            .expect("root issue artifact")
            .id;
        (db, space.id, issue.row.id, root)
    }

    async fn running_iter(
        conn: &DatabaseConnection,
        space_id: i32,
        issue_id: i32,
        stage: Stage,
        target: Option<i32>,
        token: &str,
    ) -> i32 {
        let it = transitions::try_claim_iteration(
            conn,
            IterationClaim {
                space_id,
                issue_id,
                stage,
                target_artifact_id: target,
                slot_no: if stage == Stage::Review { Some(0) } else { None },
                capability_token: token.to_string(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .expect("claimed iteration");
        transitions::cas_iteration_status(
            conn,
            it.id,
            IterationStatus::Queued,
            IterationStatus::Running,
        )
        .await
        .unwrap();
        it.id
    }

    #[tokio::test]
    async fn unknown_token_is_rejected() {
        let (db, _s, _i, _root) = seed().await;
        let err = ingest(&db.conn, "nope", "loop_submit_route", &json!({"route":"full"}))
            .await
            .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn route_written_only_from_triage() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Triage, Some(root), "tok-triage").await;
        ingest(
            &db.conn,
            "tok-triage",
            "loop_submit_route",
            &json!({"route":"skip_design","priority":"high"}),
        )
        .await
        .unwrap();
        let updated = loop_service::issue::get_issue(&db.conn, issue)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.route, IssueRoute::SkipDesign);
        assert_eq!(updated.priority, IssuePriority::High);

        // A refine iteration may not submit a route.
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok-refine").await;
        let err = ingest(&db.conn, "tok-refine", "loop_submit_route", &json!({"route":"full"}))
            .await
            .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn submit_artifacts_rejects_when_over_write_budget() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok").await;
        // One artifact whose content exceeds MAX_BYTES_PER_ITERATION (chars).
        let huge = "x".repeat(MAX_BYTES_PER_ITERATION + 1);
        let payload = json!({ "artifacts": [ { "title": "Big", "content": huge } ] });
        let err = ingest(&db.conn, "tok", "loop_submit_artifacts", &payload)
            .await
            .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
        // A blocked card was filed for the human.
        let items = loop_service::inbox::list_inbox(&db.conn, space, None)
            .await
            .unwrap();
        assert!(items.iter().any(|i| matches!(i.kind, InboxKind::Blocked)));
    }

    #[tokio::test]
    async fn artifacts_create_nodes_and_edges_then_idempotent() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok").await;

        let payload = json!({
            "artifacts": [
                {"title": "Req A", "content": "shall A", "criteria": ["AC one", "AC two"]},
                {"title": "Req B", "content": "shall B"}
            ]
        });
        let out = ingest(&db.conn, "tok", "loop_submit_artifacts", &payload)
            .await
            .unwrap();
        let ids = out["ids"].as_array().unwrap();
        assert_eq!(ids.len(), 2);

        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        let reqs: Vec<_> = dag
            .artifacts
            .iter()
            .filter(|a| matches!(a.kind, ArtifactKind::Requirement))
            .collect();
        assert_eq!(reqs.len(), 2);
        // Both derive_from the root.
        let derive_edges = dag
            .links
            .iter()
            .filter(|l| matches!(l.kind, LinkKind::DerivesFrom) && l.to_artifact_id == root)
            .count();
        assert_eq!(derive_edges, 2);

        // Replay → idempotent, no duplicates.
        let again = ingest(&db.conn, "tok", "loop_submit_artifacts", &payload)
            .await
            .unwrap();
        assert_eq!(again["idempotent"], json!(true));
        let dag2 = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        assert_eq!(
            dag2.artifacts
                .iter()
                .filter(|a| matches!(a.kind, ArtifactKind::Requirement))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn stage_kind_mismatch_is_rejected() {
        let (db, space, issue, root) = seed().await;
        // A review iteration cannot submit artifacts.
        let _ = running_iter(&db.conn, space, issue, Stage::Review, Some(root), "tok-rev").await;
        let err = ingest(
            &db.conn,
            "tok-rev",
            "loop_submit_artifacts",
            &json!({"artifacts":[{"title":"x","content":"y"}]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn review_records_verdict_and_edge() {
        let (db, space, issue, root) = seed().await;
        // Make a task to review (refine stage abused here just to mint a node).
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-plan").await;
        let plan_out = ingest(
            &db.conn,
            "tok-plan",
            "loop_submit_artifacts",
            &json!({"artifacts":[{"title":"Task 1","content":"do"}]}),
        )
        .await
        .unwrap();
        let task_id = plan_out["ids"][0].as_i64().unwrap() as i32;

        let _ = running_iter(&db.conn, space, issue, Stage::Review, Some(task_id), "tok-review")
            .await;
        let out = ingest(
            &db.conn,
            "tok-review",
            "loop_submit_review",
            &json!({"verdict":"pass","findings":"looks good"}),
        )
        .await
        .unwrap();
        let review_id = out["id"].as_i64().unwrap() as i32;

        let detail = loop_service::artifact::get_artifact_detail(&db.conn, review_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.row.verdict, Some(ReviewVerdict::Pass));
        assert!(detail
            .links
            .iter()
            .any(|l| matches!(l.kind, LinkKind::Reviews) && l.to_artifact_id == task_id));
    }
}
