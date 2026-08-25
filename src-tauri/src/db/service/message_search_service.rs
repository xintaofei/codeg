use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Set, Statement,
    TransactionTrait,
};

use crate::db::entities::{
    conversation, conversation::ConversationKind, folder, message_search_document,
    search_index_state,
};
use crate::db::error::DbError;
use crate::models::AgentType;
use crate::search::normalizer::{NormalizedBlockOffset, NormalizedDocument};

pub const SEARCH_SCHEMA_VERSION: i32 = 3;
pub const MODE_SCAN: &str = "scan";
pub const MODE_FTS: &str = "fts";
pub const USER_MODE_AUTO: &str = "auto";
pub const USER_MODE_SCAN: &str = "scan";
pub const USER_MODE_FTS: &str = "fts";

/// Which optional FTS tables should be kept in sync with a document write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncFlags {
    pub trigram: bool,
    pub short: bool,
}

pub async fn ensure_search_state(
    conn: &DatabaseConnection,
) -> Result<search_index_state::Model, DbError> {
    if let Some(state) = search_index_state::Entity::find_by_id(1).one(conn).await? {
        return Ok(state);
    }
    let state = search_index_state::ActiveModel {
        id: Set(1),
        schema_version: Set(SEARCH_SCHEMA_VERSION),
        mode: Set(MODE_SCAN.to_string()),
        threshold_mb: Set(40.0),
        short_fts_enabled: Set(false),
        indexed_conversation_count: Set(0),
        last_backfill_at: NotSet,
        user_enabled: Set(true),
        user_mode: Set(USER_MODE_AUTO.to_string()),
    };
    Ok(state.insert(conn).await?)
}

pub async fn get_search_state<C>(
    conn: &C,
) -> Result<search_index_state::Model, DbError>
where
    C: ConnectionTrait,
{
    search_index_state::Entity::find_by_id(1)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound("search_index_state row is missing".to_string()))
}

pub async fn set_search_mode(
    conn: &DatabaseConnection,
    mode: &str,
    short_fts_enabled: bool,
) -> Result<(), DbError> {
    let Some(state) = search_index_state::Entity::find_by_id(1).one(conn).await? else {
        return Err(DbError::NotFound(
            "search_index_state row is missing".to_string(),
        ));
    };
    if !matches!(mode, MODE_SCAN | MODE_FTS) {
        return Err(DbError::Validation(format!("invalid search mode: {mode}")));
    }
    let mut active: search_index_state::ActiveModel = state.into();
    active.mode = Set(mode.to_string());
    active.short_fts_enabled = Set(short_fts_enabled);
    active.update(conn).await?;
    Ok(())
}

/// Atomically rebuild the optional FTS postings and switch modes in one
/// transaction. If the rebuild fails, the rollback keeps the previous mode and
/// tables intact so the indexer can retry on its next tick instead of being
/// stranded in `fts` with empty postings.
pub async fn rebuild_fts_and_set_mode(
    conn: &DatabaseConnection,
    mode: &str,
    short_fts_enabled: bool,
) -> Result<(), DbError> {
    if !matches!(mode, MODE_SCAN | MODE_FTS) {
        return Err(DbError::Validation(format!("invalid search mode: {mode}")));
    }
    let txn = conn.begin().await?;
    txn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM message_search_trigram".to_string(),
    ))
    .await?;
    txn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM message_search_short".to_string(),
    ))
    .await?;
    let sync = SyncFlags {
        trigram: mode == MODE_FTS,
        short: mode == MODE_FTS && short_fts_enabled,
    };
    let documents = message_search_document::Entity::find().all(&txn).await?;
    for model in &documents {
        let doc = NormalizedDocument {
            text: model.text.clone(),
            content_hash: model.content_hash.clone(),
            blocks: parse_block_offsets(&model.block_offsets),
        };
        sync_fts_rows(&txn, model.id, &doc, sync).await?;
    }
    let state = get_search_state(&txn).await?;
    let mut active: search_index_state::ActiveModel = state.into();
    active.mode = Set(mode.to_string());
    active.short_fts_enabled = Set(short_fts_enabled);
    active.update(&txn).await?;
    txn.commit().await?;
    Ok(())
}

