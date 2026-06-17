//! Briefing assembler — builds the deterministic prompt the engine hands a loop
//! iteration agent, plus an audit manifest of exactly what went into it.
//!
//! Fixed §4.8 ordering, every section optional-but-positioned:
//!   ① space constitution (human-authored rules, always first)
//!   ② stage memory matrix (the memory kinds relevant to this stage)
//!   ③ issue full text (the human-written objective)
//!   ④ lineage — the target node verbatim ("direct parent" of what the agent
//!      produces) plus farther ancestors as title + first-paragraph summaries,
//!      cycle-protected so a malformed DAG can't loop forever
//!   ⑤ acceptance criteria — the target's and its parent's criteria, verbatim
//!   ⑥ stage instruction — exhaustive over all seven stages
//!   ⑦ tool contract — which `loop_submit_*` tool the stage calls
//!
//! Section ⑧ (implement: worktree path + validation commands; review: checkpoint
//! diff + validation output) is appended by the dispatcher when those stages run
//! (M2.2) — the read stages this milestone drives carry sections ①–⑦.
//!
//! The returned [`BriefingOutput::manifest`] mirrors the sections that were
//! actually emitted (`{ v, template, components }`) so a run is auditable: you
//! can see which context the agent was and wasn't given.

use std::collections::{HashMap, HashSet};

use sea_orm::DatabaseConnection;
use serde_json::{json, Value};

use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_criterion::CriterionKind;
use crate::db::entities::loop_iteration::Stage;
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::loop_memory::{self, MemoryKind};
use crate::db::entities::loop_issue;
use crate::db::service::loop_service;
use crate::loop_engine::LoopError;
use crate::models::loops::{LoopArtifactDetail, LoopArtifactRow, LoopDagView};

/// Assembled briefing text plus the manifest auditing which components it carried.
#[derive(Debug, Clone)]
pub struct BriefingOutput {
    pub text: String,
    pub manifest: Value,
}

/// Stable lowercase token for a stage — used in the manifest template, section
/// headers, and memory-matrix labeling.
pub fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Triage => "triage",
        Stage::Refine => "refine",
        Stage::Design => "design",
        Stage::Plan => "plan",
        Stage::Implement => "implement",
        Stage::Review => "review",
        Stage::Finalize => "finalize",
    }
}

fn memory_kind_label(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Constitution => "constitution",
        MemoryKind::Constraint => "constraint",
        MemoryKind::Decision => "decision",
        MemoryKind::Preference => "preference",
        MemoryKind::Pitfall => "pitfall",
    }
}

/// The agent-facing working instruction for a stage. Exhaustive over all seven
/// stages so a newly added stage can never silently fall through to a generic
/// prompt.
fn stage_instruction(stage: Stage) -> &'static str {
    match stage {
        Stage::Triage => {
            "Triage this issue. Decide how it should flow: `full` (requirements → \
             design → tasks) for non-trivial or design-bearing work, `skip_design` \
             (requirements → tasks) when scope is clear and no design is needed, or \
             `direct` (a single task) for a small, obvious change. Optionally adjust \
             the issue priority based on what you find."
        }
        Stage::Refine => {
            "Turn this issue into a set of concrete, independently-verifiable \
             requirements. Each requirement is one capability or behavior the \
             solution must have. Attach acceptance criteria to each so later stages \
             can prove they are met."
        }
        Stage::Design => {
            "Produce ONE design that satisfies ALL the requirements listed below. \
             Describe the approach, the components and their responsibilities, the \
             data flow, and the trade-offs you weighed. Stay within the issue's \
             scope; do not invent new requirements. Capture any cross-cutting \
             property the implementation must uphold — a constraint, an invariant, \
             or an obligation — as a typed design criterion so later stages gate \
             on it (these are NOT acceptance criteria; those live on requirements)."
        }
        Stage::Plan => {
            "Break the work into a set of small, self-contained implementation \
             tasks. Each task must be doable and verifiable on its own and carry \
             enough detail (files, approach, acceptance criteria) for an implementer \
             with no other context to execute it. Tasks that touch disjoint files \
             can run in parallel, so prefer non-overlapping file domains; when two \
             tasks must be ordered (one builds on another's output, or they would \
             edit the same files), make the later one declare the earlier as its \
             dependency. A task may declare at most one predecessor."
        }
        Stage::Implement => {
            "Implement the task in the provided worktree. Make the change, keep it \
             scoped to this task, and ensure the acceptance criteria are met. The \
             engine commits your work — you do not need to commit."
        }
        Stage::Review => {
            "Review the implementation against the acceptance-criteria checklist \
             below. Go through the handles ONE BY ONE: for EACH handle, decide \
             pass or fail and cite the specific evidence (the code, behavior, or \
             test that proves it) — a fail MUST name the concrete defect. The \
             design obligations are listed for context only; do not score them \
             here (the assembled result is gated on them at integration). Also \
             surface any defects, omissions, or regressions in your overall \
             findings. The engine derives the gate decision from your per-criterion \
             checks, so submit exactly one check per listed handle."
        }
        Stage::Finalize => {
            "Summarize the completed work for this issue: what was built, how it \
             satisfies the requirements, and anything a reviewer or maintainer \
             should know before it merges."
        }
    }
}

