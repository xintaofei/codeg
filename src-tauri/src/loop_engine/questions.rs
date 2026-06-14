//! Routing of a loop iteration's `ask_user_question` into the space inbox.
//!
//! A loop iteration runs an agent that can call the `ask_user_question` MCP tool
//! like any other session (the loop launch always exposes it — see
//! `inject_codeg_mcp`). The existing question machinery already parks the tool
//! call and broadcasts a `QuestionRequest` / `QuestionResolved` pair on the
//! in-process event bus. The engine's bus subscriber
//! ([`crate::loop_engine::LoopEngine::completion_watcher_task`]) reacts to that
//! pair here — writing a `question` inbox card when an iteration raises a
//! question, clearing it when the question is answered or canceled — so a person
//! can discover the blocked iteration from the space inbox and open it to
//! answer.
//!
//! This never touches the question/answer path itself: the answer still flows
//! through the normal `answer_question` route on the iteration's connection. The
//! card is purely a discovery surface, and (like the completion watcher) this is
//! an *additive* bus subscriber — it never modifies the ACP lifecycle.

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::acp::question::QuestionSpec;
use crate::db::entities::loop_inbox_item::{self, InboxKind, InboxStatus};
use crate::db::entities::loop_issue;
use crate::db::entities::loop_iteration::{self, IterationStatus};
use crate::db::service::loop_service::inbox;
use crate::loop_engine::driver::resolve_agent;
use crate::models::agent::AgentType;

use super::LoopEngine;

impl LoopEngine {
    /// A session's agent called `ask_user_question`. When `connection_id` backs a
    /// running loop iteration, file a `question` inbox card so the space inbox
    /// surfaces it (deduped on the question id); otherwise ignore — ordinary and
    /// delegation turns are not loop iterations and keep their existing flow.
    pub async fn on_question_request(
        &self,
        connection_id: &str,
        question_id: &str,
        questions: &[QuestionSpec],
    ) {
        // Resolve the conversation backing this connection (in-memory), then ask
        // the DB whether it is a running loop iteration — same gate as
        // `on_turn_complete`.
        let Some((state, _)) = self.manager.get_state_and_emitter(connection_id).await else {
            return;
        };
        let conversation_id = state.read().await.conversation_id;
        let Some(cid) = conversation_id else {
            return;
        };
        let iter = match loop_iteration::Entity::find()
            .filter(loop_iteration::Column::ConversationId.eq(cid))
            .filter(loop_iteration::Column::Status.eq(IterationStatus::Running))
            .one(&self.db.conn)
            .await
        {
            Ok(Some(it)) => it,
            Ok(None) => return,
            Err(e) => {
                eprintln!("[loop] on_question_request iteration lookup failed: {e}");
                return;
            }
        };
        // The agent that owns this iteration's session, so the inbox can open a
        // viewer with the right rendering before the transcript loads.
        let agent_type = match loop_issue::Entity::find_by_id(iter.issue_id)
            .one(&self.db.conn)
            .await
        {
            Ok(Some(issue)) => {
                let config =
                    crate::loop_engine::config_resolver::effective_config(&self.db.conn, &issue)
                        .await;
                resolve_agent(&config, iter.stage)
            }
            _ => AgentType::ClaudeCode,
        };
        // The payload carries everything the inbox needs to open the iteration
        // viewer and re-render the live question, without a second round-trip.
        let payload = serde_json::json!({
            "question_id": question_id,
            "questions": questions,
            "connection_id": connection_id,
            "conversation_id": cid,
            "agent_type": agent_type,
        });
        let subject = format!("question:{question_id}");
        if let Err(e) = inbox::upsert_inbox(
            &self.db.conn,
            iter.space_id,
            iter.issue_id,
            Some(iter.id),
            InboxKind::Question,
            &subject,
            payload,
        )
        .await
        {
            eprintln!("[loop] on_question_request upsert_inbox failed: {e}");
            return;
        }
        self.emit_changed(iter.space_id, iter.issue_id, "question_raised");
    }

    /// A question was answered (from the iteration viewer or any client) or
    /// canceled (tool call aborted / connection drained). Clear its inbox card if
    /// one is still pending. Idempotent: `question_id` is a UUID, so the
    /// `question:{id}` subject matches at most one pending card.
    pub async fn on_question_resolved(&self, question_id: &str) {
        let subject = format!("question:{question_id}");
        let card = match loop_inbox_item::Entity::find()
            .filter(loop_inbox_item::Column::SubjectKey.eq(&subject))
            .filter(loop_inbox_item::Column::Kind.eq(InboxKind::Question))
            .filter(loop_inbox_item::Column::Status.eq(InboxStatus::Pending))
            .one(&self.db.conn)
            .await
        {
            Ok(Some(c)) => c,
            Ok(None) => return,
            Err(e) => {
                eprintln!("[loop] on_question_resolved lookup failed: {e}");
                return;
            }
        };
        if let Err(e) = inbox::handle_inbox(
            &self.db.conn,
            card.id,
            serde_json::json!({ "action": "answered" }),
        )
        .await
        {
            eprintln!("[loop] on_question_resolved handle_inbox failed: {e}");
            return;
        }
        self.emit_changed(card.space_id, card.issue_id, "question_resolved");
    }
}

