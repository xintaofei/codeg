//! Persistence for in-place agent handoffs (`acp::handoff`).
//!
//! A conversation row names only its current agent and session; these rows
//! are the chain of earlier segments. Append-only: a handoff is history the
//! moment it happens, and nothing edits it afterwards.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Set,
};

use crate::db::entities::conversation_handoff;
use crate::db::error::DbError;
use crate::models::AgentType;

/// Everything a handoff record needs. `seq` is assigned here, never by the
/// caller, so two records can never claim the same slot.
#[derive(Debug, Clone)]
pub struct NewHandoff {
    pub conversation_id: i32,
    pub from_agent_type: AgentType,
    pub from_external_id: Option<String>,
    pub to_agent_type: AgentType,
    pub to_external_id: String,
    /// `acp::handoff::HandoffPath::as_str()`.
    pub path: &'static str,
    pub carried: bool,
    pub user_turns_before: u32,
    pub note: Option<String>,
    pub briefing: Option<String>,
    pub truncated: bool,
}

/// Append one handoff to the conversation's chain and return the stored row.
pub async fn record(
    conn: &DatabaseConnection,
    new: NewHandoff,
) -> Result<conversation_handoff::Model, DbError> {
    let seq = conversation_handoff::Entity::find()
        .filter(conversation_handoff::Column::ConversationId.eq(new.conversation_id))
        .order_by_desc(conversation_handoff::Column::Seq)
        .one(conn)
        .await?
        .map(|last| last.seq + 1)
        .unwrap_or(0);
    let row = conversation_handoff::ActiveModel {
        id: NotSet,
        conversation_id: Set(new.conversation_id),
        seq: Set(seq),
        from_agent_type: Set(new.from_agent_type.as_wire().into_owned()),
        from_external_id: Set(new.from_external_id),
        to_agent_type: Set(new.to_agent_type.as_wire().into_owned()),
        to_external_id: Set(new.to_external_id),
        path: Set(new.path.to_string()),
        carried: Set(new.carried),
        user_turns_before: Set(i32::try_from(new.user_turns_before).unwrap_or(i32::MAX)),
        note: Set(new.note),
        briefing: Set(new.briefing),
        truncated: Set(new.truncated),
        created_at: Set(Utc::now()),
    };
    Ok(row.insert(conn).await?)
}

/// The conversation's handoff chain, oldest first. Empty for a conversation
/// that never changed agents, which is the overwhelmingly common answer.
pub async fn list_for_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Vec<conversation_handoff::Model>, DbError> {
    Ok(conversation_handoff::Entity::find()
        .filter(conversation_handoff::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_handoff::Column::Seq)
        .all(conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};

    fn new_handoff(conversation_id: i32, to: AgentType, to_ext: &str) -> NewHandoff {
        NewHandoff {
            conversation_id,
            from_agent_type: AgentType::ClaudeCode,
            from_external_id: Some("S1".into()),
            to_agent_type: to,
            to_external_id: to_ext.into(),
            path: "summary",
            carried: false,
            user_turns_before: 3,
            note: Some("finish the tests".into()),
            briefing: Some("briefing".into()),
            truncated: false,
        }
    }

    #[tokio::test]
    async fn records_append_in_seq_order_per_conversation() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-handoff-seq").await;
        let a = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        let b = seed_conversation(&db, folder, AgentType::ClaudeCode).await;

        let first = record(&db.conn, new_handoff(a, AgentType::Codex, "S2"))
            .await
            .expect("first");
        let second = record(&db.conn, new_handoff(a, AgentType::Grok, "S3"))
            .await
            .expect("second");
        // A second conversation starts its own chain at 0.
        let other = record(&db.conn, new_handoff(b, AgentType::Codex, "S9"))
            .await
            .expect("other");

        assert_eq!(first.seq, 0);
        assert_eq!(second.seq, 1);
        assert_eq!(other.seq, 0);

        let chain = list_for_conversation(&db.conn, a).await.expect("list");
        assert_eq!(
            chain.iter().map(|h| h.to_external_id.as_str()).collect::<Vec<_>>(),
            vec!["S2", "S3"]
        );
        assert_eq!(chain[0].from_agent_type, "claude_code");
        assert_eq!(chain[0].to_agent_type, "codex");
        assert_eq!(chain[0].note.as_deref(), Some("finish the tests"));
        assert_eq!(chain[0].user_turns_before, 3);
        assert!(!chain[0].carried);

        assert!(list_for_conversation(&db.conn, 999_999)
            .await
            .expect("unknown")
            .is_empty());
    }

    #[tokio::test]
    async fn custom_agents_round_trip_their_wire_form() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-handoff-wire").await;
        let id = seed_conversation(&db, folder, AgentType::ClaudeCode).await;
        let mut new = new_handoff(id, AgentType::custom("claude-code-2").unwrap(), "S1");
        new.carried = true;
        new.path = "native";
        let row = record(&db.conn, new).await.expect("record");
        assert_eq!(row.to_agent_type, "custom:claude-code-2");
        assert_eq!(
            AgentType::from_wire(&row.to_agent_type),
            Some(AgentType::custom("claude-code-2").unwrap())
        );
        assert!(row.carried);
        assert_eq!(row.path, "native");
    }
}