/// The submission contract for a stage — which MCP tool the agent must call and
/// what it produces. Exhaustive over all seven stages.
fn tool_contract(stage: Stage) -> &'static str {
    match stage {
        Stage::Triage => {
            "Call `loop_submit_route` exactly once with your chosen route \
             (full / skip_design / direct)."
        }
        Stage::Refine => {
            "Call `loop_submit_artifacts` exactly once with the full set of \
             requirements (the kind is inferred as `requirement`). Put acceptance \
             criteria in each artifact's `criteria`."
        }
        Stage::Design => {
            "Call `loop_submit_artifacts` exactly once with your single design (the \
             kind is inferred as `design`). A design carries NO acceptance criteria; \
             put any cross-cutting properties in `criteria` as typed objects \
             `{\"text\": \"...\", \"kind\": \"constraint\"|\"invariant\"|\"obligation\"}`."
        }
        Stage::Plan => {
            "Call `loop_submit_artifacts` exactly once with the task breakdown (the \
             kind is inferred as `task`). List tasks in dependency order and put \
             each task's own acceptance criteria in its `criteria`. EVERY task MUST \
             include a `covers` array naming the requirement acceptance ordinals it \
             delivers (from the Requirements / Coverage contract sections), e.g. \
             `\"covers\": [\"R1.AC1\", \"R2.AC1\"]`. Across all tasks, every \
             acceptance ordinal listed in the Coverage contract MUST be covered by \
             at least one task — a submission that leaves any uncovered is REJECTED \
             and you must resubmit the complete task list. To make a task depend on \
             an earlier one, set its `depends_on` to a one-element array holding \
             that earlier task's 0-based index in this same submission (e.g. a task \
             waiting on the first → `\"depends_on\": [0]`). A reference may only \
             point to an earlier task, and a task may declare at most one."
        }
        Stage::Implement => {
            "Do not call a submit tool — the engine detects and commits your \
             worktree changes. If you are blocked, call `loop_report_blocked`."
        }
        Stage::Review => {
            "Call `loop_submit_review` exactly once. Put one entry in `checks` for \
             EACH acceptance-criterion handle in the checklist above: \
             `{\"criterion\": \"R1.AC1\", \"verdict\": \"pass\"|\"fail\", \
             \"evidence\": \"...\"}`. Submit exactly one check per listed handle — \
             no more, no fewer — and a `fail` check MUST cite specific evidence. Do \
             NOT submit checks for the design obligations (they are context only). \
             Add overall `findings` so a failed criterion guides the next attempt."
        }
        Stage::Finalize => {
            "Call `loop_submit_artifacts` exactly once with the result summary."
        }
    }
}

/// Parallel-mode finalize is NOT a result submission — the engine integrates the
/// per-task branches and synthesizes the result itself. A finalize agent is only
/// dispatched to resolve a fan-in MERGE CONFLICT in the integrate worktree.
const PARALLEL_FINALIZE_INSTRUCTION: &str =
    "The engine is integrating this issue's parallel task branches and hit a merge \
     conflict in THIS worktree. Resolve every conflict so the combined work is \
     correct and consistent, preserving each task's intent. Do not start new \
     feature work — only finish the in-progress merge.";

const PARALLEL_FINALIZE_TOOL_CONTRACT: &str =
    "Do NOT call any submit tool. Resolve the conflicted files in the worktree, \
     stage them (`git add`), and COMPLETE the in-progress merge with a plain \
     `git commit` (keep the default merge message — preserve both parents). The \
     engine detects the completed merge and continues the fan-in. If you cannot \
     resolve it, call `loop_report_blocked`.";

/// First non-empty paragraph (up to the first blank line) of `s`, trimmed. Used
/// to summarize farther ancestors without dumping their entire body.
fn first_paragraph(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        if line.trim().is_empty() {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out.trim().to_string()
}

/// Keep the last `max_chars` characters of `s` (failures surface at the end of a
/// transcript), prefixing an ellipsis when truncated. Char-boundary safe.
fn tail(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let kept: String = chars[chars.len() - max_chars..].iter().collect();
    format!("…{kept}")
}

fn render_memories(mems: &[loop_memory::Model]) -> String {
    let mut s = String::new();
    for m in mems {
        s.push_str(&format!(
            "- ({}) {}: {}\n",
            memory_kind_label(m.kind),
            m.title,
            m.content
        ));
    }
    s.trim_end().to_string()
}

/// Walk the lineage of `target` toward the issue root by following the inbound
/// edge direction (`from` = derived, `to` = source) for `derives_from` /
/// `skips_to` links. Returns the chain starting AT `target`, parent next, and so
/// on. Cycle-protected: a node already seen ends the walk, so a malformed DAG
/// can't loop forever.
fn build_lineage(links: &[crate::models::loops::LoopLinkRow], target: i32) -> Vec<i32> {
    let mut parent: HashMap<i32, i32> = HashMap::new();
    for link in links {
        if matches!(link.kind, LinkKind::DerivesFrom | LinkKind::SkipsTo) {
            // First parent wins (a well-formed DAG has at most one source edge
            // per derived node); ignore extras defensively.
            parent.entry(link.from_artifact_id).or_insert(link.to_artifact_id);
        }
    }
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = Some(target);
    while let Some(node) = cur {
        if !seen.insert(node) {
            break;
        }
        chain.push(node);
        cur = parent.get(&node).copied();
    }
    chain
}

/// Lowercase token for a criterion kind (obligation rendering + manifest).
fn criterion_kind_label(kind: CriterionKind) -> &'static str {
    match kind {
        CriterionKind::Acceptance => "acceptance",
        CriterionKind::Constraint => "constraint",
        CriterionKind::Invariant => "invariant",
        CriterionKind::Obligation => "obligation",
    }
}

/// The issue's done requirements with full details (criteria), ordered by
/// `(sort, id)` — the same order ingest and the driver use for the `R{i}.AC{j}`
/// coverage ordinals, so an ordinal printed in a briefing matches what was stored
/// and gated on.
async fn done_requirement_details(
    conn: &DatabaseConnection,
    dag: &LoopDagView,
) -> Result<Vec<LoopArtifactDetail>, LoopError> {
    let mut reqs: Vec<&LoopArtifactRow> = dag
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Requirement && a.status == ArtifactStatus::Done)
        .collect();
    reqs.sort_by_key(|a| (a.sort, a.id));
    let mut out = Vec::with_capacity(reqs.len());
    for r in reqs {
        if let Some(d) = loop_service::artifact::get_artifact_detail(conn, r.id).await? {
            out.push(d);
        }
    }
    Ok(out)
}

