use std::collections::HashSet;
use std::sync::Arc;

use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Statement};
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
        turns: Arc<Vec<MessageTurn>>,
    },
    Delete(i32),
    SyncMode,
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

    /// Submit already-parsed turns without touching the indexer's queue with a
    /// full deep copy: callers create one shared allocation and the worker
    /// borrows it.
    pub fn submit_turns(&self, conversation_id: i32, turns: Arc<Vec<MessageTurn>>) {
        let _ = self.sender.send(IndexRequest::Turns {
            conversation_id,
            turns,
        });
    }

    pub fn request_delete(&self, conversation_id: i32) {
        let _ = self.sender.send(IndexRequest::Delete(conversation_id));
    }

    /// Ask the worker to re-evaluate mode / user settings asynchronously so a
    /// settings save never blocks the command thread on a full FTS rebuild.
    pub fn request_mode_sync(&self) {
        let _ = self.sender.send(IndexRequest::SyncMode);
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
    if let Err(error) = message_search_service::upgrade_schema_if_needed(&conn).await {
        tracing::error!("[search-index] schema upgrade failed: {error}");
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
                        pending.lock().await.insert(conversation_id);
                        if let Err(error) = process_parse(&conn, conversation_id).await {
                            tracing::warn!("[search-index] parse {conversation_id} failed: {error}");
                        }
                        pending.lock().await.remove(&conversation_id);
                    }
                    IndexRequest::Turns { conversation_id, turns } => {
                        pending.lock().await.insert(conversation_id);
                        if let Err(error) = process_turns(&conn, conversation_id, &turns).await {
                            tracing::warn!("[search-index] turns {conversation_id} failed: {error}");
                        }
                        pending.lock().await.remove(&conversation_id);
                    }
                    IndexRequest::Delete(conversation_id) => {
                        pending.lock().await.insert(conversation_id);
                        if let Err(error) = process_delete(&conn, conversation_id).await {
                            tracing::warn!("[search-index] delete {conversation_id} failed: {error}");
                        }
                        pending.lock().await.remove(&conversation_id);
                    }
                    IndexRequest::SyncMode => {
                        if let Err(error) = drift_resync(&conn, &handle).await {
                            tracing::warn!("[search-index] requested resync failed: {error}");
                        }
                        if let Err(error) = sync_mode_and_progress(&conn, &emitter).await {
                            tracing::warn!("[search-index] requested mode sync failed: {error}");
                        }
                    }
                }
                // Sync once after the queued batch drains instead of after every
                // item: `pending` is populated by the arms above.
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