#[cfg(test)]
mod tests {
    // `super::*` re-exports the module's own imports (inbox, loop_iteration,
    // loop_issue, IterationStatus, InboxKind/Status, AgentType, IssueConfig,
    // QuestionSpec, the sea_orm traits); the test adds only what's unique to it.
    use super::*;
    use crate::models::loops::IssueConfig;
    use std::sync::Arc;

    use sea_orm::sea_query::Expr;

    use crate::acp::manager::ConnectionManager;
    use crate::acp::question::QuestionOption;
    use crate::db::entities::loop_issue::{IssuePriority, IssueStatus};
    use crate::db::entities::loop_iteration::Stage;
    use crate::db::service::loop_service::{issue, space};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::loop_engine::transitions::{
        cas_issue_status, cas_iteration_status, try_claim_iteration, IterationClaim,
    };
    use crate::web::event_bridge::EventEmitter;

    fn q_spec() -> QuestionSpec {
        QuestionSpec {
            id: "q-0".into(),
            question: "Which approach?".into(),
            header: "Approach".into(),
            multi_select: false,
            options: vec![
                QuestionOption {
                    label: "A".into(),
                    description: String::new(),
                },
                QuestionOption {
                    label: "B".into(),
                    description: String::new(),
                },
            ],
        }
    }

    /// Stand up an engine + a running loop iteration whose `conversation_id` is
    /// bound to a live agent connection, mirroring the dispatch wiring. Returns
    /// the engine, the DB conn, space id, issue id, and the iteration's
    /// connection id.
    async fn setup_running_iteration() -> (
        Arc<LoopEngine>,
        sea_orm::DatabaseConnection,
        i32,
        i32,
        String,
    ) {
        let db = fresh_in_memory_db().await;
        let conn = db.conn.clone();
        let folder_id = seed_folder(&db, "/tmp/loop-questions").await;
        let space = space::create_space(&conn, "S", folder_id).await.unwrap();
        let issue = issue::create_issue(
            &conn,
            space.id,
            "I",
            "b",
            IssuePriority::Medium,
            &IssueConfig::default(),
        )
        .await
        .unwrap();
        cas_issue_status(&conn, issue.row.id, IssueStatus::Pending, IssueStatus::Running)
            .await
            .unwrap();
        let engine = LoopEngine::new(
            db,
            ConnectionManager::new(),
            std::path::PathBuf::from("/tmp/loop-questions-data"),
            EventEmitter::Noop,
        );

        // A running iteration with a conversation id.
        let iter = try_claim_iteration(
            &conn,
            IterationClaim {
                space_id: space.id,
                issue_id: issue.row.id,
                stage: Stage::Triage,
                target_artifact_id: None,
                slot_no: None,
                capability_token: "cap".into(),
                attempt: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
        cas_iteration_status(&conn, iter.id, IterationStatus::Queued, IterationStatus::Running)
            .await
            .unwrap();
        let convo = 7777;
        loop_iteration::Entity::update_many()
            .col_expr(loop_iteration::Column::ConversationId, Expr::value(convo))
            .filter(loop_iteration::Column::Id.eq(iter.id))
            .exec(&conn)
            .await
            .unwrap();

        // A live agent connection whose session is bound to that conversation.
        let conn_id = "iter-conn".to_string();
        engine
            .manager
            .insert_test_connection(&conn_id, AgentType::ClaudeCode, None, EventEmitter::Noop)
            .await;
        engine
            .manager
            .get_state(&conn_id)
            .await
            .unwrap()
            .write()
            .await
            .conversation_id = Some(convo);

        (engine, conn, space.id, issue.row.id, conn_id)
    }

    #[tokio::test]
    async fn question_request_files_a_card_then_resolved_clears_it() {
        let (engine, conn, space_id, _issue_id, conn_id) = setup_running_iteration().await;

        engine
            .on_question_request(&conn_id, "qid-1", &[q_spec()])
            .await;

        let pending = inbox::list_inbox(&conn, space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        let card = pending
            .iter()
            .find(|c| c.kind == InboxKind::Question)
            .expect("a question card was filed");
        assert_eq!(card.subject_key, "question:qid-1");
        assert_eq!(
            card.payload["agent_type"], "claude_code",
            "payload carries the iteration's agent so the viewer renders right"
        );
        assert_eq!(card.payload["connection_id"], "iter-conn");
        assert_eq!(card.payload["conversation_id"], 7777);

        // Resolving the question clears the card.
        engine.on_question_resolved("qid-1").await;
        let still_pending = inbox::list_inbox(&conn, space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert!(
            !still_pending.iter().any(|c| c.kind == InboxKind::Question),
            "the question card is handled once the question resolves"
        );
    }

    #[tokio::test]
    async fn question_from_a_non_loop_connection_is_ignored() {
        let (engine, conn, space_id, _issue_id, _conn_id) = setup_running_iteration().await;
        // A connection bound to no loop iteration (different conversation).
        engine
            .manager
            .insert_test_connection("plain-conn", AgentType::ClaudeCode, None, EventEmitter::Noop)
            .await;
        engine
            .manager
            .get_state("plain-conn")
            .await
            .unwrap()
            .write()
            .await
            .conversation_id = Some(9999);

        engine
            .on_question_request("plain-conn", "qid-x", &[q_spec()])
            .await;

        let pending = inbox::list_inbox(&conn, space_id, Some(InboxStatus::Pending))
            .await
            .unwrap();
        assert!(
            !pending.iter().any(|c| c.kind == InboxKind::Question),
            "no card for a question raised by a non-loop session"
        );
    }
}