/// `criterion_id → "R{i}.AC{j}"` for every acceptance criterion of the issue,
/// built from the SINGLE shared ordinal source so the ordinals shown in a
/// briefing are byte-identical to what ingest stored on `covers` and what the
/// driver's coverage gate reasons about (spec invariant). Requirements ordered
/// by (sort,id) here match `done_requirement_details`, so the `## R{i}` headers
/// stay aligned with these criterion ordinals.
async fn ordinal_map(
    conn: &DatabaseConnection,
    issue_id: i32,
) -> Result<HashMap<i32, String>, LoopError> {
    let ordered = loop_service::coverage::acceptance_ordinals_for_issue(conn, issue_id).await?;
    let mut map = HashMap::new();
    for (ri, (_req, crits)) in ordered.iter().enumerate() {
        for (ci, cid) in crits.iter().enumerate() {
            map.insert(*cid, format!("R{}.AC{}", ri + 1, ci + 1));
        }
    }
    Ok(map)
}

/// Render all requirements + their acceptance criteria for the design/plan
/// briefing. Plan annotates each criterion with its `R{i}.AC{j}` ordinal (from
/// the shared `ordinals` map, so the planner's `covers` matches); design omits
/// ordinals (it satisfies them all).
fn render_requirements(
    reqs: &[LoopArtifactDetail],
    ordinals: &HashMap<i32, String>,
    with_ordinals: bool,
) -> String {
    let mut body = String::new();
    for (ri, r) in reqs.iter().enumerate() {
        let rbody = r.revisions.last().map(|x| x.content.trim()).unwrap_or("");
        body.push_str(&format!("## R{}: {}\n{}\n", ri + 1, r.row.title, rbody));
        for c in &r.criteria {
            if c.kind == CriterionKind::Acceptance {
                if with_ordinals {
                    let ord = ordinals.get(&c.id).cloned().unwrap_or_default();
                    body.push_str(&format!("- [{}] {}\n", ord, c.text));
                } else {
                    body.push_str(&format!("- {}\n", c.text));
                }
            }
        }
        body.push('\n');
    }
    body.trim_end().to_string()
}

/// The acceptance closure for a task (implement/review): the acceptance criteria
/// the task covers (by ordinal), plus the design's cross-cutting obligations
/// (constraint/invariant/obligation), plus — when the task declared no coverage —
/// a fallback to every requirement acceptance criterion, so the agent is never
/// blind to what its work must satisfy. Returns `None` only when the issue has no
/// criteria of any kind.
async fn acceptance_closure(
    conn: &DatabaseConnection,
    issue_id: i32,
    task_id: i32,
    dag: &LoopDagView,
) -> Result<Option<String>, LoopError> {
    let reqs = done_requirement_details(conn, dag).await?;
    let ordinals = ordinal_map(conn, issue_id).await?;
    // criterion id -> (ordinal, text) over acceptance criteria, ordinal from the
    // shared source.
    let mut by_id: HashMap<i32, (String, String)> = HashMap::new();
    for r in &reqs {
        for c in &r.criteria {
            if c.kind == CriterionKind::Acceptance {
                if let Some(ord) = ordinals.get(&c.id) {
                    by_id.insert(c.id, (ord.clone(), c.text.clone()));
                }
            }
        }
    }
    let covered: Vec<i32> = dag
        .coverage
        .iter()
        .filter(|cv| cv.task_artifact_id == task_id)
        .map(|cv| cv.criterion_id)
        .collect();

    let mut body = String::new();
    if !covered.is_empty() {
        body.push_str("This task is responsible for these acceptance criteria:\n");
        for cid in &covered {
            if let Some((ord, text)) = by_id.get(cid) {
                body.push_str(&format!("- [{ord}] {text}\n"));
            }
        }
    } else if !by_id.is_empty() {
        body.push_str(
            "This task declared no specific coverage, so it must respect ALL of the \
             issue's acceptance criteria:\n",
        );
        body.push_str(&render_requirements(&reqs, &ordinals, true));
        body.push('\n');
    }

    // Design obligations apply to the whole solution, on every task.
    let mut obligations = String::new();
    for a in dag
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Design && a.status == ArtifactStatus::Done)
    {
        if let Some(d) = loop_service::artifact::get_artifact_detail(conn, a.id).await? {
            for c in &d.criteria {
                obligations.push_str(&format!("- ({}) {}\n", criterion_kind_label(c.kind), c.text));
            }
        }
    }
    if !obligations.is_empty() {
        body.push_str("\nDesign obligations (must hold across the whole solution):\n");
        body.push_str(&obligations);
    }

    let body = body.trim_end().to_string();
    Ok(if body.is_empty() { None } else { Some(body) })
}

/// The per-criterion review checklist for a TASK review (§3.4, D9). Returns the
/// "# Acceptance criteria" section body (the exact handles the reviewer must
/// submit one check each for, plus design obligations shown as awareness-only
/// context) AND the `{ handle: criterion_id }` manifest the gate resolves
/// submitted checks against. The two are built from the SAME ordinal source, so
/// what the reviewer is shown is exactly what ingest accepts.
async fn review_checklist_section(
    conn: &DatabaseConnection,
    issue_id: i32,
    task_id: i32,
) -> Result<(Option<String>, Value), LoopError> {
    let entries =
        loop_service::criterion_ordinals::task_review_ordinals(conn, issue_id, task_id).await?;
    let obligations =
        loop_service::criterion_ordinals::obligation_ordinals(conn, issue_id).await?;

    let mut manifest = serde_json::Map::new();
    for e in &entries {
        manifest.insert(e.handle.clone(), json!(e.criterion_id));
    }

    if entries.is_empty() {
        // No task-verifiable criteria (a degenerate direct task). Emit no
        // checklist; the empty manifest tells the gate there is nothing to check.
        return Ok((None, Value::Object(manifest)));
    }

    let mut body = String::from(
        "Submit one check per handle below (use the EXACT handle in brackets), each \
         with a pass/fail and concrete evidence:\n",
    );
    for e in &entries {
        body.push_str(&format!("- [{}] {}\n", e.handle, e.text));
    }
    if !obligations.is_empty() {
        body.push_str(
            "\nDesign obligations — context only (the assembled result is gated on these at \
             integration, NOT in this task review; do not submit checks for them):\n",
        );
        for o in &obligations {
            body.push_str(&format!("- ({}) {}\n", criterion_kind_label(o.kind), o.text));
        }
    }
    Ok((
        Some(format!("# Acceptance criteria\n{}", body.trim_end())),
        Value::Object(manifest),
    ))
}

