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

use std::collections::HashMap;

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use serde_json::{json, Value};

use crate::acp::delegation::listener::LoopIngestAccess;
use crate::db::entities::loop_artifact::{self, ArtifactKind, ArtifactStatus, ReviewVerdict};
use crate::db::entities::loop_artifact_revision::{self, ActorKind};
use crate::db::entities::loop_criterion::CriterionKind;
use crate::db::entities::loop_criterion_check::CheckVerdict;
use crate::db::entities::loop_inbox_item::InboxKind;
use crate::db::entities::loop_issue::{self, IssuePriority, IssueRoute};
use crate::db::entities::loop_iteration::{self, IterationStatus, Stage};
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::loop_memory::{MemoryKind, TrustTier};
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

/// Parse an item's optional `depends_on` into the index of its single
/// predecessor *within this same batch* (0-based, into the `artifacts` array).
///
/// v1 dependency model (spec §3.1, §4.1): a task may declare **at most one**
/// predecessor, referenced by its position earlier in the same submission. That
/// a reference can only point *backward* (`n < idx`) makes the batch acyclic by
/// construction and sidesteps cross-batch id resolution — so cross-issue and
/// forward/self references are structurally impossible, not just rejected.
/// `None` (absent / null / empty array) means a root task (no predecessor).
fn parse_depends_on(item: &Value, idx: usize) -> Result<Option<usize>, LoopError> {
    let Some(raw) = item.get("depends_on") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let arr = raw
        .as_array()
        .ok_or_else(|| invalid("depends_on must be an array"))?;
    if arr.is_empty() {
        return Ok(None);
    }
    if arr.len() > 1 {
        return Err(invalid(
            "a task may declare at most one predecessor (v1 forbids multiple dependencies)",
        ));
    }
    let n = arr[0]
        .as_i64()
        .ok_or_else(|| invalid("depends_on entry must be an integer batch index"))?;
    if n < 0 || (n as usize) >= idx {
        return Err(invalid(format!(
            "depends_on index {n} out of range; must reference an earlier task in this batch (0..{idx})"
        )));
    }
    Ok(Some(n as usize))
}

/// Parse + validate one item's `criteria`, typed by the batch artifact kind
/// (spec §3.1). Requirements and tasks carry only `acceptance`; designs carry
/// `constraint`/`invariant`/`obligation` (cross-cutting properties, never
/// acceptance); other kinds carry none. Each entry is a bare string (defaulted
/// by artifact kind) or `{ "text": str, "kind"?: str }`. A disallowed or
/// unparseable kind is rejected so the caller can abort the whole batch before
/// any write — same all-or-nothing contract as `depends_on`.
fn parse_criteria(
    item: &Value,
    kind: ArtifactKind,
) -> Result<Vec<(CriterionKind, String)>, LoopError> {
    let Some(raw) = item.get("criteria") else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let arr = raw
        .as_array()
        .ok_or_else(|| invalid("criteria must be an array"))?;

    // Default + allow-set per artifact kind.
    let (default_kind, allowed): (Option<CriterionKind>, &[CriterionKind]) = match kind {
        ArtifactKind::Requirement | ArtifactKind::Task => {
            (Some(CriterionKind::Acceptance), &[CriterionKind::Acceptance])
        }
        ArtifactKind::Design => (
            Some(CriterionKind::Constraint),
            &[
                CriterionKind::Constraint,
                CriterionKind::Invariant,
                CriterionKind::Obligation,
            ],
        ),
        _ => (None, &[]),
    };

    let mut out = Vec::new();
    for c in arr {
        let (text, explicit_kind) = if let Some(s) = c.as_str() {
            (s.trim().to_string(), None)
        } else if let Some(obj) = c.as_object() {
            let text = obj
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let k = match obj.get("kind").and_then(|v| v.as_str()) {
                Some(ks) => Some(
                    serde_json::from_value::<CriterionKind>(json!(ks))
                        .map_err(|_| invalid(format!("invalid criterion kind '{ks}'")))?,
                ),
                None => None,
            };
            (text, k)
        } else {
            return Err(invalid("criterion must be a string or an object"));
        };
        if text.is_empty() {
            continue;
        }
        let ck = explicit_kind
            .or(default_kind)
            .ok_or_else(|| invalid(format!("{kind:?} artifacts do not accept criteria")))?;
        if !allowed.contains(&ck) {
            return Err(invalid(format!(
                "criterion kind {ck:?} not allowed for {kind:?} artifacts"
            )));
        }
        out.push((ck, text));
    }
    Ok(out)
}

