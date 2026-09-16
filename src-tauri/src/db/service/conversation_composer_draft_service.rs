use std::path::{Path, PathBuf};

use chrono::Utc;
use sea_orm::sea_query::OnConflict;
use sea_orm::{DatabaseConnection, EntityTrait, Set};
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::db::entities::conversation_composer_draft;
use crate::db::error::DbError;
use crate::db::service::conversation_service;
use crate::paths::{codeg_uploads_root, simplify_verbatim_path};

/// Same ceiling as `UPLOAD_MAX_BYTES` in the upload handler. Kept local so
/// the draft table does not import the web layer.
const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// Hard cap on persisted composer text. Matches a generous chat box, not a
/// file upload. Enforced on UTF-8 byte length so a crafted payload cannot
/// grow the SQLite row without bound.
pub const MAX_DRAFT_BYTES: usize = 256 * 1024;
/// Client origin id (uuid-ish). Stored + echoed on the WS notify; never a
/// secret, just long enough to ignore our own broadcast.
pub const MAX_ORIGIN_LEN: usize = 64;
/// Staged files on one draft. Matches the iOS chip cap and keeps the
/// metadata JSON small.
pub const MAX_DRAFT_ATTACHMENTS: usize = 16;

/// Jail-ref or workspace-link metadata for one staged composer file.
/// Bytes never live here — images are `path`s under the uploads root;
/// workspace files are `file://` links the host agent can already read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposerDraftAttachment {
    pub id: String,
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// Full draft as returned by GET. `text` is the composer's visible string
/// (not a Tiptap document) so mobile and desktop share one representation.
/// `attachments` is metadata only.
#[derive(Debug, Clone, Serialize)]
pub struct ComposerDraft {
    pub conversation_id: i32,
    pub text: String,
    pub revision: i64,
    pub origin: String,
    pub attachments: Vec<ComposerDraftAttachment>,
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

fn parse_attachments_json(raw: &str) -> Vec<ComposerDraftAttachment> {
    if raw.trim().is_empty() || raw.trim() == "[]" {
        return Vec::new();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

fn encode_attachments(items: &[ComposerDraftAttachment]) -> Result<String, AppCommandError> {
    serde_json::to_string(items).map_err(|e| {
        AppCommandError::invalid_input("failed to encode composer draft attachments")
            .with_detail(e.to_string())
    })
}

fn valid_id(id: &str) -> bool {
    let len = id.len();
    (1..=80).contains(&len)
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'_' | b'-'))
}

fn valid_name(name: &str) -> bool {
    let len = name.chars().count();
    (1..=255).contains(&len)
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

fn valid_mime(mime: &str) -> bool {
    let len = mime.len();
    if !(1..=127).contains(&len) {
        return false;
    }
    let Some((typ, sub)) = mime.split_once('/') else {
        return false;
    };
    !typ.is_empty()
        && !sub.is_empty()
        && typ
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'+')
        && sub
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'+')
}

fn valid_file_uri(uri: &str) -> bool {
    if uri.len() > 2048 || !uri.starts_with("file://") {
        return false;
    }
    // Reject authority-form `file://host/share` (SMB / UNC). Same reason
    // prompt hydration refuses it: canonicalize would connect out.
    let rest = &uri["file://".len()..];
    if rest.starts_with('/') {
        return true;
    }
    rest.starts_with("?/") || rest.is_empty()
}

/// True when `candidate` is a regular file inside `uploads_root`.
pub async fn path_is_inside_uploads(
    candidate: &Path,
    uploads_root: &Path,
) -> Result<PathBuf, AppCommandError> {
    let candidate_canon = tokio::fs::canonicalize(candidate).await.map_err(|_| {
        AppCommandError::invalid_input("draft attachment path is not a readable file")
    })?;
    let root_canon = tokio::fs::canonicalize(uploads_root).await.map_err(|_| {
        AppCommandError::invalid_input("uploads directory is not accessible")
    })?;
    if !candidate_canon.starts_with(&root_canon) {
        return Err(AppCommandError::invalid_input(
            "draft attachment path is outside the uploads directory",
        ));
    }
    let meta = tokio::fs::metadata(&candidate_canon).await.map_err(|_| {
        AppCommandError::invalid_input("draft attachment path is not a readable file")
    })?;
    if !meta.is_file() {
        return Err(AppCommandError::invalid_input(
            "draft attachment path is not a regular file",
        ));
    }
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(AppCommandError::invalid_input(
            "draft attachment exceeds the upload size limit",
        ));
    }
    Ok(simplify_verbatim_path(&candidate_canon))
}