/// Bring a pre-v3 schema (or an otherwise stale singleton row) up to date and
/// repopulate the FTS postings that the migration had to drop.
pub async fn upgrade_schema_if_needed(conn: &DatabaseConnection) -> Result<bool, DbError> {
    let state = ensure_search_state(conn).await?;
    if state.schema_version >= SEARCH_SCHEMA_VERSION {
        return Ok(false);
    }
    let sync = SyncFlags {
        trigram: state.mode == MODE_FTS,
        short: state.mode == MODE_FTS && state.short_fts_enabled,
    };
    rebuild_fts_from_documents(conn, sync).await?;
    let mut active: search_index_state::ActiveModel = state.into();
    active.schema_version = Set(SEARCH_SCHEMA_VERSION);
    active.update(conn).await?;
    Ok(true)
}

/// Drop every stored content document and its FTS rows. Used when content
/// search is disabled so plaintext transcripts are not retained in the DB.
pub async fn clear_all_documents(conn: &DatabaseConnection) -> Result<(), DbError> {
    let txn = conn.begin().await?;
    txn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM message_search_trigram".to_string(),
    ))
    .await?;
    txn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM message_search_short".to_string(),
    ))
    .await?;
    txn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM message_search_document".to_string(),
    ))
    .await?;
    let state = get_search_state(&txn).await?;
    let mut active: search_index_state::ActiveModel = state.into();
    active.indexed_conversation_count = Set(0);
    active.update(&txn).await?;
    txn.commit().await?;
    Ok(())
}

/// Reclaim documents whose conversation no longer satisfies the visibility
/// filters (soft-deleted, loop, delegation child, or missing row). Deletions
/// performed while the app was not running have no event to consume, so this
/// sweep is the backstop.
pub async fn delete_orphan_documents(conn: &DatabaseConnection) -> Result<u64, DbError> {
    let state = ensure_search_state(conn).await?;
    let sync = SyncFlags {
        trigram: state.mode == MODE_FTS,
        short: state.mode == MODE_FTS && state.short_fts_enabled,
    };
    let orphans: Vec<i32> = conn
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT d.id FROM message_search_document d \
             LEFT JOIN conversation c ON c.id = d.conversation_id \
             WHERE c.id IS NULL OR c.deleted_at IS NOT NULL \
                OR c.parent_id IS NOT NULL OR c.kind = 'loop'"
                .to_string(),
        ))
        .await?
        .into_iter()
        .map(|row| row.try_get_by_index::<i32>(0))
        .collect::<Result<Vec<_>, _>>()?;
    if orphans.is_empty() {
        return Ok(0);
    }
    let txn = conn.begin().await?;
    for id in &orphans {
        delete_fts_rows(&txn, *id, sync).await?;
        message_search_document::Entity::delete_by_id(*id)
            .exec(&txn)
            .await?;
    }
    txn.commit().await?;
    Ok(orphans.len() as u64)
}

pub async fn set_search_user_settings(
    conn: &DatabaseConnection,
    enabled: bool,
    user_mode: &str,
) -> Result<(), DbError> {
    let Some(state) = search_index_state::Entity::find_by_id(1).one(conn).await? else {
        return Err(DbError::NotFound(
            "search_index_state row is missing".to_string(),
        ));
    };
    if !matches!(user_mode, USER_MODE_AUTO | USER_MODE_SCAN | USER_MODE_FTS) {
        return Err(DbError::Validation(format!(
            "invalid user search mode: {user_mode}"
        )));
    }
    let mut active: search_index_state::ActiveModel = state.into();
    active.user_enabled = Set(enabled);
    active.user_mode = Set(user_mode.to_string());
    active.update(conn).await?;
    Ok(())
}

pub async fn set_index_progress(
    conn: &DatabaseConnection,
    indexed_conversation_count: i32,
    backfill_at: Option<chrono::DateTime<Utc>>,
) -> Result<(), DbError> {
    let Some(state) = search_index_state::Entity::find_by_id(1).one(conn).await? else {
        return Err(DbError::NotFound(
            "search_index_state row is missing".to_string(),
        ));
    };
    let mut active: search_index_state::ActiveModel = state.into();
    active.indexed_conversation_count = Set(indexed_conversation_count);
    if let Some(backfill_at) = backfill_at {
        active.last_backfill_at = Set(Some(backfill_at));
    }
    active.update(conn).await?;
    Ok(())
}

