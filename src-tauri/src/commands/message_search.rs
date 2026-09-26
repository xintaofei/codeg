//! ⌘K message search: the search command, and the background indexer that
//! keeps the full-text index current (storage lives in
//! `db::service::message_search_service`).
//!
//! Index lifecycle: [`run_message_indexer`], started by both the desktop app
//! and the standalone server, backfills every conversation shortly after
//! start, then once a minute re-indexes the conversations whose `updated_at`
//! moved since they were last indexed. That covers finished turns (the status
//! change at turn end bumps `updated_at`) and imports without hooking either
//! path.

use std::future::Future;
use std::time::Duration;

use chrono::Utc;
use sea_orm::DatabaseConnection;

use crate::app_error::AppCommandError;
use crate::commands::conversations::get_folder_conversation_core;
use crate::db::error::DbError;
use crate::db::service::message_search_service::{self, MessageSearchHit};
use crate::models::message::MessageTurn;
// Only the desktop command wrapper takes `tauri::State<AppDatabase>`.
#[cfg(feature = "tauri-runtime")]
use crate::db::AppDatabase;

/// Hits returned when the caller does not ask for a number, and the most it
/// may ask for.
const DEFAULT_SEARCH_LIMIT: u32 = 40;
const MAX_SEARCH_LIMIT: u32 = 200;

/// Wait after start before the first pass, so indexing never competes with
/// workspace boot (folder scan, session load, agent connect).
const INDEX_START_DELAY: Duration = Duration::from_secs(15);
/// Gap between passes once caught up.
const INDEX_INTERVAL: Duration = Duration::from_secs(60);
/// Conversations indexed per pass. A full batch means more may be waiting, so
/// the next pass follows after a short pause instead of a whole interval.
const INDEX_BATCH: usize = 25;
const INDEX_BATCH_PAUSE: Duration = Duration::from_millis(200);
/// A conversation touched more recently than this is left for a later pass:
/// the status change at turn end races the agent CLI flushing its transcript,
/// so indexing at once could record the transcript without its last reply and,
/// with `updated_at` then unchanged, never look again.
const SETTLE_SECS: i64 = 20;

pub async fn message_search_core(
    conn: &DatabaseConnection,
    query: &str,
    limit: Option<u32>,
) -> Result<Vec<MessageSearchHit>, AppCommandError> {
    let limit = limit.unwrap_or(DEFAULT_SEARCH_LIMIT).min(MAX_SEARCH_LIMIT);
    Ok(message_search_service::search_messages(conn, query, limit).await?)
}

/// Full-text search over the messages of every conversation the sidebar lists.
#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn message_search(
    db: tauri::State<'_, AppDatabase>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<MessageSearchHit>, AppCommandError> {
    message_search_core(&db.conn, &query, limit).await
}

/// Index up to `batch` conversations that are new or changed since their last
/// indexing and have settled. Returns how many were processed; a full batch
/// means more may be waiting.
pub async fn index_stale_conversations(
    conn: &DatabaseConnection,
    batch: usize,
) -> Result<usize, DbError> {
    index_stale_conversations_with(conn, batch, move |id| async move {
        get_folder_conversation_core(conn, id)
            .await
            .map(|(detail, _)| detail.turns)
    })
    .await
}

/// [`index_stale_conversations`] reading each transcript with `read_turns`, so
/// a test can make a read fail.
async fn index_stale_conversations_with<F, Fut>(
    conn: &DatabaseConnection,
    batch: usize,
    read_turns: F,
) -> Result<usize, DbError>
where
    F: Fn(i32) -> Fut,
    Fut: Future<Output = Result<Vec<MessageTurn>, AppCommandError>>,
{
    let settled_before = Utc::now() - chrono::Duration::seconds(SETTLE_SECS);
    let stale =
        message_search_service::list_stale_conversations(conn, settled_before, batch).await?;
    for conversation in &stale {
        match read_turns(conversation.id).await {
            Ok(turns) => {
                message_search_service::index_conversation(
                    conn,
                    conversation.id,
                    conversation.updated_at,
                    &turns,
                )
                .await?;
            }
            Err(err) => {
                // A transcript that cannot be read (moved, agent uninstalled)
                // is stamped anyway: retrying every minute would not fix it,
                // and the conversation's next real change retries it. Whatever
                // an earlier read indexed goes with it, so search stops
                // returning text that can no longer be opened.
                tracing::debug!(
                    conversation_id = conversation.id,
                    error = %err,
                    "[message-search] could not read the conversation to index it"
                );
                message_search_service::mark_indexed(
                    conn,
                    conversation.id,
                    conversation.updated_at,
                )
                .await?;
            }
        }
    }
    Ok(stale.len())
}