/// The per-criterion checklist for an INTEGRATION review (target = the assembled
/// `result`, §3.6, D9): the whole-issue closure — every requirement acceptance
/// (`R{i}.AC{j}`) plus every design obligation (`D{k}`), or the tasks' own
/// acceptance on the `direct` route. ALL of these are checks here (unlike a task
/// review, where obligations are awareness-only). Returns the section body and the
/// `{ handle: criterion_id }` manifest the gate resolves against.
async fn integration_checklist_section(
    conn: &DatabaseConnection,
    issue_id: i32,
) -> Result<(Option<String>, Value), LoopError> {
    let entries =
        loop_service::criterion_ordinals::integration_ordinals(conn, issue_id).await?;
    let mut manifest = serde_json::Map::new();
    for e in &entries {
        manifest.insert(e.handle.clone(), json!(e.criterion_id));
    }
    if entries.is_empty() {
        return Ok((None, Value::Object(manifest)));
    }
    let mut body = String::from(
        "Verify the ASSEMBLED RESULT (the full combined change) against the whole-issue \
         closure. Submit one check per handle below (use the EXACT handle in brackets), each \
         with a pass/fail and concrete evidence drawn from the integrated result — a `fail` \
         names the specific gap or cross-task conflict:\n",
    );
    for e in &entries {
        body.push_str(&format!("- [{}] {}\n", e.handle, e.text));
    }
    Ok((
        Some(format!("# Integration criteria\n{}", body.trim_end())),
        Value::Object(manifest),
    ))
}