/// Upsert one conversation document and its enabled FTS rows atomically.
///
/// Returns the stable document row id, which is also both FTS tables' rowid.
pub async fn upsert_document(
    conn: &DatabaseConnection,
    conversation_id: i32,
    doc: &NormalizedDocument,
    source_ended_at: Option<chrono::DateTime<Utc>>,
    source_message_count: i32,
    sync: SyncFlags,
) -> Result<i32, DbError> {
    let txn = conn.begin().await?;
    let existing = message_search_document::Entity::find()
        .filter(message_search_document::Column::ConversationId.eq(conversation_id))
        .one(&txn)
        .await?;

    let id = if let Some(existing) = existing {
        let id = existing.id;
        let mut active: message_search_document::ActiveModel = existing.into();
        active.text = Set(doc.text.clone());
        active.content_hash = Set(doc.content_hash.clone());
        active.block_offsets = Set(block_offsets_json(doc)?);
        active.source_ended_at = Set(source_ended_at);
        active.source_message_count = Set(source_message_count);
        active.updated_at = Set(Utc::now());
        active.update(&txn).await?;
        id
    } else {
        let now = Utc::now();
        let model = message_search_document::ActiveModel {
            id: NotSet,
            conversation_id: Set(conversation_id),
            text: Set(doc.text.clone()),
            content_hash: Set(doc.content_hash.clone()),
            block_offsets: Set(block_offsets_json(doc)?),
            source_ended_at: Set(source_ended_at),
            source_message_count: Set(source_message_count),
            updated_at: Set(now),
        };
        model.insert(&txn).await?.id
    };

    sync_fts_rows(&txn, id, doc, sync).await?;
    txn.commit().await?;
    Ok(id)
}

fn block_offsets_json(doc: &NormalizedDocument) -> Result<String, DbError> {
    serde_json::to_string(&doc.blocks)
        .map_err(|error| DbError::Migration(format!("serialize block offsets: {error}")))
}

pub async fn delete_document(
    conn: &DatabaseConnection,
    conversation_id: i32,
    sync: SyncFlags,
) -> Result<(), DbError> {
    let txn = conn.begin().await?;
    if let Some(existing) = message_search_document::Entity::find()
        .filter(message_search_document::Column::ConversationId.eq(conversation_id))
        .one(&txn)
        .await?
    {
        delete_fts_rows(&txn, existing.id, sync).await?;
        message_search_document::Entity::delete_by_id(existing.id)
            .exec(&txn)
            .await?;
    }
    txn.commit().await?;
    Ok(())
}

async fn sync_fts_rows<C>(
    conn: &C,
    id: i32,
    doc: &NormalizedDocument,
    sync: SyncFlags,
) -> Result<(), DbError>
where
    C: ConnectionTrait,
{
    delete_fts_rows(conn, id, sync).await?;
    if sync.trigram {
        conn.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO message_search_trigram(rowid, text) VALUES(?, ?)",
            [id.into(), doc.text.clone().into()],
        ))
        .await?;
    }
    if sync.short {
        let tokens = crate::search::normalizer::short_index_tokens(&doc.text);
        conn.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO message_search_short(rowid, words, bigrams) VALUES(?, ?, ?)",
            [
                id.into(),
                tokens.words.clone().into(),
                tokens.bigrams.clone().into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

async fn delete_fts_rows<C>(conn: &C, id: i32, sync: SyncFlags) -> Result<(), DbError>
where
    C: ConnectionTrait,
{
    if sync.trigram {
        conn.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "DELETE FROM message_search_trigram WHERE rowid = ?",
            [id.into()],
        ))
        .await?;
    }
    if sync.short {
        conn.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "DELETE FROM message_search_short WHERE rowid = ?",
            [id.into()],
        ))
        .await?;
    }
    Ok(())
}

pub async fn total_indexed_text_bytes(conn: &DatabaseConnection) -> Result<i64, DbError> {
    let row = conn
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COALESCE(SUM(LENGTH(CAST(text AS BLOB))), 0) \
             FROM message_search_document"
                .to_string(),
        ))
        .await?
        .map(|result| result.try_get_by_index::<i64>(0))
        .transpose()?
        .unwrap_or(0);
    Ok(row)
}

