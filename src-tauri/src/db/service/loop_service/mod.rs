//! DB layer for the loop engineering subsystem. CRUD + read models; the
//! compare-and-swap transitions and dispatch leases live in
//! `loop_engine::transitions`.

pub mod artifact;
pub mod coverage;
pub mod criterion_check;
pub mod criterion_ordinals;
pub mod gate_decision;
pub mod inbox;
pub mod issue;
pub mod iteration;
pub mod link;
pub mod memory;
pub mod space;
pub mod validation;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::loop_artifact::{ArtifactKind, ArtifactStatus};
    use crate::db::entities::loop_artifact_revision::ActorKind;
    use crate::db::entities::loop_inbox_item::{InboxKind, InboxStatus};
    use crate::db::entities::loop_issue::{IssuePriority, IssueStatus};
    use crate::db::entities::loop_criterion::CriterionKind;
    use crate::db::entities::loop_iteration::Stage;
    use crate::db::entities::loop_link::LinkKind;
    use crate::db::entities::loop_memory::{MemoryKind, TrustTier};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::models::loops::IssueConfig;

    #[tokio::test]
    async fn create_issue_seeds_root_artifact() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/repo-a").await;
        let space = space::create_space(&db.conn, "Pay", folder_id).await.unwrap();
        let detail = issue::create_issue(
            &db.conn,
            space.id,
            "Fix webhook",
            "the body",
            IssuePriority::High,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();

        assert_eq!(detail.row.seq_no, 1);
        assert_eq!(detail.row.status, IssueStatus::Pending);

        let dag = artifact::list_dag(&db.conn, detail.row.id).await.unwrap();
        assert_eq!(dag.artifacts.len(), 1, "root artifact created");
        assert_eq!(dag.artifacts[0].kind, ArtifactKind::Issue);
        assert_eq!(dag.artifacts[0].status, ArtifactStatus::Done);

        let det = artifact::get_artifact_detail(&db.conn, dag.artifacts[0].id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(det.revisions.len(), 1, "description seeded as revision 1");
        assert_eq!(det.revisions[0].content, "the body");
    }

    #[tokio::test]
    async fn artifacts_links_idempotent_and_dag() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/repo-b").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "I",
            "d",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        let issue_id = issue.row.id;
        let root_id = artifact::list_dag(&db.conn, issue_id).await.unwrap().artifacts[0].id;

        let req = artifact::create_artifact(
            &db.conn,
            space.id,
            issue_id,
            ArtifactKind::Requirement,
            "R1",
            ArtifactStatus::Done,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        artifact::add_revision(&db.conn, req.id, "req body", ActorKind::Agent, None)
            .await
            .unwrap();
        let crit = artifact::add_criterion(&db.conn, req.id, CriterionKind::Acceptance, "must do x")
            .await
            .unwrap();
        assert_eq!(crit.label, "AC-1");
        assert_eq!(crit.kind, CriterionKind::Acceptance);

        // `requirement derives_from issue` — repeated, must dedupe.
        let l1 =
            link::create_link(&db.conn, space.id, req.id, root_id, LinkKind::DerivesFrom, None)
                .await
                .unwrap();
        let l2 =
            link::create_link(&db.conn, space.id, req.id, root_id, LinkKind::DerivesFrom, None)
                .await
                .unwrap();
        assert_eq!(l1.id, l2.id, "link is idempotent");

        let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
        assert_eq!(dag.artifacts.len(), 2);
        assert_eq!(dag.links.len(), 1);

        let det = artifact::get_artifact_detail(&db.conn, req.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(det.revisions.len(), 1);
        assert_eq!(det.criteria.len(), 1);
        assert_eq!(det.links.len(), 1);
    }

    #[tokio::test]
    async fn coverage_idempotent_and_typed_criteria() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/repo-cov").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "I",
            "d",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();
        let issue_id = issue.row.id;

        // A requirement with one acceptance criterion + one constraint.
        let req = artifact::create_artifact(
            &db.conn,
            space.id,
            issue_id,
            ArtifactKind::Requirement,
            "R1",
            ArtifactStatus::Done,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        let ac = artifact::add_criterion(&db.conn, req.id, CriterionKind::Acceptance, "do x")
            .await
            .unwrap();
        artifact::add_criterion(&db.conn, req.id, CriterionKind::Constraint, "no panics")
            .await
            .unwrap();

        // Kinds round-trip through the detail read.
        let det = artifact::get_artifact_detail(&db.conn, req.id).await.unwrap().unwrap();
        assert_eq!(det.criteria.len(), 2);
        assert_eq!(det.criteria[0].kind, CriterionKind::Acceptance);
        assert_eq!(det.criteria[1].kind, CriterionKind::Constraint);

        // A task covers the acceptance criterion; coverage is idempotent.
        let task = artifact::create_artifact(
            &db.conn,
            space.id,
            issue_id,
            ArtifactKind::Task,
            "T1",
            ArtifactStatus::Pending,
            ActorKind::Agent,
            None,
        )
        .await
        .unwrap();
        let c1 = coverage::create_coverage(&db.conn, space.id, task.id, ac.id)
            .await
            .unwrap();
        let c2 = coverage::create_coverage(&db.conn, space.id, task.id, ac.id)
            .await
            .unwrap();
        assert_eq!(c1.id, c2.id, "coverage is idempotent");

        // Surfaced both by list_for_issue and inside the DAG view.
        let cov = coverage::list_for_issue(&db.conn, issue_id).await.unwrap();
        assert_eq!(cov.len(), 1);
        assert_eq!(cov[0].task_artifact_id, task.id);
        assert_eq!(cov[0].criterion_id, ac.id);
        let dag = artifact::list_dag(&db.conn, issue_id).await.unwrap();
        assert_eq!(dag.coverage.len(), 1);
        assert_eq!(dag.coverage[0].criterion_id, ac.id);
    }

    #[tokio::test]
    async fn inbox_upsert_dedupes_pending() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/repo-c").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "I",
            "d",
            IssuePriority::Low,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();

        let a = inbox::upsert_inbox(
            &db.conn,
            space.id,
            issue.row.id,
            None,
            InboxKind::Blocked,
            "artifact:1",
            serde_json::json!({"reason": "x"}),
        )
        .await
        .unwrap();
        let b = inbox::upsert_inbox(
            &db.conn,
            space.id,
            issue.row.id,
            None,
            InboxKind::Blocked,
            "artifact:1",
            serde_json::json!({"reason": "y"}),
        )
        .await
        .unwrap();
        assert_eq!(a.id, b.id, "pending inbox item deduped by subject_key");

        let pending = inbox::list_inbox(&db.conn, space.id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);

        inbox::handle_inbox(&db.conn, a.id, serde_json::json!({"ok": true}))
            .await
            .unwrap();
        let still_pending = inbox::list_inbox(&db.conn, space.id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert_eq!(still_pending.len(), 0);
    }

    #[tokio::test]
    async fn space_summary_and_cascade_delete() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/repo-d").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &db.conn,
            space.id,
            "I",
            "d",
            IssuePriority::Medium,
            Some(&IssueConfig::default()),
        )
        .await
        .unwrap();

        let summaries = space::list_spaces(&db.conn).await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].issue_count, 1);
        assert!(!summaries[0].detached, "live folder is not detached");

        space::delete_space(&db.conn, space.id).await.unwrap();
        // FK cascade removed the issue and its root artifact.
        let dag = artifact::list_dag(&db.conn, issue.row.id).await.unwrap();
        assert_eq!(dag.artifacts.len(), 0, "cascade removed artifacts");
        assert!(space::list_spaces(&db.conn).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn memory_crud_and_stage_matrix() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/repo-e").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();

        memory::create_memory(
            &db.conn,
            space.id,
            MemoryKind::Pitfall,
            ActorKind::Agent,
            "p",
            None,
            "b",
            TrustTier::Proposed,
            memory::MemoryProvenance::default(),
        )
        .await
        .unwrap();
        memory::create_memory(
            &db.conn,
            space.id,
            MemoryKind::Decision,
            ActorKind::Human,
            "d",
            None,
            "b",
            TrustTier::Human,
            memory::MemoryProvenance::default(),
        )
        .await
        .unwrap();
        assert_eq!(memory::list_memory(&db.conn, space.id).await.unwrap().len(), 2);

        // implement stage injects pitfalls, not decisions.
        let injected = memory::list_active_for_stage(&db.conn, space.id, Stage::Implement)
            .await
            .unwrap();
        assert!(injected.iter().any(|m| m.kind == MemoryKind::Pitfall));
        assert!(!injected.iter().any(|m| m.kind == MemoryKind::Decision));
    }
}