/// Assemble the briefing for one iteration. `issue` is the loaded issue row;
/// `target_artifact_id` is the node the iteration derives its output from (the
/// issue root for triage/refine, a requirement for design, etc.) — `None` only
/// for a target-less stage.
pub async fn assemble_briefing(
    conn: &DatabaseConnection,
    issue: &loop_issue::Model,
    stage: Stage,
    target_artifact_id: Option<i32>,
) -> Result<BriefingOutput, LoopError> {
    let mut sections: Vec<String> = Vec::new();
    let mut components: Vec<Value> = Vec::new();
    // For a review iteration: the `{ handle: criterion_id }` map the gate resolves
    // submitted checks against, persisted into the iteration's `context_manifest`
    // at dispatch (D10). `None` for non-review stages.
    let mut criteria_manifest: Option<Value> = None;

    // ① Space constitution — human-authored, always first.
    let constitution = loop_service::memory::list_constitution(conn, issue.space_id).await?;
    if !constitution.is_empty() {
        sections.push(format!(
            "# Space constitution\n{}",
            render_memories(&constitution)
        ));
        components.push(json!({ "section": "constitution", "count": constitution.len() }));
    }

    // ② Stage memory matrix — the memory kinds relevant to this stage.
    let mems = loop_service::memory::list_active_for_stage(conn, issue.space_id, stage).await?;
    if !mems.is_empty() {
        sections.push(format!(
            "# Relevant memory ({} stage)\n{}",
            stage_label(stage),
            render_memories(&mems)
        ));
        components.push(json!({
            "section": "memory_matrix",
            "stage": stage_label(stage),
            "count": mems.len(),
        }));
    }

    // ③ Issue full text — the human-written objective.
    sections.push(format!(
        "# Issue #{}: {}\n\n{}",
        issue.seq_no,
        issue.title,
        issue.description.trim()
    ));
    components.push(json!({ "section": "issue", "issue_seq": issue.seq_no }));

    // ④/⑤ Stage-shaped requirement context:
    //   • design & plan see ALL requirements (design satisfies them all; plan
    //     declares `covers` against their ordinals, and on a replan sees the gap);
    //   • implement & review get their task's acceptance closure (covered criteria
    //     + design obligations + fallback);
    //   • other staged targets get single-target lineage + its criteria verbatim.
    if matches!(stage, Stage::Design | Stage::Plan) {
        let dag = loop_service::artifact::list_dag(conn, issue.id).await?;
        let reqs = done_requirement_details(conn, &dag).await?;
        let ordinals = ordinal_map(conn, issue.id).await?;
        if !reqs.is_empty() {
            sections.push(format!(
                "# Requirements\n{}",
                render_requirements(&reqs, &ordinals, stage == Stage::Plan)
            ));
            components.push(json!({ "section": "requirements", "count": reqs.len() }));
        }
        // Plan only: an explicit flat checklist of EVERY acceptance ordinal the
        // plan must cover. The per-requirement ordinals above are easy to miss when
        // scattered; restating the full target set as one list makes the planner's
        // `covers` complete on the first submission (an incomplete plan is rejected
        // at submit time and must be resubmitted — see the tool contract).
        if stage == Stage::Plan && !reqs.is_empty() {
            let all_ords: Vec<String> = reqs
                .iter()
                .flat_map(|r| {
                    r.criteria
                        .iter()
                        .filter(|c| c.kind == CriterionKind::Acceptance)
                        .filter_map(|c| ordinals.get(&c.id).cloned())
                })
                .collect();
            if !all_ords.is_empty() {
                let list = all_ords
                    .iter()
                    .map(|o| format!("- {o}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!(
                    "# Coverage contract\nEvery task MUST declare a `covers` array. Together your \
                     tasks MUST cover ALL {} acceptance ordinals below — a submission that leaves \
                     any uncovered is rejected and you will be asked to resubmit:\n{list}",
                    all_ords.len()
                ));
                components
                    .push(json!({ "section": "coverage_contract", "count": all_ords.len() }));
            }
        }
        // On a plan replan (a prior plan's tasks were superseded by the coverage
        // gate), call out the criteria still uncovered by ANY task so the new plan
        // closes them.
        if stage == Stage::Plan
            && dag
                .artifacts
                .iter()
                .any(|a| a.kind == ArtifactKind::Task && a.status == ArtifactStatus::Superseded)
        {
            let ord_pairs =
                loop_service::coverage::acceptance_ordinals_for_issue(conn, issue.id).await?;
            let all_tasks: HashSet<i32> = dag
                .artifacts
                .iter()
                .filter(|a| a.kind == ArtifactKind::Task)
                .map(|a| a.id)
                .collect();
            let gap =
                loop_service::coverage::uncovered_ordinals(&ord_pairs, &dag.coverage, &all_tasks);
            if !gap.is_empty() {
                let list = gap
                    .iter()
                    .map(|o| format!("- {o}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!(
                    "# Coverage gap\nA previous plan left these acceptance criteria uncovered by \
                     any task. The new plan MUST cover them:\n{list}"
                ));
                components.push(json!({ "section": "coverage_gap", "count": gap.len() }));
            }
        }
    } else if let Some(target) = target_artifact_id {
        let dag = loop_service::artifact::list_dag(conn, issue.id).await?;
        let chain = build_lineage(&dag.links, target);

        let mut details = Vec::new();
        for id in &chain {
            if let Some(d) = loop_service::artifact::get_artifact_detail(conn, *id).await? {
                details.push(d);
            }
        }

        if let Some((head, ancestors)) = details.split_first() {
            // ④ Lineage: target verbatim, ancestors as title + first-paragraph.
            let head_body = head
                .revisions
                .last()
                .map(|r| r.content.trim())
                .unwrap_or("");
            let mut lineage = format!("## {} (direct parent)\n{}\n", head.row.title, head_body);
            for d in ancestors {
                let summary =
                    first_paragraph(d.revisions.last().map(|r| r.content.as_str()).unwrap_or(""));
                lineage.push_str(&format!("\n## {} (ancestor)\n{}\n", d.row.title, summary));
            }
            sections.push(format!("# Lineage\n{}", lineage.trim_end()));
            components.push(json!({ "section": "lineage", "depth": details.len() }));

            // ⑤ Criteria.
            //   • implement gets the AC closure (coverage + design obligations +
            //     fallback) as guidance;
            //   • review gets the per-criterion CHECKLIST (the exact handles it
            //     must submit one check each for) + the manifest the gate resolves
            //     against — design obligations shown as awareness-only;
            //   • other staged targets get the target's + parent's criteria verbatim.
            if stage == Stage::Implement {
                if let Some(closure) = acceptance_closure(conn, issue.id, target, &dag).await? {
                    sections.push(format!("# Acceptance criteria\n{closure}"));
                    components.push(json!({ "section": "acceptance_criteria", "closure": true }));
                }
            } else if stage == Stage::Review {
                // A review targeting the `result` is the INTEGRATION gate (whole-issue
                // closure); a review targeting a task is the task gate (covered ACs +
                // the task's own acceptance, obligations awareness-only).
                let is_integration = dag
                    .artifacts
                    .iter()
                    .find(|a| a.id == target)
                    .map(|a| a.kind == ArtifactKind::Result)
                    .unwrap_or(false);
                let (label, (section, manifest)) = if is_integration {
                    ("integration_checklist", integration_checklist_section(conn, issue.id).await?)
                } else {
                    ("review_checklist", review_checklist_section(conn, issue.id, target).await?)
                };
                if let Some(s) = section {
                    sections.push(s);
                    let count = manifest.as_object().map(|m| m.len()).unwrap_or(0);
                    components.push(json!({ "section": label, "count": count }));
                }
                criteria_manifest = Some(manifest);
            } else {
                let mut crit = String::new();
                if !head.criteria.is_empty() {
                    crit.push_str(&format!("From {}:\n", head.row.title));
                    for c in &head.criteria {
                        crit.push_str(&format!("- [{}] {}\n", c.label, c.text));
                    }
                }
                if let Some(parent) = ancestors.first() {
                    if !parent.criteria.is_empty() {
                        crit.push_str(&format!("\nFrom {} (parent):\n", parent.row.title));
                        for c in &parent.criteria {
                            crit.push_str(&format!("- [{}] {}\n", c.label, c.text));
                        }
                    }
                }
                if !crit.is_empty() {
                    sections.push(format!("# Acceptance criteria\n{}", crit.trim_end()));
                    components.push(json!({ "section": "acceptance_criteria" }));
                }
            }
        }
    }

    // ⑤a Rework feedback — on an implement retry, surface why the last attempt
    // was rejected (validation failure and/or reviewer findings) so the agent
    // fixes forward instead of repeating it.
    if stage == Stage::Implement {
        if let Some(target) = target_artifact_id {
            if let Some(run) = loop_service::validation::latest_for_task(conn, target).await? {
                if !run.passed {
                    sections.push(format!(
                        "# Previous validation failure\nYour last attempt did not pass the \
                         deterministic validation commands. Fix the problems below, then make \
                         the change again.\n\n```\n{}\n```",
                        tail(run.output.trim(), 4000)
                    ));
                    components.push(json!({ "section": "validation_feedback", "run_id": run.id }));
                }
            }
            let findings = loop_service::artifact::latest_failed_review_findings(conn, target).await?;
            if !findings.is_empty() {
                let mut body = String::from(
                    "# Previous review findings\nReviewers rejected your last attempt. Address \
                     every point below, then make the change again.",
                );
                for (i, f) in findings.iter().enumerate() {
                    body.push_str(&format!("\n\n## Reviewer {}\n{}", i + 1, tail(f, 2000)));
                }
                sections.push(body);
                components.push(json!({ "section": "review_feedback", "count": findings.len() }));
            }
        }
    }

    // ⑤b Review context — point reviewers at the committed work to inspect and at
    // the validation result, so they review against the real changes.
    if stage == Stage::Review {
        if let Some(base) = issue.base_commit.as_deref() {
            sections.push(format!(
                "# What to review\nThe implementation is committed on this branch. Inspect the \
                 changes since base commit `{base}` (e.g. `git diff {base}..HEAD`) and read the \
                 affected files in the worktree."
            ));
            components.push(json!({ "section": "review_context", "base": base }));
        }
        if let Some(target) = target_artifact_id {
            if let Some(run) = loop_service::validation::latest_for_task(conn, target).await? {
                sections.push(format!(
                    "# Validation result\nDeterministic validation {}.\n\n```\n{}\n```",
                    if run.passed { "passed" } else { "did not pass" },
                    tail(run.output.trim(), 2000)
                ));
                components.push(json!({ "section": "validation_result", "passed": run.passed }));
            }
        }
    }

    // ⑤c Design rework feedback — on a design re-dispatched after a human
    // rejection, surface the prior proposal and the reviewer's comment so the new
    // design addresses it rather than repeating it.
    if stage == Stage::Design {
        let dag = loop_service::artifact::list_dag(conn, issue.id).await?;
        let mut rejected = String::new();
        for a in dag.artifacts.iter().filter(|a| {
            a.kind == ArtifactKind::Design && a.status == ArtifactStatus::Superseded
        }) {
            if let Some(d) = loop_service::artifact::get_artifact_detail(conn, a.id).await? {
                let body = d
                    .revisions
                    .iter()
                    .rev()
                    .find(|r| r.actor_kind == ActorKind::Agent)
                    .map(|r| r.content.trim())
                    .unwrap_or("");
                let note = d
                    .revisions
                    .iter()
                    .rev()
                    .find(|r| r.actor_kind == ActorKind::Human)
                    .map(|r| r.content.trim())
                    .unwrap_or("");
                rejected.push_str(&format!(
                    "\n\n## {} (rejected)\n{}",
                    d.row.title,
                    first_paragraph(body)
                ));
                if !note.is_empty() {
                    rejected.push_str(&format!("\n\nReviewer feedback: {note}"));
                }
            }
        }
        if !rejected.is_empty() {
            sections.push(format!(
                "# Previously rejected design\nA prior design was rejected. Address the \
                 feedback below and propose a revised design.{rejected}"
            ));
            components.push(json!({ "section": "design_rework_feedback" }));
        }
    }

    // ⑥ Stage instruction — what to do this turn. A parallel-mode finalize is a
    // fan-in conflict resolution (the engine synthesizes the result), so it gets
    // the conflict-resolution briefing instead of the serial result-submit one.
    let parallel_finalize =
        stage == Stage::Finalize && issue.execution_mode.as_deref() == Some("parallel");
    sections.push(format!(
        "# Your task\n{}",
        if parallel_finalize {
            PARALLEL_FINALIZE_INSTRUCTION
        } else {
            stage_instruction(stage)
        }
    ));
    components.push(json!({ "section": "stage_instruction", "stage": stage_label(stage) }));

    // ⑦ Tool contract — how to submit.
    sections.push(format!(
        "# How to submit\n{}",
        if parallel_finalize {
            PARALLEL_FINALIZE_TOOL_CONTRACT
        } else {
            tool_contract(stage)
        }
    ));
    components.push(json!({ "section": "tool_contract", "stage": stage_label(stage) }));

    let mut manifest = json!({
        "v": 1,
        "template": format!("{}@v1", stage_label(stage)),
        "components": components,
    });
    // A review iteration carries its injected criterion manifest (D10) — the
    // single source ingest resolves submitted check handles against.
    if let Some(criteria) = criteria_manifest {
        manifest["criteria"] = criteria;
    }

    Ok(BriefingOutput {
        text: sections.join("\n\n"),
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::db::entities::loop_criterion::CriterionKind;
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::models::loops::IssueConfig;

    /// Seed space + issue (auto-creates the kind=issue root artifact). Returns
    /// `(db, space_id, issue_model, root_artifact_id)`.
    async fn seed() -> (crate::db::AppDatabase, i32, loop_issue::Model, i32) {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/repo").await;
        let space = loop_service::space::create_space(&db.conn, "S", folder_id)
            .await
            .unwrap();
        let issue = loop_service::issue::create_issue(
            &db.conn,
            space.id,
            "Add login",
            "Users must be able to authenticate.",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        let model = loop_service::issue::get_issue(&db.conn, issue.row.id)
            .await
            .unwrap()
            .unwrap();
        let dag = loop_service::artifact::list_dag(&db.conn, issue.row.id)
            .await
            .unwrap();
        let root = dag
            .artifacts
            .iter()
            .find(|a| matches!(a.kind, ArtifactKind::Issue))
            .expect("root issue artifact")
            .id;
        (db, space.id, model, root)
    }

    /// Create an artifact + one revision + optional criteria + a DerivesFrom edge
    /// to `source`. Returns the new artifact id.
    #[allow(clippy::too_many_arguments)]
    async fn add_node(
        db: &crate::db::AppDatabase,
        space_id: i32,
        issue_id: i32,
        kind: ArtifactKind,
        title: &str,
        content: &str,
        criteria: &[&str],
        source: i32,
    ) -> i32 {
        let art = loop_service::artifact::create_artifact(
            &db.conn,
            space_id,
            issue_id,
            kind,
            title,
            ArtifactStatus::Done,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        loop_service::artifact::add_revision(&db.conn, art.id, content, ActorKind::Agent, None)
            .await
            .unwrap();
        for c in criteria {
            loop_service::artifact::add_criterion(&db.conn, art.id, CriterionKind::Acceptance, c)
                .await
                .unwrap();
        }
        loop_service::link::create_link(
            &db.conn,
            space_id,
            art.id,
            source,
            LinkKind::DerivesFrom,
            None,
        )
        .await
        .unwrap();
        art.id
    }

    #[tokio::test]
    async fn design_briefing_has_all_sections() {
        let (db, space, issue, root) = seed().await;
        // Constitution + a design-relevant decision + an implement-only pitfall.
        loop_service::memory::create_memory(
            &db.conn,
            space,
            MemoryKind::Constitution,
            ActorKind::Human,
            "House rules",
            None,
            "No new dependencies without approval.",
            loop_memory::TrustTier::Human,
            loop_service::memory::MemoryProvenance::default(),
        )
        .await
        .unwrap();
        loop_service::memory::create_memory(
            &db.conn,
            space,
            MemoryKind::Decision,
            ActorKind::Agent,
            "Token store",
            None,
            "Use the existing keyring abstraction.",
            loop_memory::TrustTier::Proposed,
            loop_service::memory::MemoryProvenance::default(),
        )
        .await
        .unwrap();
        loop_service::memory::create_memory(
            &db.conn,
            space,
            MemoryKind::Pitfall,
            ActorKind::Agent,
            "Flaky test",
            None,
            "auth_test is order-dependent.",
            loop_memory::TrustTier::Proposed,
            loop_service::memory::MemoryProvenance::default(),
        )
        .await
        .unwrap();

        // issue root → requirement → (design target is the requirement).
        let req = add_node(
            &db,
            space,
            issue.id,
            ArtifactKind::Requirement,
            "R1: credential check",
            "The system must verify a username/password pair.\n\nDetails follow.",
            &["Rejects an unknown user", "Accepts a valid pair"],
            root,
        )
        .await;

        let out = assemble_briefing(&db.conn, &issue, Stage::Design, Some(req))
            .await
            .unwrap();
        let t = &out.text;

        // ① constitution, ② design memory matrix (decision present, pitfall NOT).
        assert!(t.contains("Space constitution"));
        assert!(t.contains("No new dependencies"));
        assert!(t.contains("Use the existing keyring abstraction."));
        assert!(
            !t.contains("auth_test is order-dependent"),
            "pitfall is not in the design memory matrix"
        );
        // ③ issue full text.
        assert!(t.contains("Add login"));
        assert!(t.contains("Users must be able to authenticate."));
        // ④ requirements: design sees ALL requirements (title + body + criteria),
        // not a single-target lineage.
        assert!(t.contains("# Requirements"));
        assert!(t.contains("R1: credential check"));
        assert!(t.contains("The system must verify a username/password pair."));
        assert!(t.contains("Rejects an unknown user"));
        // ⑥ stage instruction (design-specific) + ⑦ tool contract.
        assert!(t.contains("Produce ONE design"));
        assert!(t.contains("loop_submit_artifacts"));

        // Manifest lists every emitted component + the stage template.
        assert_eq!(out.manifest["template"], "design@v1");
        let sections: Vec<&str> = out.manifest["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["section"].as_str().unwrap())
            .collect();
        for expected in [
            "constitution",
            "memory_matrix",
            "issue",
            "requirements",
            "stage_instruction",
            "tool_contract",
        ] {
            assert!(sections.contains(&expected), "manifest missing {expected}");
        }
    }

    #[tokio::test]
    async fn design_briefing_shows_all_requirements() {
        let (db, space, issue, root) = seed().await;
        add_node(&db, space, issue.id, ArtifactKind::Requirement, "Alpha", "req alpha", &[], root).await;
        add_node(&db, space, issue.id, ArtifactKind::Requirement, "Beta", "req beta", &[], root).await;
        add_node(&db, space, issue.id, ArtifactKind::Requirement, "Gamma", "req gamma", &[], root).await;
        let out = assemble_briefing(&db.conn, &issue, Stage::Design, Some(root))
            .await
            .unwrap();
        let t = &out.text;
        assert!(
            t.contains("Alpha") && t.contains("Beta") && t.contains("Gamma"),
            "design must see every requirement, not just one"
        );
    }

    #[tokio::test]
    async fn plan_briefing_enumerates_criterion_ordinals() {
        let (db, space, issue, root) = seed().await;
        add_node(&db, space, issue.id, ArtifactKind::Requirement, "R1", "first req", &["alpha holds"], root).await;
        add_node(&db, space, issue.id, ArtifactKind::Requirement, "R2", "second req", &["beta holds"], root).await;
        let out = assemble_briefing(&db.conn, &issue, Stage::Plan, Some(root))
            .await
            .unwrap();
        let t = &out.text;
        assert!(t.contains("# Requirements"));
        assert!(t.contains("[R1.AC1] alpha holds"), "plan enumerates ordinals");
        assert!(t.contains("[R2.AC1] beta holds"));
        assert!(t.contains("covers"), "plan tool contract explains covers");
        // The coverage contract restates the full target set as one flat checklist.
        assert!(t.contains("# Coverage contract"), "plan gets the coverage contract");
        assert!(
            t.contains("cover ALL 2 acceptance ordinals"),
            "coverage contract states the full count"
        );
    }

    #[tokio::test]
    async fn implement_briefing_ac_closure_covered_and_fallback() {
        let (db, space, issue, root) = seed().await;
        let r1 = add_node(&db, space, issue.id, ArtifactKind::Requirement, "R1", "first", &["alpha holds"], root).await;
        add_node(&db, space, issue.id, ArtifactKind::Requirement, "R2", "second", &["beta holds"], root).await;
        // A design carrying a cross-cutting obligation (invariant).
        let design = loop_service::artifact::create_artifact(&db.conn, space, issue.id, ArtifactKind::Design, "D", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();
        loop_service::artifact::add_criterion(&db.conn, design.id, CriterionKind::Invariant, "stays O(1)").await.unwrap();
        // Task 1 covers R1.AC1; task 2 covers nothing.
        let t1 = loop_service::artifact::create_artifact(&db.conn, space, issue.id, ArtifactKind::Task, "T1", ArtifactStatus::Pending, ActorKind::Agent, None).await.unwrap();
        let t2 = loop_service::artifact::create_artifact(&db.conn, space, issue.id, ArtifactKind::Task, "T2", ArtifactStatus::Pending, ActorKind::Agent, None).await.unwrap();
        let r1ac = loop_service::artifact::get_artifact_detail(&db.conn, r1).await.unwrap().unwrap().criteria[0].id;
        loop_service::coverage::create_coverage(&db.conn, space, t1.id, r1ac).await.unwrap();

        // Covered task: its criterion (by ordinal) + the design obligation, NOT
        // the unrelated R2.AC1.
        let out1 = assemble_briefing(&db.conn, &issue, Stage::Implement, Some(t1.id)).await.unwrap();
        assert!(out1.text.contains("[R1.AC1] alpha holds"));
        assert!(out1.text.contains("(invariant) stays O(1)"));
        assert!(!out1.text.contains("beta holds"), "covered task isn't shown unrelated criteria");

        // Uncovered task: falls back to ALL requirement acceptance criteria.
        let out2 = assemble_briefing(&db.conn, &issue, Stage::Implement, Some(t2.id)).await.unwrap();
        assert!(out2.text.contains("alpha holds"));
        assert!(out2.text.contains("beta holds"));
        assert!(out2.text.contains("(invariant) stays O(1)"));
    }

    #[tokio::test]
    async fn review_briefing_emits_checklist_and_manifest() {
        let (db, space, issue, root) = seed().await;
        let r1 = add_node(&db, space, issue.id, ArtifactKind::Requirement, "R1", "first", &["alpha holds"], root).await;
        // A design with a cross-cutting obligation.
        let design = loop_service::artifact::create_artifact(&db.conn, space, issue.id, ArtifactKind::Design, "D", ArtifactStatus::Done, ActorKind::Agent, None).await.unwrap();
        loop_service::artifact::add_criterion(&db.conn, design.id, CriterionKind::Invariant, "stays O(1)").await.unwrap();
        // A task that covers R1.AC1 AND has its own acceptance.
        let t1 = loop_service::artifact::create_artifact(&db.conn, space, issue.id, ArtifactKind::Task, "T1", ArtifactStatus::InProgress, ActorKind::Agent, None).await.unwrap();
        loop_service::artifact::add_criterion(&db.conn, t1.id, CriterionKind::Acceptance, "task own ac").await.unwrap();
        let r1ac = loop_service::artifact::get_artifact_detail(&db.conn, r1).await.unwrap().unwrap().criteria[0].id;
        loop_service::coverage::create_coverage(&db.conn, space, t1.id, r1ac).await.unwrap();
        loop_service::link::create_link(&db.conn, space, t1.id, root, LinkKind::DerivesFrom, None).await.unwrap();

        let out = assemble_briefing(&db.conn, &issue, Stage::Review, Some(t1.id)).await.unwrap();
        let t = &out.text;
        // Checklist prints the covered requirement AC handle + the task's own T1.
        assert!(t.contains("[R1.AC1] alpha holds"), "covered AC is in the checklist");
        assert!(t.contains("[T1] task own ac"), "task's own acceptance is in the checklist");
        // The design obligation is shown as awareness-only context, not a handle.
        assert!(t.contains("(invariant) stays O(1)"));
        assert!(t.contains("context only"));
        assert!(t.contains("one check per handle"));

        // The manifest carries the resolution map (handle → criterion id), and ONLY
        // the task-verifiable criteria — never a design obligation.
        let crit = out.manifest.get("criteria").unwrap().as_object().unwrap();
        assert_eq!(crit.len(), 2, "only task-verifiable criteria are injected");
        assert_eq!(crit["R1.AC1"], json!(r1ac));
        assert!(crit.contains_key("T1"));
    }

    #[tokio::test]
    async fn triage_briefing_minimal_without_target() {
        let (db, _space, issue, _root) = seed().await;
        let out = assemble_briefing(&db.conn, &issue, Stage::Triage, None)
            .await
            .unwrap();
        // No target → no lineage / criteria sections, but the core stays.
        assert!(out.text.contains("Triage this issue"));
        assert!(out.text.contains("loop_submit_route"));
        assert_eq!(out.manifest["template"], "triage@v1");
        let sections: Vec<&str> = out.manifest["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["section"].as_str().unwrap())
            .collect();
        assert!(!sections.contains(&"lineage"));
        assert!(!sections.contains(&"acceptance_criteria"));
        assert!(sections.contains(&"issue"));
    }

    #[test]
    fn lineage_is_cycle_protected() {
        // A → B → A: build_lineage must terminate rather than loop forever.
        let link = |from, to| crate::models::loops::LoopLinkRow {
            id: 0,
            from_artifact_id: from,
            to_artifact_id: to,
            kind: LinkKind::DerivesFrom,
            source_revision_id: None,
        };
        let links = vec![link(1, 2), link(2, 1)];
        // Walk from 1: 1 → 2 → (1 already seen) stops. Bounded chain, no hang.
        assert_eq!(build_lineage(&links, 1), vec![1, 2]);
    }

    #[test]
    fn first_paragraph_stops_at_blank_line() {
        assert_eq!(first_paragraph("one\ntwo\n\nthree"), "one\ntwo");
        assert_eq!(first_paragraph("\n\nlead\nmore"), "lead\nmore");
        assert_eq!(first_paragraph("   "), "");
    }
}