/// Resolve one task item's `covers` ordinals (e.g. `"R1.AC1"`) into the
/// criterion ids they name, against the issue's stable ordinal map (spec §3.3).
/// Up-front validation: an unknown or malformed ordinal is rejected so the
/// caller can abort the whole batch with no partial coverage rows. `None` /
/// null / empty array means the task covers nothing.
fn parse_covers(item: &Value, ordinals: &HashMap<String, i32>) -> Result<Vec<i32>, LoopError> {
    let Some(raw) = item.get("covers") else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let arr = raw
        .as_array()
        .ok_or_else(|| invalid("covers must be an array"))?;
    let mut out = Vec::new();
    for c in arr {
        let key = c
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("covers entry must be a non-empty ordinal like \"R1.AC1\""))?;
        let cid = ordinals.get(key).copied().ok_or_else(|| {
            invalid(format!("covers references unknown criterion ordinal '{key}'"))
        })?;
        out.push(cid);
    }
    Ok(out)
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

    // A design fans into EVERY done requirement of the issue (spec §3.2), not
    // just the iteration's single anchor node — so requirement criteria reach
    // implement/review through a real edge. Each edge is bound to the
    // requirement's latest revision (a content snapshot, so a later requirement
    // edit is detectable as stale lineage). Ordered by (sort, id) to match the
    // R{i} ordinals the plan stage references.
    let design_targets: Vec<(i32, Option<i32>)> = if kind == ArtifactKind::Design {
        let reqs = loop_artifact::Entity::find()
            .filter(loop_artifact::Column::IssueId.eq(it.issue_id))
            .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Requirement))
            .filter(loop_artifact::Column::Status.eq(ArtifactStatus::Done))
            .order_by_asc(loop_artifact::Column::Sort)
            .order_by_asc(loop_artifact::Column::Id)
            .all(conn)
            .await?;
        let mut out = Vec::with_capacity(reqs.len());
        for r in reqs {
            let rev = loop_service::artifact::latest_revision_id(conn, r.id).await?;
            out.push((r.id, rev));
        }
        out
    } else {
        Vec::new()
    };

    // Plan stage: the stable `R{i}.AC{j}` ordinals, built from the single shared
    // ordinal source so a task's `covers` references acceptance criteria by the
    // same ordinals the driver's coverage gate and the planner briefing use. The
    // agent never sees a DB id. `acceptance_ordinals` keeps canonical order (so a
    // missing-coverage list reads R1.AC1, R1.AC2, …); `covers_ordinals` is the
    // ordinal→id lookup map.
    let acceptance_ordinals: Vec<(String, i32)> = if kind == ArtifactKind::Task {
        let ordered =
            loop_service::coverage::acceptance_ordinals_for_issue(conn, it.issue_id).await?;
        let mut v = Vec::new();
        for (ri, (_req, crits)) in ordered.iter().enumerate() {
            for (ci, cid) in crits.iter().enumerate() {
                v.push((format!("R{}.AC{}", ri + 1, ci + 1), *cid));
            }
        }
        v
    } else {
        Vec::new()
    };
    let covers_ordinals: HashMap<String, i32> = acceptance_ordinals.iter().cloned().collect();

    // Validate every item's `depends_on` up-front, before any artifact is
    // written — a bad reference must abort the whole batch with no partial rows
    // (a partial write would then look "done" to the idempotency replay guard).
    // Only Task artifacts carry dependencies; depends_on on other kinds is
    // ignored.
    let dep_indices: Vec<Option<usize>> = if kind == ArtifactKind::Task {
        items
            .iter()
            .enumerate()
            .map(|(idx, item)| parse_depends_on(item, idx))
            .collect::<Result<_, _>>()?
    } else {
        vec![None; items.len()]
    };

    // Validate every item's criteria up-front (typed, per-artifact-kind
    // allow-set) — same all-or-nothing contract as depends_on: a disallowed kind
    // aborts the batch before any row is written.
    let criteria_per_item: Vec<Vec<(CriterionKind, String)>> = items
        .iter()
        .map(|item| parse_criteria(item, kind))
        .collect::<Result<_, _>>()?;

    // Validate every task's `covers` ordinals up-front — an unknown ordinal
    // aborts the batch before any coverage row is written.
    let covers_per_item: Vec<Vec<i32>> = if kind == ArtifactKind::Task {
        items
            .iter()
            .map(|item| parse_covers(item, &covers_ordinals))
            .collect::<Result<_, _>>()?
    } else {
        vec![Vec::new(); items.len()]
    };

    // Plan completeness (spec §3.3): every acceptance criterion must be covered by
    // at least one task. Enforced up-front (no write) so an incomplete plan is
    // REJECTED in the planner's own turn — it resubmits a complete one immediately,
    // converging in-turn — instead of being accepted and then superseded next tick
    // by the driver's coverage loop-back (which churns tasks and clutters the DAG).
    // Only bites when the issue actually has acceptance criteria (the `direct`
    // route has none, so `acceptance_ordinals` is empty and this is a no-op).
    if kind == ArtifactKind::Task && !acceptance_ordinals.is_empty() {
        let covered: std::collections::HashSet<i32> =
            covers_per_item.iter().flatten().copied().collect();
        let missing: Vec<&str> = acceptance_ordinals
            .iter()
            .filter(|(_, cid)| !covered.contains(cid))
            .map(|(ord, _)| ord.as_str())
            .collect();
        if !missing.is_empty() {
            return Err(invalid(format!(
                "plan leaves these acceptance criteria uncovered: {}. Every \
                 acceptance ordinal must be covered by at least one task's `covers`; \
                 resubmit the complete task list covering all of them.",
                missing.join(", ")
            )));
        }
    }

    let status = default_status_for_kind(kind);
    // One transaction for the whole batch: artifacts + revisions + criteria +
    // edges + coverage commit all-or-nothing. This is what makes the
    // "any produced artifact exists → idempotent replay" guard above CORRECT — a
    // crash mid-batch rolls everything back, so on replay either the full batch
    // is present (skip) or none of it is (rewrite). Without this, a crash after
    // an artifact but before its links/coverage would leave them permanently
    // skipped.
    let txn = conn.begin().await?;
    let mut ids = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Untitled");
        let content = truncate(item.get("content").and_then(|v| v.as_str()).unwrap_or(""));

        let art = loop_service::artifact::create_artifact(
            &txn,
            it.space_id,
            it.issue_id,
            kind,
            title,
            status,
            ActorKind::Agent,
            Some(it.id),
        )
        .await?;
        loop_service::artifact::add_revision(&txn, art.id, &content, ActorKind::Agent, Some(it.id))
            .await?;
        for (ck, text) in &criteria_per_item[idx] {
            loop_service::artifact::add_criterion(&txn, art.id, *ck, text).await?;
        }
        // Canonical edge direction: from = derived/result node, to = its source.
        if kind == ArtifactKind::Design {
            // Fan into every done requirement, each bound to its latest revision.
            for (req_id, rev) in &design_targets {
                loop_service::link::create_link(
                    &txn,
                    it.space_id,
                    art.id,
                    *req_id,
                    LinkKind::DerivesFrom,
                    *rev,
                )
                .await?;
            }
        } else if let Some(target) = derive_target {
            loop_service::link::create_link(
                &txn,
                it.space_id,
                art.id,
                target,
                LinkKind::DerivesFrom,
                None,
            )
            .await?;
        }
        for task_id in &result_targets {
            loop_service::link::create_link(
                &txn,
                it.space_id,
                art.id,
                *task_id,
                LinkKind::ResultsFrom,
                None,
            )
            .await?;
        }
        ids.push(art.id);
        // Wire the task dependency edge: from = this (successor) task, to = its
        // predecessor (already created earlier in this batch, so `ids[pred]`
        // exists). Validated above to be backward-only.
        if let Some(pred) = dep_indices[idx] {
            loop_service::link::create_link(
                &txn,
                it.space_id,
                art.id,
                ids[pred],
                LinkKind::DependsOn,
                None,
            )
            .await?;
        }
        // Criterion-level coverage: this task claims the acceptance criteria its
        // `covers` ordinals named (resolved + validated up-front). Idempotent on
        // replay via `uniq_loop_coverage`.
        for &cid in &covers_per_item[idx] {
            loop_service::coverage::create_coverage(&txn, it.space_id, art.id, cid).await?;
        }
    }
    txn.commit().await?;

    Ok(json!({ "ok": true, "ids": ids }))
}

