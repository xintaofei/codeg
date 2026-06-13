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

use crate::db::entities::loop_iteration::Stage;
use crate::db::entities::loop_link::LinkKind;
use crate::db::entities::loop_memory::{self, MemoryKind};
use crate::db::entities::loop_issue;
use crate::db::service::loop_service;
use crate::loop_engine::LoopError;

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
            "Produce the design that satisfies the requirement you were given. \
             Describe the approach, the components and their responsibilities, the \
             data flow, and the trade-offs you weighed. Stay within the issue's \
             scope; do not invent new requirements."
        }
        Stage::Plan => {
            "Break the work into a sequence of small, self-contained implementation \
             tasks. Each task must be doable and verifiable on its own and carry \
             enough detail (files, approach, acceptance criteria) for an implementer \
             with no other context to execute it."
        }
        Stage::Implement => {
            "Implement the task in the provided worktree. Make the change, keep it \
             scoped to this task, and ensure the acceptance criteria are met. The \
             engine commits your work — you do not need to commit."
        }
        Stage::Review => {
            "Review the implementation against its acceptance criteria. Verify each \
             criterion is met and look for defects, omissions, and regressions. \
             Return a single verdict: `pass` only if every criterion holds, \
             otherwise `fail` with specific, actionable findings."
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
            "Call `loop_submit_artifacts` exactly once with your design(s) (the kind \
             is inferred as `design`)."
        }
        Stage::Plan => {
            "Call `loop_submit_artifacts` exactly once with the task breakdown (the \
             kind is inferred as `task`). Put per-task acceptance criteria in each \
             artifact's `criteria`."
        }
        Stage::Implement => {
            "Do not call a submit tool — the engine detects and commits your \
             worktree changes. If you are blocked, call `loop_report_blocked`."
        }
        Stage::Review => {
            "Call `loop_submit_review` exactly once with your `verdict` \
             (pass / fail) and `findings`."
        }
        Stage::Finalize => {
            "Call `loop_submit_artifacts` exactly once with the result summary."
        }
    }
}

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

    // ④ Lineage + ⑤ acceptance criteria — both need the target's chain details.
    if let Some(target) = target_artifact_id {
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

            // ⑤ Acceptance criteria: target + its direct parent, verbatim.
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

    // ⑥ Stage instruction — what to do this turn.
    sections.push(format!("# Your task\n{}", stage_instruction(stage)));
    components.push(json!({ "section": "stage_instruction", "stage": stage_label(stage) }));

    // ⑦ Tool contract — how to submit.
    sections.push(format!("# How to submit\n{}", tool_contract(stage)));
    components.push(json!({ "section": "tool_contract", "stage": stage_label(stage) }));

    let manifest = json!({
        "v": 1,
        "template": format!("{}@v1", stage_label(stage)),
        "components": components,
    });

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
            &IssueConfig::default(),
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
            loop_service::artifact::add_criterion(&db.conn, art.id, c)
                .await
                .unwrap();
        }
        loop_service::link::create_link(&db.conn, space_id, art.id, source, LinkKind::DerivesFrom)
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
            "No new dependencies without approval.",
        )
        .await
        .unwrap();
        loop_service::memory::create_memory(
            &db.conn,
            space,
            MemoryKind::Decision,
            ActorKind::Agent,
            "Token store",
            "Use the existing keyring abstraction.",
        )
        .await
        .unwrap();
        loop_service::memory::create_memory(
            &db.conn,
            space,
            MemoryKind::Pitfall,
            ActorKind::Agent,
            "Flaky test",
            "auth_test is order-dependent.",
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
        // ④ lineage: requirement (target) verbatim + issue-root ancestor summary.
        assert!(t.contains("R1: credential check (direct parent)"));
        assert!(t.contains("The system must verify a username/password pair."));
        assert!(t.contains("(ancestor)"));
        // ⑤ acceptance criteria from the requirement.
        assert!(t.contains("Acceptance criteria"));
        assert!(t.contains("Rejects an unknown user"));
        // ⑥ stage instruction (design-specific) + ⑦ tool contract.
        assert!(t.contains("Produce the design"));
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
            "lineage",
            "acceptance_criteria",
            "stage_instruction",
            "tool_contract",
        ] {
            assert!(sections.contains(&expected), "manifest missing {expected}");
        }
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

    #[tokio::test]
    async fn lineage_is_cycle_protected() {
        // A → B → A: build_lineage must terminate rather than loop forever.
        let (db, space, issue, root) = seed().await;
        let a = add_node(
            &db,
            space,
            issue.id,
            ArtifactKind::Requirement,
            "A",
            "node a",
            &[],
            root,
        )
        .await;
        let b = add_node(
            &db,
            space,
            issue.id,
            ArtifactKind::Design,
            "B",
            "node b",
            &[],
            a,
        )
        .await;
        // Add the back-edge A→B to close the cycle (A derives_from B as well).
        loop_service::link::create_link(&db.conn, space, a, b, LinkKind::DerivesFrom)
            .await
            .unwrap();

        // Should return promptly with a bounded chain, not hang.
        let out = assemble_briefing(&db.conn, &issue, Stage::Design, Some(a))
            .await
            .unwrap();
        assert!(out.text.contains("Lineage"));
    }

    #[test]
    fn first_paragraph_stops_at_blank_line() {
        assert_eq!(first_paragraph("one\ntwo\n\nthree"), "one\ntwo");
        assert_eq!(first_paragraph("\n\nlead\nmore"), "lead\nmore");
        assert_eq!(first_paragraph("   "), "");
    }
}
