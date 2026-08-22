use chrono::Utc;
use sea_orm::sea_query::OnConflict;
use sea_orm::{DatabaseConnection, EntityTrait, Set};
use serde::Serialize;

use crate::app_error::AppCommandError;
use crate::db::entities::conversation_composer_draft;
use crate::db::error::DbError;
use crate::db::service::conversation_service;

/// Hard cap on persisted composer text. Matches a generous chat box, not a
/// file upload. Enforced on UTF-8 byte length so a crafted payload cannot
/// grow the SQLite row without bound.
pub const MAX_DRAFT_BYTES: usize = 256 * 1024;
/// Client origin id (uuid-ish). Stored + echoed on the WS notify; never a
/// secret, just long enough to ignore our own broadcast.
pub const MAX_ORIGIN_LEN: usize = 64;

/// Full draft as returned by GET. `text` is the composer's visible string
/// (not a Tiptap document) so mobile and desktop share one representation.
#[derive(Debug, Clone, Serialize)]
pub struct ComposerDraft {
    pub conversation_id: i32,
    pub text: String,
    pub revision: i64,
    pub origin: String,
}

/// PUT result. Deliberately omits `text` so a log of the response cannot
/// leak the composer body.
#[derive(Debug, Clone, Serialize)]
pub struct ComposerDraftPutResult {
    pub conversation_id: i32,
    pub revision: i64,
    pub origin: String,
    pub cleared: bool,
}

pub fn validate_origin(origin: &str) -> Result<(), AppCommandError> {
    if origin.is_empty() || origin.len() > MAX_ORIGIN_LEN {
        return Err(AppCommandError::invalid_input(
            "origin must be 1-64 characters",
        ));
    }
    if !origin
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppCommandError::invalid_input(
            "origin must be alphanumeric, hyphen, or underscore",
        ));
    }
    Ok(())
}

pub fn validate_text(text: &str) -> Result<(), AppCommandError> {
    if text.len() > MAX_DRAFT_BYTES {
        return Err(AppCommandError::invalid_input(
            "composer draft exceeds 256 KiB",
        ));
    }
    Ok(())
}

pub async fn get(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<Option<ComposerDraft>, DbError> {
    conversation_service::get_by_id(conn, conversation_id).await?;
    let Some(row) = conversation_composer_draft::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(ComposerDraft {
        conversation_id: row.conversation_id,
        text: row.text,
        revision: row.revision,
        origin: row.origin,
    }))
}

/// Last-write-wins upsert. Every accepted PUT increments `revision` so
/// clients can ignore their own echo and apply only newer remote revisions.
/// An empty `text` still writes a row (a tombstone) so a clear on one
/// device can wipe the other.
pub async fn put(
    conn: &DatabaseConnection,
    conversation_id: i32,
    text: String,
    origin: String,
) -> Result<ComposerDraftPutResult, AppCommandError> {
    validate_origin(&origin)?;
    validate_text(&text)?;
    conversation_service::get_by_id(conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?;

    let current = conversation_composer_draft::Entity::find_by_id(conversation_id)
        .one(conn)
        .await
        .map_err(|e| AppCommandError::from(DbError::from(e)))?;
    let next_revision = current.map(|row| row.revision + 1).unwrap_or(1);
    let cleared = text.is_empty();
    let now = Utc::now();
    let model = conversation_composer_draft::ActiveModel {
        conversation_id: Set(conversation_id),
        text: Set(text),
        revision: Set(next_revision),
        origin: Set(origin.clone()),
        updated_at: Set(now),
    };
    conversation_composer_draft::Entity::insert(model)
        .on_conflict(
            OnConflict::column(conversation_composer_draft::Column::ConversationId)
                .update_columns([
                    conversation_composer_draft::Column::Text,
                    conversation_composer_draft::Column::Revision,
                    conversation_composer_draft::Column::Origin,
                    conversation_composer_draft::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(conn)
        .await
        .map_err(|e| AppCommandError::from(DbError::from(e)))?;

    Ok(ComposerDraftPutResult {
        conversation_id,
        revision: next_revision,
        origin,
        cleared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::agent::AgentType;

    #[test]
    fn origin_rejects_empty_long_and_junk() {
        assert!(validate_origin("").is_err());
        assert!(validate_origin(&"a".repeat(65)).is_err());
        assert!(validate_origin("bad origin").is_err());
        assert!(validate_origin("win-abc_1").is_ok());
    }

    #[test]
    fn text_rejects_oversize() {
        assert!(validate_text("ok").is_ok());
        assert!(validate_text(&"x".repeat(MAX_DRAFT_BYTES + 1)).is_err());
    }

    #[tokio::test]
    async fn put_get_round_trip_and_clear() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/draft-sync").await;
        let id = seed_conversation(&db, folder, AgentType::Grok).await;

        assert!(get(&db.conn, id).await.unwrap().is_none());

        let first = put(&db.conn, id, "hello from desktop".into(), "desk1".into())
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        assert!(!first.cleared);

        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert_eq!(loaded.text, "hello from desktop");
        assert_eq!(loaded.origin, "desk1");
        assert_eq!(loaded.revision, 1);

        let second = put(&db.conn, id, "hello from phone".into(), "phone1".into())
            .await
            .unwrap();
        assert_eq!(second.revision, 2);
        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert_eq!(loaded.text, "hello from phone");
        assert_eq!(loaded.origin, "phone1");

        let cleared = put(&db.conn, id, String::new(), "phone1".into())
            .await
            .unwrap();
        assert!(cleared.cleared);
        assert_eq!(cleared.revision, 3);
        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert_eq!(loaded.text, "");
    }

    #[tokio::test]
    async fn put_requires_a_live_conversation() {
        let db = fresh_in_memory_db().await;
        let err = put(&db.conn, 999_999, "x".into(), "desk1".into())
            .await
            .unwrap_err();
        let detail = err.detail.unwrap_or_default();
        assert!(
            detail.contains("not found") || err.message.to_lowercase().contains("database"),
            "missing conversation must fail closed, got {} / {detail}",
            err.message
        );
    }
}