/// A single resolved check, validated against the iteration's injected manifest.
struct ResolvedCheck {
    criterion_id: i32,
    verdict: CheckVerdict,
    evidence: String,
}

/// Review submission (§3.4): the reviewer emits ONE structured check per injected
/// acceptance-criterion handle (`{criterion, verdict, evidence}`), NOT a holistic
/// verdict. Each handle is resolved against the **persisted** criterion manifest
/// (D10) the briefing showed at dispatch — so a concurrent replan can't drift the
/// handles — and the batch is rejected (no write) unless it has exactly one check
/// per injected criterion. The review artifact's `verdict` is derived (`pass` iff
/// all checks pass) for DISPLAY only; the gate decides per-criterion (P2.3).
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

    // Idempotency: this review slot already submitted (its artifact + checks are
    // committed atomically below, so an existing review artifact means the whole
    // submission landed).
    if let Some(existing) = loop_artifact::Entity::find()
        .filter(loop_artifact::Column::ProducedByIterationId.eq(it.id))
        .filter(loop_artifact::Column::Kind.eq(ArtifactKind::Review))
        .one(conn)
        .await?
    {
        return Ok(json!({ "ok": true, "idempotent": true, "id": existing.id }));
    }

    // The injected criterion manifest (D10): handle → criterion id, persisted into
    // `context_manifest` at dispatch. Every submitted handle resolves against THIS,
    // never a recompute, so a replan after dispatch can't change what's accepted.
    let injected = injected_criteria(it)?;
    if injected.is_empty() {
        return Err(invalid(
            "this review has no criteria to check; nothing to submit",
        ));
    }

    // Parse + resolve each check against the manifest. Unknown/duplicate handle,
    // an invalid verdict, or a fail without evidence aborts the whole batch with
    // no write — same all-or-nothing contract as artifact submission.
    let raw = payload
        .get("checks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid("missing checks array"))?;
    if raw.is_empty() {
        return Err(invalid("checks array is empty"));
    }
    let mut resolved: Vec<ResolvedCheck> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for c in raw {
        let handle = c
            .get("criterion")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("each check needs a non-empty `criterion` handle"))?;
        let criterion_id = injected.get(handle).copied().ok_or_else(|| {
            invalid(format!(
                "check references unknown criterion handle '{handle}' (not in this review's checklist)"
            ))
        })?;
        if seen.insert(handle.to_string(), ()).is_some() {
            return Err(invalid(format!("duplicate check for criterion '{handle}'")));
        }
        let verdict_str = c
            .get("verdict")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid(format!("check '{handle}' is missing a verdict")))?;
        let verdict: CheckVerdict = serde_json::from_value(json!(verdict_str))
            .map_err(|_| invalid(format!("check '{handle}' has an invalid verdict '{verdict_str}'")))?;
        let evidence = c
            .get("evidence")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if verdict == CheckVerdict::Fail && evidence.is_empty() {
            return Err(invalid(format!(
                "check '{handle}' is a fail but cites no evidence; name the specific defect"
            )));
        }
        resolved.push(ResolvedCheck {
            criterion_id,
            verdict,
            evidence: truncate(&evidence),
        });
    }

    // Require exactly one check per injected criterion (the injected set = the
    // manifest's keys); a missing handle aborts the batch.
    let missing: Vec<&str> = injected
        .keys()
        .filter(|h| !seen.contains_key(*h))
        .map(|s| s.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(invalid(format!(
            "missing checks for: {}. Submit exactly one check per listed criterion handle.",
            missing.join(", ")
        )));
    }

    // Derived display verdict (DISPLAY ONLY — the gate decides per-criterion).
    let all_pass = resolved.iter().all(|c| c.verdict == CheckVerdict::Pass);
    let display_verdict = if all_pass {
        ReviewVerdict::Pass
    } else {
        ReviewVerdict::Fail
    };
    let findings = truncate(payload.get("findings").and_then(|v| v.as_str()).unwrap_or(""));

    let title = format!("Review (slot {})", it.slot_no.unwrap_or(0));
    // Atomic: the review artifact, its derived verdict, the findings revision, the
    // `reviews` edge, and every per-criterion check land all-or-nothing, so the
    // idempotency guard above (a review by this iteration exists → skip) is correct
    // under crash replay.
    let txn = conn.begin().await?;
    let art = loop_service::artifact::create_artifact(
        &txn,
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
    active.verdict = Set(Some(display_verdict));
    active.update(&txn).await?;

    loop_service::artifact::add_revision(&txn, art.id, &findings, ActorKind::Agent, Some(it.id))
        .await?;
    loop_service::link::create_link(&txn, it.space_id, art.id, target, LinkKind::Reviews, None)
        .await?;
    // One criterion_check per check; the scope is the reviewed target, the
    // iteration is this reviewer slot's stable per-attempt identity (the gate's
    // digest key). Idempotent on `(criterion, iteration, scope)`.
    for c in &resolved {
        loop_service::criterion_check::create_check(
            &txn,
            it.space_id,
            c.criterion_id,
            it.id,
            target,
            c.verdict,
            &c.evidence,
        )
        .await?;
    }
    txn.commit().await?;

    Ok(json!({
        "ok": true,
        "id": art.id,
        "verdict": review_verdict_str(display_verdict),
        "checks": resolved.len(),
    }))
}