/// Count conversations that are searchable under the same filters the
/// existing title search applies.
pub async fn visible_conversation_count(
    conn: &DatabaseConnection,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
) -> Result<i64, DbError> {
    let mut query = conversation::Entity::find()
        .filter(conversation::Column::DeletedAt.is_null())
        .filter(conversation::Column::Kind.ne(ConversationKind::Loop))
        .filter(conversation::Column::ParentId.is_null());

    match folder_ids {
        Some(ids) if !ids.is_empty() => {
            query = query.filter(conversation::Column::FolderId.is_in(ids));
        }
        Some(_) => return Ok(0),
        None => {
            let active_ids: Vec<i32> = folder::Entity::find()
                .filter(folder::Column::DeletedAt.is_null())
                .all(conn)
                .await?
                .into_iter()
                .map(|folder| folder.id)
                .collect();
            if active_ids.is_empty() {
                return Ok(0);
            }
            query = query.filter(conversation::Column::FolderId.is_in(active_ids));
        }
    }

    if let Some(agent_type) = agent_type {
        let agent_str = serde_json::to_value(agent_type)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        query = query.filter(conversation::Column::AgentType.eq(agent_str));
    }

    Ok(i64::try_from(query.count(conn).await?).unwrap_or(i64::MAX))
}

pub async fn indexable_conversation_count(conn: &DatabaseConnection) -> Result<i64, DbError> {
    Ok(i64::try_from(
        conversation::Entity::find()
            .filter(conversation::Column::DeletedAt.is_null())
            .filter(conversation::Column::Kind.ne(ConversationKind::Loop))
            .filter(conversation::Column::ParentId.is_null())
            .filter(conversation::Column::ExternalId.is_not_null())
            .count(conn)
            .await?,
    )
    .unwrap_or(i64::MAX))
}

pub async fn list_documents_by_conversation(
    conn: &DatabaseConnection,
    conversation_ids: &[i32],
) -> Result<Vec<(i32, String, Vec<NormalizedBlockOffset>)>, DbError> {
    if conversation_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(message_search_document::Entity::find()
        .filter(message_search_document::Column::ConversationId.is_in(conversation_ids.to_vec()))
        .all(conn)
        .await?
        .into_iter()
        .map(|model| {
            (
                model.conversation_id,
                model.text,
                parse_block_offsets(&model.block_offsets),
            )
        })
        .collect())
}

/// Decode indexed block offsets. Legacy rows fall back to an empty manifest,
/// which keeps search working but disables precise jump/highlighting.
pub fn parse_block_offsets(raw: &str) -> Vec<NormalizedBlockOffset> {
    serde_json::from_str(raw).unwrap_or_default()
}