/// Queue parses for every visible conversation whose indexed document is stale
/// or missing, using one LEFT JOIN instead of an N+1 per-row SELECT. The
/// joined metadata is the same probe `process_turns` records, so unchanged
/// conversations are skipped without re-reading their transcripts.
async fn drift_resync(
    conn: &DatabaseConnection,
    handle: &MessageSearchIndexer,
) -> Result<(), crate::db::error::DbError> {
    let state = message_search_service::ensure_search_state(conn).await?;
    if !state.user_enabled {
        return Ok(());
    }
    let rows = conn
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT c.id, c.updated_at, c.message_count, \
                    d.source_ended_at, d.source_message_count \
             FROM conversation c \
             LEFT JOIN message_search_document d ON d.conversation_id = c.id \
             WHERE c.deleted_at IS NULL AND c.kind != 'loop' \
               AND c.parent_id IS NULL AND c.external_id IS NOT NULL \
             ORDER BY c.updated_at DESC"
                .to_string(),
        ))
        .await?;
    for row in rows {
        let conversation_id = row.try_get_by_index::<i32>(0)?;
        let updated_at = row.try_get_by_index::<chrono::DateTime<chrono::Utc>>(1)?;
        let message_count = row.try_get_by_index::<i32>(2)?;
        let source_ended_at =
            row.try_get_by_index::<Option<chrono::DateTime<chrono::Utc>>>(3)?;
        let source_message_count = row.try_get_by_index::<Option<i32>>(4)?;
        let dirty = source_ended_at != Some(updated_at)
            || source_message_count != Some(message_count);
        if dirty {
            handle.request_parse(conversation_id);
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
    let state = message_search_service::get_search_state(conn).await?;
    if !state.user_enabled {
        return Ok(());
    }
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
    // The cheap source-metadata guard mirrors `drift_resync`'s dirtiness probe:
    // an unchanged detail load never re-normalizes the whole transcript.
    if existing.as_ref().is_some_and(|doc| {
        doc.source_ended_at == source_ended_at
            && doc.source_message_count == source_message_count
    }) {
        return Ok(());
    }

    let document = normalize_turns(turns);
    // Empty turns (typically a transcript whose session file no longer
    // exists) still record a tombstone row so the progress denominator treats
    // the conversation as handled. `drift_resync` sees a non-dirty document
    // and stops re-queueing the parse every ten minutes.
    let sync = sync_flags_for(&state);
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

    // Disabled means "do not store my content": drop existing documents and
    // report a settled (not building) status rather than a permanent 0%.
    if !state.user_enabled {
        message_search_service::clear_all_documents(conn).await?;
        let visible = message_search_service::indexable_conversation_count(conn).await?;
        message_search_service::set_index_progress(conn, 0, Some(chrono::Utc::now())).await?;
        let status = SearchIndexStatus {
            mode: state.mode,
            user_enabled: false,
            user_mode: state.user_mode,
            indexed_conversation_count: 0,
            visible_conversation_count: visible,
            building: false,
            progress: 1.0,
        };
        emit_event(emitter, SEARCH_INDEX_PROGRESS_EVENT, status);
        return Ok(());
    }

    message_search_service::delete_orphan_documents(conn).await?;
    let bytes = message_search_service::total_indexed_text_bytes(conn).await? as f64;
    let threshold_bytes = state.threshold_mb.max(0.000_001) * 1_000_000.0;
    // Real hysteresis: only fall back to scan below half the threshold. The
    // previous `_ if mode == FTS && bytes < threshold * 0.5` arm returned the
    // same tuple as the fallback, so the switch happened at the threshold.
    let (mode, short) = match state.user_mode.as_str() {
        "scan" => (MODE_SCAN, false),
        "fts" => (MODE_FTS, true),
        _ => {
            if bytes >= threshold_bytes {
                (MODE_FTS, true)
            } else if state.mode == MODE_FTS && bytes >= threshold_bytes * 0.5 {
                (MODE_FTS, state.short_fts_enabled)
            } else {
                (MODE_SCAN, false)
            }
        }
    };

    if state.mode != mode || state.short_fts_enabled != short {
        message_search_service::rebuild_fts_and_set_mode(conn, mode, short).await?;
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
    use crate::db::entities::{conversation, message_search_document};
    use crate::db::service::message_search_service::USER_MODE_SCAN;
    use crate::db::service::message_search_service::USER_MODE_AUTO;
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
    async fn stale_metadata_reparses_and_refreshes_source_fields() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-metadata").await;
        let conversation_id =
            seed_conversation(&db, folder_id, crate::models::AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn)
            .await
            .expect("state");

        process_turns(&db.conn, conversation_id, &[turn("same text")])
            .await
            .expect("first");
        let first = message_search_document::Entity::find()
            .filter(message_search_document::Column::ConversationId.eq(conversation_id))
            .one(&db.conn)
            .await
            .expect("read")
            .expect("document");

        crate::db::service::conversation_service::update_status(
            &db.conn,
            conversation_id,
            conversation::ConversationStatus::Completed,
        )
        .await
        .expect("bump");

        process_turns(&db.conn, conversation_id, &[turn("same text")])
            .await
            .expect("second");
        let second = message_search_document::Entity::find()
            .filter(message_search_document::Column::ConversationId.eq(conversation_id))
            .one(&db.conn)
            .await
            .expect("read")
            .expect("document");

        assert_eq!(second.content_hash, first.content_hash);
        assert_ne!(second.source_ended_at, first.source_ended_at);
        assert_eq!(second.source_message_count, first.source_message_count);
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
    #[tokio::test]
    async fn disabling_search_clears_stored_documents() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-disable").await;
        let conversation_id =
            seed_conversation(&db, folder_id, crate::models::AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn)
            .await
            .expect("state");
        message_search_service::upsert_document(
            &db.conn,
            conversation_id,
            &normalize_turns(&[turn("secret text")]),
            None,
            1,
            SyncFlags::default(),
        )
        .await
        .expect("doc");
        message_search_service::set_search_user_settings(&db.conn, false, USER_MODE_AUTO)
            .await
            .expect("settings");

        sync_mode_and_progress(&db.conn, &EventEmitter::Noop)
            .await
            .expect("sync");
        let count = message_search_document::Entity::find()
            .count(&db.conn)
            .await
            .expect("count");
        assert_eq!(count, 0);
    }
}