pub async fn validate_attachments(
    items: &[ComposerDraftAttachment],
    uploads_root: &Path,
) -> Result<Vec<ComposerDraftAttachment>, AppCommandError> {
    if items.len() > MAX_DRAFT_ATTACHMENTS {
        return Err(AppCommandError::invalid_input(
            "composer draft has too many attachments",
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if !valid_id(&item.id) {
            return Err(AppCommandError::invalid_input(
                "draft attachment id is invalid",
            ));
        }
        if !seen.insert(item.id.clone()) {
            return Err(AppCommandError::invalid_input(
                "draft attachment ids must be unique",
            ));
        }
        if !valid_name(&item.name) {
            return Err(AppCommandError::invalid_input(
                "draft attachment name is invalid",
            ));
        }
        if let Some(mime) = item.mime.as_deref() {
            if !valid_mime(mime) {
                return Err(AppCommandError::invalid_input(
                    "draft attachment mime type is invalid",
                ));
            }
        }
        if item.size > MAX_ATTACHMENT_BYTES {
            return Err(AppCommandError::invalid_input(
                "draft attachment exceeds the upload size limit",
            ));
        }
        match item.kind.as_str() {
            "image" => {
                let Some(path) = item.path.as_deref().map(str::trim).filter(|s| !s.is_empty())
                else {
                    return Err(AppCommandError::invalid_input(
                        "image draft attachments need an uploads path",
                    ));
                };
                if path.len() > 1024 {
                    return Err(AppCommandError::invalid_input(
                        "draft attachment path is too long",
                    ));
                }
                let checked = path_is_inside_uploads(Path::new(path), uploads_root).await?;
                out.push(ComposerDraftAttachment {
                    id: item.id.clone(),
                    kind: "image".into(),
                    name: item.name.clone(),
                    mime: item.mime.clone(),
                    size: item.size,
                    path: Some(checked.to_string_lossy().into_owned()),
                    uri: None,
                });
            }
            "file" => {
                let Some(uri) = item.uri.as_deref().map(str::trim).filter(|s| !s.is_empty())
                else {
                    return Err(AppCommandError::invalid_input(
                        "file draft attachments need a file:// uri",
                    ));
                };
                if !valid_file_uri(uri) {
                    return Err(AppCommandError::invalid_input(
                        "draft attachment uri is invalid",
                    ));
                }
                out.push(ComposerDraftAttachment {
                    id: item.id.clone(),
                    kind: "file".into(),
                    name: item.name.clone(),
                    mime: item.mime.clone(),
                    size: item.size,
                    path: None,
                    uri: Some(uri.to_string()),
                });
            }
            _ => {
                return Err(AppCommandError::invalid_input(
                    "draft attachment kind must be image or file",
                ));
            }
        }
    }
    Ok(out)
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
        attachments: parse_attachments_json(&row.attachments),
    }))
}