/// Parse the iteration's persisted criterion manifest (D10) into a `handle →
/// criterion id` map. `context_manifest` is the briefing manifest stashed at
/// dispatch; review iterations carry a `"criteria"` object. Absent/malformed ⇒
/// empty map (the caller rejects an empty injected set).
fn injected_criteria(it: &loop_iteration::Model) -> Result<HashMap<String, i32>, LoopError> {
    let Some(raw) = it.context_manifest.as_deref() else {
        return Ok(HashMap::new());
    };
    let manifest: Value =
        serde_json::from_str(raw).map_err(|_| invalid("review manifest is unreadable"))?;
    let Some(obj) = manifest.get("criteria").and_then(|v| v.as_object()) else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for (handle, v) in obj {
        if let Some(id) = v.as_i64() {
            out.insert(handle.clone(), id as i32);
        }
    }
    Ok(out)
}

/// Lowercase wire token for a derived review verdict.
fn review_verdict_str(v: ReviewVerdict) -> &'static str {
    match v {
        ReviewVerdict::Pass => "pass",
        ReviewVerdict::Fail => "fail",
    }
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
        None,
        &truncate(content),
        TrustTier::Proposed,
        loop_service::memory::MemoryProvenance {
            source_issue_id: Some(it.issue_id),
            source_artifact_id: None,
            produced_by_iteration_id: Some(it.id),
        },
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
    async fn design_fans_into_all_requirements_with_bound_revisions() {
        let (db, space, issue, root) = seed().await;
        // Refine: two requirements, each with an acceptance criterion.
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok-r").await;
        ingest(
            &db.conn,
            "tok-r",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [
                    {"title": "Req A", "content": "shall A", "criteria": ["A1"]},
                    {"title": "Req B", "content": "shall B", "criteria": ["B1"]}
                ]
            }),
        )
        .await
        .unwrap();
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        let reqs: std::collections::HashSet<i32> = dag
            .artifacts
            .iter()
            .filter(|a| matches!(a.kind, ArtifactKind::Requirement))
            .map(|a| a.id)
            .collect();
        assert_eq!(reqs.len(), 2);

        // Design: one design fans into BOTH requirements, each edge bound to a rev.
        let _ = running_iter(&db.conn, space, issue, Stage::Design, Some(root), "tok-d").await;
        ingest(
            &db.conn,
            "tok-d",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [
                    {"title": "Design", "content": "the design",
                     "criteria": [{"text": "stays O(1)", "kind": "invariant"}]}
                ]
            }),
        )
        .await
        .unwrap();
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        let design_id = dag
            .artifacts
            .iter()
            .find(|a| matches!(a.kind, ArtifactKind::Design))
            .unwrap()
            .id;
        let edges: Vec<_> = dag
            .links
            .iter()
            .filter(|l| matches!(l.kind, LinkKind::DerivesFrom) && l.from_artifact_id == design_id)
            .collect();
        assert_eq!(edges.len(), 2, "design derives from both requirements");
        assert!(
            edges.iter().all(|e| e.source_revision_id.is_some()),
            "each lineage edge binds a requirement revision"
        );
        let targets: std::collections::HashSet<i32> =
            edges.iter().map(|e| e.to_artifact_id).collect();
        assert_eq!(targets, reqs, "edges point at exactly the requirements");
    }

    #[tokio::test]
    async fn typed_criteria_allow_set_enforced() {
        let (db, space, issue, root) = seed().await;
        // A design may not carry an acceptance criterion → batch aborts, no write.
        let _ = running_iter(&db.conn, space, issue, Stage::Design, Some(root), "tok-d").await;
        let err = ingest(
            &db.conn,
            "tok-d",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [{"title": "D", "content": "x",
                    "criteria": [{"text": "do x", "kind": "acceptance"}]}]
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        assert!(
            !dag.artifacts.iter().any(|a| matches!(a.kind, ArtifactKind::Design)),
            "rejected batch wrote nothing"
        );

        // A requirement may not carry an obligation.
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok-r").await;
        let err = ingest(
            &db.conn,
            "tok-r",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [{"title": "R", "content": "x",
                    "criteria": [{"text": "no panics", "kind": "obligation"}]}]
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn plan_covers_creates_criterion_coverage() {
        let (db, space, issue, root) = seed().await;
        // Refine: two requirements, each with one acceptance criterion.
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok-r").await;
        ingest(
            &db.conn,
            "tok-r",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [
                    {"title": "Req A", "content": "shall A", "criteria": ["A1"]},
                    {"title": "Req B", "content": "shall B", "criteria": ["B1"]}
                ]
            }),
        )
        .await
        .unwrap();

        // Resolve each requirement's acceptance criterion id (ordered by sort,id).
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        let mut reqs: Vec<_> = dag
            .artifacts
            .iter()
            .filter(|a| matches!(a.kind, ArtifactKind::Requirement))
            .collect();
        reqs.sort_by_key(|a| (a.sort, a.id));
        let ac1 = loop_service::artifact::get_artifact_detail(&db.conn, reqs[0].id)
            .await
            .unwrap()
            .unwrap()
            .criteria[0]
            .id;
        let ac2 = loop_service::artifact::get_artifact_detail(&db.conn, reqs[1].id)
            .await
            .unwrap()
            .unwrap()
            .criteria[0]
            .id;

        // Plan: two tasks, each covering one requirement's acceptance criterion.
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-p").await;
        ingest(
            &db.conn,
            "tok-p",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [
                    {"title": "T1", "content": "do A", "covers": ["R1.AC1"]},
                    {"title": "T2", "content": "do B", "covers": ["R2.AC1"]}
                ]
            }),
        )
        .await
        .unwrap();

        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        assert_eq!(dag.coverage.len(), 2);
        let covered: std::collections::HashSet<i32> =
            dag.coverage.iter().map(|c| c.criterion_id).collect();
        assert_eq!(covered, [ac1, ac2].into_iter().collect());
    }

    #[tokio::test]
    async fn plan_covers_unknown_ordinal_aborts_batch() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok-r").await;
        ingest(
            &db.conn,
            "tok-r",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [{"title": "Req A", "content": "shall A", "criteria": ["A1"]}]
            }),
        )
        .await
        .unwrap();

        // Plan references a non-existent ordinal → whole batch rejected, no rows.
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-p").await;
        let err = ingest(
            &db.conn,
            "tok-p",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [
                    {"title": "T1", "content": "do A", "covers": ["R1.AC1"]},
                    {"title": "T2", "content": "do B", "covers": ["R9.AC1"]}
                ]
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        assert_eq!(
            dag.artifacts
                .iter()
                .filter(|a| matches!(a.kind, ArtifactKind::Task))
                .count(),
            0,
            "no tasks written"
        );
        assert_eq!(dag.coverage.len(), 0, "no coverage written");
    }

    #[tokio::test]
    async fn plan_incomplete_coverage_rejected_then_resubmit_succeeds() {
        let (db, space, issue, root) = seed().await;
        // Two requirements, each one acceptance criterion → ordinals R1.AC1, R2.AC1.
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok-r").await;
        ingest(
            &db.conn,
            "tok-r",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [
                    {"title": "Req A", "content": "shall A", "criteria": ["A1"]},
                    {"title": "Req B", "content": "shall B", "criteria": ["B1"]}
                ]
            }),
        )
        .await
        .unwrap();

        // A plan covering only R1.AC1 leaves R2.AC1 uncovered → rejected in-turn,
        // nothing written (the planner's iteration stays running so it can resubmit).
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-p").await;
        let err = ingest(
            &db.conn,
            "tok-p",
            "loop_submit_artifacts",
            &json!({"artifacts": [{"title": "T1", "content": "do A", "covers": ["R1.AC1"]}]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        assert_eq!(
            dag.artifacts
                .iter()
                .filter(|a| matches!(a.kind, ArtifactKind::Task))
                .count(),
            0,
            "incomplete plan wrote no tasks"
        );
        assert_eq!(dag.coverage.len(), 0, "no coverage written");

        // Same turn (same token): resubmit covering BOTH ordinals → accepted.
        let out = ingest(
            &db.conn,
            "tok-p",
            "loop_submit_artifacts",
            &json!({
                "artifacts": [
                    {"title": "T1", "content": "do A", "covers": ["R1.AC1"]},
                    {"title": "T2", "content": "do B", "covers": ["R2.AC1"]}
                ]
            }),
        )
        .await
        .unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 2);
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        assert_eq!(dag.coverage.len(), 2, "complete resubmit records full coverage");
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

    /// Mint a task with one acceptance criterion + a running review iteration whose
    /// persisted manifest injects `{ "T1": <criterion id> }` (what dispatch would
    /// stash). Created directly (not via a Plan iteration) so it can be called
    /// repeatedly for one issue without colliding on the Plan node lease. Returns
    /// `(task_id, criterion_id)`.
    async fn seed_task_under_review(
        db: &crate::db::AppDatabase,
        space: i32,
        issue: i32,
        _root: i32,
        token: &str,
    ) -> (i32, i32) {
        let task = loop_service::artifact::create_artifact(
            &db.conn,
            space,
            issue,
            ArtifactKind::Task,
            "Task 1",
            ArtifactStatus::InProgress,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        let task_id = task.id;
        let t1 = loop_service::artifact::add_criterion(
            &db.conn,
            task_id,
            CriterionKind::Acceptance,
            "does the thing",
        )
        .await
        .unwrap()
        .id;

        let iter_id = running_iter(&db.conn, space, issue, Stage::Review, Some(task_id), token).await;
        // Stash the injected criterion manifest (D10) — what assemble_briefing emits.
        loop_iteration::Entity::update_many()
            .col_expr(
                loop_iteration::Column::ContextManifest,
                sea_orm::sea_query::Expr::value(json!({ "criteria": { "T1": t1 } }).to_string()),
            )
            .filter(loop_iteration::Column::Id.eq(iter_id))
            .exec(&db.conn)
            .await
            .unwrap();
        (task_id, t1)
    }

    #[tokio::test]
    async fn review_records_checks_verdict_and_edge() {
        let (db, space, issue, root) = seed().await;
        let (task_id, t1) =
            seed_task_under_review(&db, space, issue, root, "tok-review").await;

        let out = ingest(
            &db.conn,
            "tok-review",
            "loop_submit_review",
            &json!({"checks":[{"criterion":"T1","verdict":"pass","evidence":"it works"}],
                    "findings":"looks good"}),
        )
        .await
        .unwrap();
        let review_id = out["id"].as_i64().unwrap() as i32;
        assert_eq!(out["checks"], json!(1));

        // Display verdict derived = pass; the reviews edge points at the task.
        let detail = loop_service::artifact::get_artifact_detail(&db.conn, review_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.row.verdict, Some(ReviewVerdict::Pass));
        assert!(detail
            .links
            .iter()
            .any(|l| matches!(l.kind, LinkKind::Reviews) && l.to_artifact_id == task_id));
        // A criterion_check row landed for T1, scoped to the task.
        let checks = loop_service::criterion_check::list_for_issue(&db.conn, issue)
            .await
            .unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].criterion_id, t1);
        assert_eq!(checks[0].scope_artifact_id, task_id);

        // Replay → idempotent, no second review / check.
        let again = ingest(
            &db.conn,
            "tok-review",
            "loop_submit_review",
            &json!({"checks":[{"criterion":"T1","verdict":"pass","evidence":"it works"}]}),
        )
        .await
        .unwrap();
        assert_eq!(again["idempotent"], json!(true));
        assert_eq!(
            loop_service::criterion_check::list_for_issue(&db.conn, issue)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn review_fail_check_derives_fail_verdict() {
        let (db, space, issue, root) = seed().await;
        let (_task, _t1) = seed_task_under_review(&db, space, issue, root, "tok-rev").await;
        let out = ingest(
            &db.conn,
            "tok-rev",
            "loop_submit_review",
            &json!({"checks":[{"criterion":"T1","verdict":"fail","evidence":"crashes on empty input"}]}),
        )
        .await
        .unwrap();
        let detail = loop_service::artifact::get_artifact_detail(&db.conn, out["id"].as_i64().unwrap() as i32)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.row.verdict, Some(ReviewVerdict::Fail), "any failing check → fail verdict");
    }

    #[tokio::test]
    async fn review_rejects_unknown_missing_duplicate_and_evidenceless_fail() {
        let (db, space, issue, root) = seed().await;

        // Unknown handle → rejected, no write.
        let (_task, _t1) = seed_task_under_review(&db, space, issue, root, "tok-a").await;
        let err = ingest(
            &db.conn,
            "tok-a",
            "loop_submit_review",
            &json!({"checks":[{"criterion":"R9.AC9","verdict":"pass","evidence":"x"}]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
        assert!(
            loop_service::criterion_check::list_for_issue(&db.conn, issue).await.unwrap().is_empty(),
            "rejected batch wrote no checks"
        );

        // Missing a required handle (empty checks for a non-empty injected set).
        let (_t2, _) = seed_task_under_review(&db, space, issue, root, "tok-b").await;
        let err = ingest(&db.conn, "tok-b", "loop_submit_review", &json!({"checks":[]}))
            .await
            .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));

        // Duplicate check for the same handle.
        let (_t3, _) = seed_task_under_review(&db, space, issue, root, "tok-c").await;
        let err = ingest(
            &db.conn,
            "tok-c",
            "loop_submit_review",
            &json!({"checks":[
                {"criterion":"T1","verdict":"pass","evidence":"a"},
                {"criterion":"T1","verdict":"pass","evidence":"b"}
            ]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));

        // A fail with no evidence is rejected.
        let (_t4, _) = seed_task_under_review(&db, space, issue, root, "tok-d").await;
        let err = ingest(
            &db.conn,
            "tok-d",
            "loop_submit_review",
            &json!({"checks":[{"criterion":"T1","verdict":"fail","evidence":""}]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn review_resolves_against_persisted_manifest_not_current_state() {
        // D10: ingest resolves handles against the manifest stashed at dispatch,
        // so a requirement set that changes AFTER dispatch can't drift the handles.
        let (db, space, issue, root) = seed().await;
        let (task_id, t1) = seed_task_under_review(&db, space, issue, root, "tok-drift").await;

        // Simulate a concurrent refine adding a new requirement+criterion AFTER the
        // review was dispatched (the live ordinals would now differ from T1).
        let _ = running_iter(&db.conn, space, issue, Stage::Refine, Some(root), "tok-late").await;
        ingest(
            &db.conn,
            "tok-late",
            "loop_submit_artifacts",
            &json!({"artifacts":[{"title":"R late","content":"x","criteria":["new ac"]}]}),
        )
        .await
        .unwrap();

        // The reviewer still submits against the DISPATCHED handle T1 → resolves to
        // the same criterion id the manifest froze.
        ingest(
            &db.conn,
            "tok-drift",
            "loop_submit_review",
            &json!({"checks":[{"criterion":"T1","verdict":"pass","evidence":"ok"}]}),
        )
        .await
        .unwrap();
        let checks = loop_service::criterion_check::list_for_issue(&db.conn, issue)
            .await
            .unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].criterion_id, t1, "resolved against the frozen manifest");
        assert_eq!(checks[0].scope_artifact_id, task_id);
    }

    #[tokio::test]
    async fn submit_tasks_with_deps_creates_depends_on_links() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-plan").await;
        let out = ingest(
            &db.conn,
            "tok-plan",
            "loop_submit_artifacts",
            &json!({"artifacts":[
                {"title":"T0","content":"first"},
                {"title":"T1","content":"second","depends_on":[0]}
            ]}),
        )
        .await
        .unwrap();
        let ids = out["ids"].as_array().unwrap();
        assert_eq!(ids.len(), 2);
        let t0 = ids[0].as_i64().unwrap() as i32;
        let t1 = ids[1].as_i64().unwrap() as i32;

        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        // Edge contract: DependsOn from = successor (T1), to = predecessor (T0).
        assert!(dag.links.iter().any(|l| matches!(l.kind, LinkKind::DependsOn)
            && l.from_artifact_id == t1
            && l.to_artifact_id == t0));
        // The root task (T0) has no DependsOn edge of its own.
        assert!(!dag
            .links
            .iter()
            .any(|l| matches!(l.kind, LinkKind::DependsOn) && l.from_artifact_id == t0));
    }

    #[tokio::test]
    async fn submit_tasks_rejects_cycle() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-plan").await;
        // Item 0 references index 1 (forward). Refs may only point backward, so
        // cycles are impossible by construction — this is rejected.
        let err = ingest(
            &db.conn,
            "tok-plan",
            "loop_submit_artifacts",
            &json!({"artifacts":[
                {"title":"A","content":"x","depends_on":[1]},
                {"title":"B","content":"y"}
            ]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn submit_tasks_rejects_multi_predecessor() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-plan").await;
        let err = ingest(
            &db.conn,
            "tok-plan",
            "loop_submit_artifacts",
            &json!({"artifacts":[
                {"title":"A","content":"x"},
                {"title":"B","content":"y"},
                {"title":"C","content":"z","depends_on":[0,1]}
            ]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
        // No partial write: up-front validation aborted before any task row.
        let dag = loop_service::artifact::list_dag(&db.conn, issue).await.unwrap();
        assert_eq!(
            dag.artifacts
                .iter()
                .filter(|a| matches!(a.kind, ArtifactKind::Task))
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn submit_tasks_rejects_out_of_range_dep() {
        let (db, space, issue, root) = seed().await;
        let _ = running_iter(&db.conn, space, issue, Stage::Plan, Some(root), "tok-plan").await;
        // Batch-index refs can't name another issue's task (cross-issue deps are
        // structurally impossible); the equivalent boundary guard rejects an
        // index past the batch.
        let err = ingest(
            &db.conn,
            "tok-plan",
            "loop_submit_artifacts",
            &json!({"artifacts":[{"title":"A","content":"x","depends_on":[5]}]}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LoopError::InvalidInput(_)));
    }
}