/// Rebuild the optional FTS postings from the authoritative document table.
///
/// Used when switching into FTS mode; rows for disabled tables are cleared.
pub async fn rebuild_fts_from_documents(
    conn: &DatabaseConnection,
    sync: SyncFlags,
) -> Result<(), DbError> {
    let txn = conn.begin().await?;
    txn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM message_search_trigram".to_string(),
    ))
    .await?;
    txn.execute(Statement::from_string(
        DbBackend::Sqlite,
        "DELETE FROM message_search_short".to_string(),
    ))
    .await?;

    if sync.trigram || sync.short {
        let documents = message_search_document::Entity::find().all(&txn).await?;
        for model in documents {
            let doc = NormalizedDocument {
                text: model.text.clone(),
                content_hash: model.content_hash.clone(),
                blocks: parse_block_offsets(&model.block_offsets),
            };
            if sync.trigram {
                txn.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "INSERT INTO message_search_trigram(rowid, text) VALUES(?, ?)",
                    [model.id.into(), doc.text.clone().into()],
                ))
                .await?;
            }
            if sync.short {
                let tokens = crate::search::normalizer::short_index_tokens(&doc.text);
                txn.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "INSERT INTO message_search_short(rowid, words, bigrams) VALUES(?, ?, ?)",
                    [model.id.into(), tokens.words.into(), tokens.bigrams.into()],
                ))
                .await?;
            }
        }
    }
    txn.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::search::normalizer::{NormalizedBlockOffset, NormalizedDocument};

    fn doc(text: &str) -> NormalizedDocument {
        NormalizedDocument {
            text: text.to_string(),
            content_hash: crate::search::normalizer::sha256_hex(text),
            blocks: Vec::new(),
        }
    }

    #[tokio::test]
    async fn upsert_persists_block_offsets() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-offsets").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let doc = NormalizedDocument {
            text: "hello".to_string(),
            content_hash: crate::search::normalizer::sha256_hex("hello"),
            blocks: vec![NormalizedBlockOffset {
                turn_id: "turn-1".to_string(),
                block_index: 0,
                start: 0,
                end: 5,
                leading_trim: 0,
            }],
        };

        upsert_document(
            &db.conn,
            conversation_id,
            &doc,
            None,
            1,
            SyncFlags::default(),
        )
        .await
        .expect("insert");

        let rows = list_documents_by_conversation(&db.conn, &[conversation_id])
            .await
            .expect("documents");
        assert_eq!(rows[0].2[0].turn_id, "turn-1");
        assert_eq!(rows[0].2[0].block_index, 0);
    }

    #[tokio::test]
    async fn state_starts_with_scan_defaults() {
        let db = fresh_in_memory_db().await;
        let state = ensure_search_state(&db.conn).await.expect("state");
        assert_eq!(state.id, 1);
        assert_eq!(state.mode, MODE_SCAN);
        assert_eq!(state.threshold_mb, 40.0);
        assert!(state.user_enabled);
        assert_eq!(state.user_mode, USER_MODE_AUTO);
    }

    #[tokio::test]
    async fn upsert_preserves_id_and_updates_hash() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-service").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;

        let first_id = upsert_document(
            &db.conn,
            conversation_id,
            &doc("first"),
            None,
            1,
            SyncFlags {
                trigram: true,
                short: true,
            },
        )
        .await
        .expect("insert");

        let second_id = upsert_document(
            &db.conn,
            conversation_id,
            &doc("second"),
            None,
            2,
            SyncFlags {
                trigram: true,
                short: true,
            },
        )
        .await
        .expect("update");

        assert_eq!(first_id, second_id);
        let rows = list_documents_by_conversation(&db.conn, &[conversation_id])
            .await
            .expect("documents");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "second");
    }

    #[tokio::test]
    async fn delete_removes_fts_rows_and_document() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-delete").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::ClaudeCode).await;
        let id = upsert_document(
            &db.conn,
            conversation_id,
            &doc("delete me"),
            None,
            1,
            SyncFlags {
                trigram: true,
                short: true,
            },
        )
        .await
        .expect("insert");

        delete_document(
            &db.conn,
            conversation_id,
            SyncFlags {
                trigram: true,
                short: true,
            },
        )
        .await
        .expect("delete");

        let orphan: i64 = db
            .conn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT COUNT(*) FROM message_search_trigram WHERE rowid = ?",
                [id.into()],
            ))
            .await
            .expect("count")
            .unwrap()
            .try_get_by_index::<i64>(0)
            .expect("count value");
        assert_eq!(orphan, 0);
    }

    #[tokio::test]
    async fn visible_count_filters_children_loops_and_deleted() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-count").await;
        let first = seed_conversation(&db, folder_id, AgentType::Codex).await;
        seed_conversation(&db, folder_id, AgentType::Codex).await;
        crate::db::service::conversation_service::create_with_delegation(
            &db.conn,
            folder_id,
            AgentType::Codex,
            Some("child".into()),
            None,
            Some(crate::acp::delegation::spawner::DelegationLink {
                parent_conversation_id: first,
                parent_tool_use_id: "tu".into(),
                delegation_call_id: "call".into(),
            }),
        )
        .await
        .expect("child");

        let count = visible_conversation_count(&db.conn, Some(vec![folder_id]), None)
            .await
            .expect("count");
        assert_eq!(count, 2);
    }
    #[tokio::test]
    async fn upgrade_schema_marks_current_version() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-upgrade").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let state = ensure_search_state(&db.conn).await.expect("state");
        let mut active: search_index_state::ActiveModel = state.into();
        active.schema_version = Set(2);
        active.update(&db.conn).await.expect("downgrade version");

        let upgraded = upgrade_schema_if_needed(&db.conn).await.expect("upgrade");
        assert!(upgraded);
        let state = get_search_state(&db.conn).await.expect("state");
        assert_eq!(state.schema_version, SEARCH_SCHEMA_VERSION);
        assert!(!upgrade_schema_if_needed(&db.conn).await.expect("again"));
        let _ = conversation_id;
    }

    #[tokio::test]
    async fn orphan_documents_are_reclaimed() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-orphan").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        ensure_search_state(&db.conn).await.expect("state");
        upsert_document(&db.conn, conversation_id, &doc("orphan text"), None, 1, SyncFlags::default())
            .await
            .expect("doc");
        crate::db::service::conversation_service::soft_delete(&db.conn, conversation_id)
            .await
            .expect("delete");

        let removed = delete_orphan_documents(&db.conn).await.expect("sweep");
        assert_eq!(removed, 1);
        let count = message_search_document::Entity::find().count(&db.conn).await.expect("count");
        assert_eq!(count, 0);
    }
}
