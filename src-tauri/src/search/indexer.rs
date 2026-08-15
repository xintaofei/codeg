use std::collections::HashSet;
use std::sync::Arc;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use tokio::sync::{mpsc, Mutex};

use crate::db::entities::message_search_document;
use crate::db::service::message_search_service::{self, SyncFlags, MODE_FTS, MODE_SCAN};
use crate::models::{MessageTurn, SearchIndexStatus};
use crate::search::normalizer::normalize_turns;
use crate::web::event_bridge::{emit_event, EventEmitter, SEARCH_INDEX_PROGRESS_EVENT};

const DRIFT_INTERVAL_SECS: u64 = 10 * 60;

#[derive(Debug)]
enum IndexRequest {
    Parse(i32),
    Turns {
        conversation_id: i32,
        turns: Vec<MessageTurn>,
    },
    Delete(i32),
}

pub struct MessageSearchIndexer {
    sender: mpsc::UnboundedSender<IndexRequest>,
}

impl MessageSearchIndexer {
    pub fn spawn(conn: DatabaseConnection, emitter: EventEmitter) -> Arc<Self> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let handle = Arc::new(Self { sender });
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let worker_handle = Arc::clone(&handle);
        #[cfg(feature = "tauri-runtime")]
        tauri::async_runtime::spawn(async move {
            run_worker(conn, emitter, receiver, pending, worker_handle).await;
        });
        #[cfg(not(feature = "tauri-runtime"))]
        tokio::spawn(async move {
            run_worker(conn, emitter, receiver, pending, worker_handle).await;
        });
        handle
    }

    pub fn request_parse(&self, conversation_id: i32) {
        let _ = self.sender.send(IndexRequest::Parse(conversation_id));
    }

    pub fn submit_turns(&self, conversation_id: i32, turns: Vec<MessageTurn>) {
        let _ = self.sender.send(IndexRequest::Turns {
            conversation_id,
            turns,
        });
    }

    pub fn request_delete(&self, conversation_id: i32) {
        let _ = self.sender.send(IndexRequest::Delete(conversation_id));
    }
}

async fn run_worker(
    conn: DatabaseConnection,
    emitter: EventEmitter,
    mut receiver: mpsc::UnboundedReceiver<IndexRequest>,
    pending: Arc<Mutex<HashSet<i32>>>,
    handle: Arc<MessageSearchIndexer>,
) {
    if let Err(error) = message_search_service::ensure_search_state(&conn).await {
        tracing::error!("[search-index] failed to ensure state: {error}");
    }
    if let Err(error) = drift_resync(&conn, &handle).await {
        tracing::error!("[search-index] initial drift resync failed: {error}");
    }

    let mut drift = tokio::time::interval(std::time::Duration::from_secs(DRIFT_INTERVAL_SECS));
    drift.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            request = receiver.recv() => {
                let Some(request) = request else { break; };
                match request {
                    IndexRequest::Parse(conversation_id) => {
                        if let Err(error) = process_parse(&conn, conversation_id).await {
                            tracing::warn!("[search-index] parse {conversation_id} failed: {error}");
                        }
                        pending.lock().await.remove(&conversation_id);
                    }
                    IndexRequest::Turns { conversation_id, turns } => {
                        if let Err(error) = process_turns(&conn, conversation_id, &turns).await {
                            tracing::warn!("[search-index] turns {conversation_id} failed: {error}");
                        }
                    }
                    IndexRequest::Delete(conversation_id) => {
                        if let Err(error) = process_delete(&conn, conversation_id).await {
                            tracing::warn!("[search-index] delete {conversation_id} failed: {error}");
                        }
                    }
                }
                if pending.lock().await.is_empty() {
                    if let Err(error) = sync_mode_and_progress(&conn, &emitter).await {
                        tracing::warn!("[search-index] mode sync failed: {error}");
                    }
                }
            }
            _ = drift.tick() => {
                if let Err(error) = drift_resync(&conn, &handle).await {
                    tracing::error!("[search-index] drift resync failed: {error}");
                }
                if let Err(error) = sync_mode_and_progress(&conn, &emitter).await {
                    tracing::error!("[search-index] progress sync failed: {error}");
                }
            }
        }
    }
}