/// Keep the message index current for the life of the process. See the module
/// docs for the lifecycle.
pub async fn run_message_indexer(conn: DatabaseConnection) {
    tokio::time::sleep(INDEX_START_DELAY).await;
    loop {
        let pause = match index_stale_conversations(&conn, INDEX_BATCH).await {
            Ok(done) if done >= INDEX_BATCH => INDEX_BATCH_PAUSE,
            Ok(_) => INDEX_INTERVAL,
            Err(err) => {
                tracing::warn!("[message-search] indexing pass failed: {err}");
                INDEX_INTERVAL
            }
        };
        tokio::time::sleep(pause).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::conversation;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::agent::AgentType;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    async fn set_updated_at(conn: &DatabaseConnection, id: i32, secs_ago: i64) {
        let row = conversation::Entity::find_by_id(id)
            .one(conn)
            .await
            .unwrap()
            .unwrap();
        let mut active: conversation::ActiveModel = row.into();
        active.updated_at = Set(Utc::now() - chrono::Duration::seconds(secs_ago));
        active.update(conn).await.unwrap();
    }

    #[tokio::test]
    async fn stale_pass_indexes_each_conversation_once_until_it_changes() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/fts-proj").await;
        let id = seed_conversation(&db, folder, AgentType::Codex).await;

        // Just touched (inside the settle window): left for a later pass.
        assert_eq!(index_stale_conversations(&db.conn, 25).await.unwrap(), 0);

        // Settled: processed once (it has no transcript to read yet, which
        // still counts), then skipped while unchanged.
        set_updated_at(&db.conn, id, 120).await;
        assert_eq!(index_stale_conversations(&db.conn, 25).await.unwrap(), 1);
        assert_eq!(index_stale_conversations(&db.conn, 25).await.unwrap(), 0);

        // A later change (a finished turn, an import) makes it stale again.
        set_updated_at(&db.conn, id, 60).await;
        assert_eq!(index_stale_conversations(&db.conn, 25).await.unwrap(), 1);
    }

    fn user_turn(text: &str) -> MessageTurn {
        serde_json::from_value(serde_json::json!({
            "id": "t-1",
            "role": "user",
            "blocks": [{ "type": "text", "text": text }],
            "timestamp": "2026-09-24T10:00:00Z",
        }))
        .expect("turn fixture")
    }

    #[tokio::test]
    async fn an_unreadable_transcript_drops_what_was_indexed_before() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/fts-proj").await;
        let id = seed_conversation(&db, folder, AgentType::Custom("test-agent")).await;
        let readable =
            |_: i32| async { Ok::<_, AppCommandError>(vec![user_turn("the flaky upload retry")]) };
        let unreadable = |_: i32| async {
            Err::<Vec<MessageTurn>, _>(AppCommandError::task_execution_failed(
                "transcript unreadable",
            ))
        };
        let search = |query: &'static str| message_search_core(&db.conn, query, None);

        // Indexed while its transcript could be read.
        set_updated_at(&db.conn, id, 120).await;
        assert_eq!(
            index_stale_conversations_with(&db.conn, 25, readable)
                .await
                .unwrap(),
            1
        );
        assert_eq!(search("upload").await.unwrap().len(), 1);

        // It changes, and now the transcript cannot be read: the text indexed
        // before must stop matching, not linger behind the new stamp.
        set_updated_at(&db.conn, id, 60).await;
        assert_eq!(
            index_stale_conversations_with(&db.conn, 25, unreadable)
                .await
                .unwrap(),
            1
        );
        assert!(search("upload").await.unwrap().is_empty());
        assert!(search("up").await.unwrap().is_empty());

        // Stamped all the same, so the next pass leaves it alone.
        assert_eq!(
            index_stale_conversations_with(&db.conn, 25, unreadable)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn stale_pass_works_through_a_backlog_in_batches() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/fts-proj").await;
        for _ in 0..3 {
            let id = seed_conversation(&db, folder, AgentType::Codex).await;
            set_updated_at(&db.conn, id, 120).await;
        }

        assert_eq!(index_stale_conversations(&db.conn, 2).await.unwrap(), 2);
        assert_eq!(index_stale_conversations(&db.conn, 2).await.unwrap(), 1);
        assert_eq!(index_stale_conversations(&db.conn, 2).await.unwrap(), 0);
    }
}