/// Last-write-wins upsert. Every accepted PUT increments `revision` so
/// clients can ignore their own echo and apply only newer remote revisions.
/// An empty `text` still writes a row (a tombstone) so a clear on one
/// device can wipe the other.
///
/// `attachments` is optional on purpose: an older client that only sends
/// text must not wipe images/files the other side already staged. `None`
/// keeps the stored list. `Some` (including empty) replaces it.
pub async fn put(
    conn: &DatabaseConnection,
    conversation_id: i32,
    text: String,
    origin: String,
    attachments: Option<Vec<ComposerDraftAttachment>>,
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
    let next_revision = current
        .as_ref()
        .map(|row| row.revision + 1)
        .unwrap_or(1);
    let kept_attachments = current
        .as_ref()
        .map(|row| row.attachments.clone())
        .unwrap_or_else(|| "[]".into());
    let stored_attachments = if let Some(items) = attachments {
        let checked = validate_attachments(&items, &codeg_uploads_root()).await?;
        encode_attachments(&checked)?
    } else {
        kept_attachments
    };
    let attachments_empty = parse_attachments_json(&stored_attachments).is_empty();
    let cleared = text.is_empty() && attachments_empty;
    let now = Utc::now();
    let model = conversation_composer_draft::ActiveModel {
        conversation_id: Set(conversation_id),
        text: Set(text),
        revision: Set(next_revision),
        origin: Set(origin.clone()),
        attachments: Set(stored_attachments),
        updated_at: Set(now),
    };
    conversation_composer_draft::Entity::insert(model)
        .on_conflict(
            OnConflict::column(conversation_composer_draft::Column::ConversationId)
                .update_columns([
                    conversation_composer_draft::Column::Text,
                    conversation_composer_draft::Column::Revision,
                    conversation_composer_draft::Column::Origin,
                    conversation_composer_draft::Column::Attachments,
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

    fn file_att(id: &str, name: &str, uri: &str) -> ComposerDraftAttachment {
        ComposerDraftAttachment {
            id: id.into(),
            kind: "file".into(),
            name: name.into(),
            mime: Some("text/plain".into()),
            size: 0,
            path: None,
            uri: Some(uri.into()),
        }
    }

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

    #[test]
    fn file_uri_rejects_unc_authority() {
        assert!(valid_file_uri("file:///C:/notes.md"));
        assert!(valid_file_uri("file:///tmp/notes.md"));
        assert!(!valid_file_uri("file://host/share/notes.md"));
        assert!(!valid_file_uri("https://evil.example/x"));
    }

    #[tokio::test]
    async fn put_get_round_trip_and_clear() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/draft-sync").await;
        let id = seed_conversation(&db, folder, AgentType::Grok).await;

        assert!(get(&db.conn, id).await.unwrap().is_none());

        let first = put(
            &db.conn,
            id,
            "hello from desktop".into(),
            "desk1".into(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(first.revision, 1);
        assert!(!first.cleared);

        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert_eq!(loaded.text, "hello from desktop");
        assert_eq!(loaded.origin, "desk1");
        assert_eq!(loaded.revision, 1);
        assert!(loaded.attachments.is_empty());

        let second = put(
            &db.conn,
            id,
            "hello from phone".into(),
            "phone1".into(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(second.revision, 2);
        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert_eq!(loaded.text, "hello from phone");
        assert_eq!(loaded.origin, "phone1");

        let cleared = put(&db.conn, id, String::new(), "phone1".into(), Some(Vec::new()))
            .await
            .unwrap();
        assert!(cleared.cleared);
        assert_eq!(cleared.revision, 3);
        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert_eq!(loaded.text, "");
        assert!(loaded.attachments.is_empty());
    }

    #[tokio::test]
    async fn omit_attachments_does_not_wipe() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/draft-att").await;
        let id = seed_conversation(&db, folder, AgentType::Grok).await;

        let file = file_att("file:notes", "notes.md", "file:///tmp/notes.md");
        put(
            &db.conn,
            id,
            "see notes".into(),
            "desk1".into(),
            Some(vec![file.clone()]),
        )
        .await
        .unwrap();

        // Old client: text only. The file chip must survive.
        put(&db.conn, id, "see notes, edited".into(), "phone1".into(), None)
            .await
            .unwrap();
        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert_eq!(loaded.text, "see notes, edited");
        assert_eq!(loaded.attachments, vec![file]);

        // New client sending [] is an explicit clear.
        put(
            &db.conn,
            id,
            "cleared chips".into(),
            "desk1".into(),
            Some(Vec::new()),
        )
        .await
        .unwrap();
        let loaded = get(&db.conn, id).await.unwrap().unwrap();
        assert!(loaded.attachments.is_empty());
        assert_eq!(loaded.text, "cleared chips");
    }

    #[tokio::test]
    async fn rejects_too_many_and_bad_kind() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/draft-att-bad").await;
        let id = seed_conversation(&db, folder, AgentType::Grok).await;

        let too_many: Vec<_> = (0..MAX_DRAFT_ATTACHMENTS + 1)
            .map(|i| file_att(&format!("file:{i}"), "n.md", "file:///tmp/n.md"))
            .collect();
        assert!(put(&db.conn, id, "x".into(), "desk1".into(), Some(too_many))
            .await
            .is_err());

        let bad = ComposerDraftAttachment {
            id: "x".into(),
            kind: "blob".into(),
            name: "x.bin".into(),
            mime: None,
            size: 0,
            path: None,
            uri: Some("file:///tmp/x.bin".into()),
        };
        assert!(put(&db.conn, id, "x".into(), "desk1".into(), Some(vec![bad]))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn image_without_jail_path_is_rejected() {
        let items = [ComposerDraftAttachment {
            id: "image:1".into(),
            kind: "image".into(),
            name: "shot.png".into(),
            mime: Some("image/png".into()),
            size: 10,
            path: None,
            uri: Some("file:///tmp/shot.png".into()),
        }];
        let err = validate_attachments(&items, Path::new("/tmp"))
            .await
            .unwrap_err();
        assert!(
            err.message.contains("uploads path"),
            "got {}",
            err.message
        );
    }

    #[tokio::test]
    async fn put_requires_a_live_conversation() {
        let db = fresh_in_memory_db().await;
        let err = put(&db.conn, 999_999, "x".into(), "desk1".into(), None)
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