async fn drift_resync(
    conn: &DatabaseConnection,
    handle: &MessageSearchIndexer,
) -> Result<(), crate::db::error::DbError> {
    let conversations = message_search_service::list_indexable_conversations(conn).await?;
    for conversation in conversations {
        let document = message_search_document::Entity::find()
            .filter(message_search_document::Column::ConversationId.eq(conversation.id))
            .one(conn)
            .await?;
        let dirty = document.as_ref().is_none_or(|doc| {
            doc.source_ended_at != Some(conversation.updated_at)
                || doc.source_message_count != conversation.message_count
        });
        if dirty {
            handle.request_parse(conversation.id);
        }
    }
    Ok(())
}

async fn process_parse(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<(), crate::db::error::DbError> {
    let (detail, _) =
        crate::commands::conversations::get_folder_conversation_core(conn, conversation_id)
            .await
            .map_err(|error| {
                crate::db::error::DbError::Migration(format!("parse conversation: {error}"))
            })?;
    process_turns(conn, conversation_id, &detail.turns).await
}

async fn process_turns(
    conn: &DatabaseConnection,
    conversation_id: i32,
    turns: &[MessageTurn],
) -> Result<(), crate::db::error::DbError> {
    let document = normalize_turns(turns);
    let state = message_search_service::get_search_state(conn).await?;
    let sync = sync_flags_for(&state);
    let existing = message_search_document::Entity::find()
        .filter(message_search_document::Column::ConversationId.eq(conversation_id))
        .one(conn)
        .await?;
    let conversation = crate::db::entities::conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .filter(|model| model.deleted_at.is_none());
    let Some(conversation) = conversation else {
        return Ok(());
    };
    let source_ended_at = Some(conversation.updated_at);
    let source_message_count = conversation.message_count;
    // Empty turns (typically a transcript whose session file no longer
    // exists) still record a tombstone row so the progress denominator treats
    // the conversation as handled. `drift_resync` sees a non-dirty document
    // and stops re-queueing the parse every ten minutes.
    if existing.as_ref().is_some_and(|doc| {
        doc.content_hash == document.content_hash
            && doc.source_ended_at == source_ended_at
            && doc.source_message_count == source_message_count
    }) {
        return Ok(());
    }
    message_search_service::upsert_document(
        conn,
        conversation_id,
        &document,
        source_ended_at,
        source_message_count,
        sync,
    )
    .await?;
    Ok(())
}

async fn process_delete(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<(), crate::db::error::DbError> {
    let state = message_search_service::get_search_state(conn).await?;
    let sync = sync_flags_for(&state);
    message_search_service::delete_document(conn, conversation_id, sync).await
}

fn sync_flags_for(state: &crate::db::entities::search_index_state::Model) -> SyncFlags {
    SyncFlags {
        trigram: state.mode == MODE_FTS,
        short: state.mode == MODE_FTS && state.short_fts_enabled,
    }
}

pub(crate) async fn sync_mode_and_progress(
    conn: &DatabaseConnection,
    emitter: &EventEmitter,
) -> Result<(), crate::db::error::DbError> {
    let state = message_search_service::ensure_search_state(conn).await?;
    let bytes = message_search_service::total_indexed_text_bytes(conn).await? as f64;
    let threshold_bytes = state.threshold_mb.max(0.000_001) * 1_000_000.0;
    let (mode, short) = match state.user_mode.as_str() {
        "scan" => (MODE_SCAN, false),
        "fts" => (MODE_FTS, true),
        _ if bytes >= threshold_bytes => (MODE_FTS, true),
        _ if state.mode == MODE_FTS && bytes < threshold_bytes * 0.5 => (MODE_SCAN, false),
        _ => (MODE_SCAN, false),
    };

    if state.mode != mode || state.short_fts_enabled != short {
        message_search_service::set_search_mode(conn, mode, short).await?;
        message_search_service::rebuild_fts_from_documents(
            conn,
            SyncFlags {
                trigram: mode == MODE_FTS,
                short: mode == MODE_FTS && short,
            },
        )
        .await?;
    }

    let indexed = message_search_document::Entity::find().count(conn).await? as i32;
    let visible = message_search_service::indexable_conversation_count(conn).await?;
    message_search_service::set_index_progress(conn, indexed, Some(chrono::Utc::now())).await?;
    let progress = if visible > 0 {
        (indexed as f64 / visible as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let status = SearchIndexStatus {
        mode: mode.to_string(),
        user_enabled: state.user_enabled,
        user_mode: state.user_mode,
        indexed_conversation_count: indexed,
        visible_conversation_count: visible,
        building: false,
        progress,
    };
    emit_event(emitter, SEARCH_INDEX_PROGRESS_EVENT, status);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::message_search_service::USER_MODE_SCAN;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::{ContentBlock, TurnRole};
    use sea_orm::ActiveModelTrait;

    fn turn(text: &str) -> MessageTurn {
        MessageTurn {
            id: "t1".to_string(),
            role: TurnRole::User,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: chrono::Utc::now(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }
    }

    #[tokio::test]
    async fn mode_sync_upgrades_fts_when_threshold_crossed() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-indexer").await;
        let conversation_id =
            seed_conversation(&db, folder_id, crate::models::AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn)
            .await
            .expect("state");
        let state = message_search_service::get_search_state(&db.conn)
            .await
            .expect("state");
        let mut active: crate::db::entities::search_index_state::ActiveModel = state.into();
        active.threshold_mb = sea_orm::Set(0.00001);
        active.update(&db.conn).await.expect("threshold");
        message_search_service::upsert_document(
            &db.conn,
            conversation_id,
            &normalize_turns(&[turn("会话记录")]),
            None,
            1,
            SyncFlags::default(),
        )
        .await
        .expect("doc");

        sync_mode_and_progress(&db.conn, &EventEmitter::Noop)
            .await
            .expect("sync");
        let state = message_search_service::get_search_state(&db.conn)
            .await
            .expect("state");
        assert_eq!(state.mode, MODE_FTS);
        assert!(state.short_fts_enabled);
    }

    #[tokio::test]
    async fn unchanged_turns_skip_a_second_write() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-skip").await;
        let conversation_id =
            seed_conversation(&db, folder_id, crate::models::AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn)
            .await
            .expect("state");
        let turns = vec![turn("same text")];
        process_turns(&db.conn, conversation_id, &turns)
            .await
            .expect("first");
        process_turns(&db.conn, conversation_id, &turns)
            .await
            .expect("second");
        let docs =
            message_search_service::list_documents_by_conversation(&db.conn, &[conversation_id])
                .await
                .expect("docs");
        assert_eq!(docs.len(), 1);
    }

    #[tokio::test]
    async fn empty_turns_leave_a_tombstone_document() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-tombstone").await;
        let conversation_id =
            seed_conversation(&db, folder_id, crate::models::AgentType::ClaudeCode).await;
        message_search_service::ensure_search_state(&db.conn)
            .await
            .expect("state");

        process_turns(&db.conn, conversation_id, &[])
            .await
            .expect("tombstone");

        let docs =
            message_search_service::list_documents_by_conversation(&db.conn, &[conversation_id])
                .await
                .expect("docs");
        assert_eq!(docs.len(), 1);
        assert!(docs[0].1.is_empty());
    }

    #[tokio::test]
    async fn scan_user_mode_clears_fts_posting() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-scan-mode").await;
        let conversation_id =
            seed_conversation(&db, folder_id, crate::models::AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn)
            .await
            .expect("state");
        message_search_service::set_search_mode(&db.conn, MODE_FTS, true)
            .await
            .expect("mode");
        message_search_service::upsert_document(
            &db.conn,
            conversation_id,
            &normalize_turns(&[turn("会话")]),
            None,
            1,
            SyncFlags {
                trigram: true,
                short: true,
            },
        )
        .await
        .expect("doc");
        message_search_service::set_search_user_settings(&db.conn, true, USER_MODE_SCAN)
            .await
            .expect("user mode");
        sync_mode_and_progress(&db.conn, &EventEmitter::Noop)
            .await
            .expect("sync");
        let state = message_search_service::get_search_state(&db.conn)
            .await
            .expect("state");
        assert_eq!(state.mode, MODE_SCAN);
    }
}
