use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};

use crate::db::entities::conversation;
use crate::db::error::DbError;
use crate::db::service::conversation_service;
use crate::models::{AgentType, ConversationSummary, ImportResult};
use crate::parsers::claude::ClaudeParser;
use crate::parsers::cline::ClineParser;
use crate::parsers::codex::CodexParser;
use crate::parsers::gemini::GeminiParser;
use crate::parsers::hermes::HermesParser;
use crate::parsers::openclaw::OpenClawParser;
use crate::parsers::opencode::OpenCodeParser;
use crate::parsers::{path_eq_for_matching, AgentParser};

/// Import (and refresh the titles of) the local agent sessions under
/// `folder_path`. Returns the tally plus the ids of already-imported
/// conversations whose title was refreshed, so the caller can broadcast a
/// sidebar upsert for each without re-querying.
pub async fn import_local_conversations(
    conn: &DatabaseConnection,
    folder_id: i32,
    folder_path: &str,
) -> Result<(ImportResult, Vec<i32>), DbError> {
    let path = folder_path.to_string();

    // Run parsers in blocking task since they do filesystem I/O
    let summaries = tokio::task::spawn_blocking(move || {
        let parsers: Vec<(AgentType, Box<dyn AgentParser>)> = vec![
            (AgentType::ClaudeCode, Box::new(ClaudeParser::new())),
            (AgentType::Codex, Box::new(CodexParser::new())),
            (AgentType::OpenCode, Box::new(OpenCodeParser::new())),
            (AgentType::Gemini, Box::new(GeminiParser::new())),
            (AgentType::OpenClaw, Box::new(OpenClawParser::new())),
            (AgentType::Cline, Box::new(ClineParser::new())),
            (AgentType::Hermes, Box::new(HermesParser::new())),
        ];

        let mut matched = Vec::new();
        for (at, parser) in &parsers {
            match parser.list_conversations() {
                Ok(convs) => {
                    for c in convs {
                        if c.folder_path
                            .as_deref()
                            .map(|p| path_eq_for_matching(p, path.as_str()))
                            .unwrap_or(false)
                        {
                            matched.push((*at, c));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error listing {} conversations: {}", at, e);
                }
            }
        }
        matched
    })
    .await
    .map_err(|e| DbError::Migration(e.to_string()))?;

    let mut imported = 0u32;
    let mut updated = 0u32;
    let mut skipped = 0u32;
    let mut updated_ids: Vec<i32> = Vec::new();

    for (agent_type, summary) in &summaries {
        match import_one(conn, folder_id, agent_type, summary).await? {
            ImportOutcome::Imported => imported += 1,
            ImportOutcome::Updated(id) => {
                updated += 1;
                updated_ids.push(id);
            }
            ImportOutcome::Skipped => skipped += 1,
        }
    }

    Ok((ImportResult { imported, updated, skipped }, updated_ids))
}

/// Outcome of reconciling a single parsed session against the DB.
#[derive(Debug, PartialEq, Eq)]
enum ImportOutcome {
    /// A new conversation row was inserted.
    Imported,
    /// An already-imported conversation had its auto-title refreshed; carries
    /// the row id so the caller can broadcast a sidebar upsert.
    Updated(i32),
    /// Already imported, title left unchanged (locked, identical, or the parse
    /// produced no title).
    Skipped,
}

/// Insert a brand-new conversation, or — when it already exists — refresh its
/// title from the freshly parsed session file so an AI-generated title that did
/// not exist at first import is adopted. `refresh_auto_title` is a single
/// conditional UPDATE that skips locked or unchanged rows and never bumps
/// `updated_at`, so a re-import neither clobbers a manual rename nor reorders a
/// recency-sorted sidebar. A missing/empty parsed title leaves the existing
/// title intact rather than nulling it.
async fn import_one(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: &AgentType,
    summary: &ConversationSummary,
) -> Result<ImportOutcome, DbError> {
    let at_str = serde_json::to_value(agent_type)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();

    let exists = conversation::Entity::find()
        .filter(conversation::Column::ExternalId.eq(&summary.id))
        .filter(conversation::Column::AgentType.eq(&at_str))
        .one(conn)
        .await?;

    if let Some(existing) = exists {
        // Preserve the original skip for rows the sidebar never shows: a
        // soft-deleted conversation must stay deleted (never resurrected or
        // rewritten), and a delegation child is not a sidebar row (the upsert
        // broadcast suppresses it too, which would also desync the `updated`
        // count). Only a visible root conversation gets its title refreshed.
        if existing.parent_id.is_some() || existing.deleted_at.is_some() {
            return Ok(ImportOutcome::Skipped);
        }
        if let Some(title) = summary
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            if conversation_service::refresh_auto_title(conn, existing.id, title.to_string()).await?
            {
                return Ok(ImportOutcome::Updated(existing.id));
            }
        }
        return Ok(ImportOutcome::Skipped);
    }

    let created_at = summary.started_at;
    let updated_at = summary.ended_at.unwrap_or(created_at);
    let conv = conversation::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        title: Set(summary.title.clone()),
        title_locked: Set(false),
        agent_type: Set(at_str),
        status: Set(conversation::ConversationStatus::Completed),
        model: Set(summary.model.clone()),
        git_branch: Set(summary.git_branch.clone()),
        external_id: Set(Some(summary.id.clone())),
        parent_id: Set(None),
        parent_tool_use_id: Set(None),
        delegation_call_id: Set(None),
        message_count: Set(summary.message_count as i32),
        created_at: Set(created_at),
        updated_at: Set(updated_at),
        deleted_at: Set(None),
        pinned_at: Set(None),
    };
    conv.insert(conn).await?;
    Ok(ImportOutcome::Imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use chrono::Utc;

    fn summary(id: &str, title: Option<&str>) -> ConversationSummary {
        ConversationSummary {
            id: id.to_string(),
            agent_type: AgentType::ClaudeCode,
            folder_path: Some("/tmp/codeg-import".to_string()),
            folder_name: None,
            title: title.map(|t| t.to_string()),
            started_at: Utc::now(),
            ended_at: None,
            message_count: 3,
            model: None,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        }
    }

    async fn find_id(conn: &DatabaseConnection, ext: &str) -> i32 {
        conversation::Entity::find()
            .filter(conversation::Column::ExternalId.eq(ext))
            .one(conn)
            .await
            .expect("query")
            .expect("row exists")
            .id
    }

    #[tokio::test]
    async fn reimport_refreshes_a_changed_title() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-import").await;
        let at = AgentType::ClaudeCode;

        let first = import_one(&db.conn, folder, &at, &summary("ext-1", Some("first prompt")))
            .await
            .expect("import");
        assert_eq!(first, ImportOutcome::Imported);

        let id = find_id(&db.conn, "ext-1").await;
        // The agent generated an AI title only after the first import; a
        // re-import must adopt it.
        let again = import_one(&db.conn, folder, &at, &summary("ext-1", Some("AI Summary")))
            .await
            .expect("re-import");
        assert_eq!(again, ImportOutcome::Updated(id));

        let got = conversation_service::get_by_id(&db.conn, id)
            .await
            .expect("get");
        assert_eq!(got.title.as_deref(), Some("AI Summary"));
        assert!(!got.title_locked, "auto refresh must not lock the title");
    }

    #[tokio::test]
    async fn reimport_skips_an_unchanged_title() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-import-same").await;
        let at = AgentType::ClaudeCode;
        let s = summary("ext-1", Some("same title"));

        assert_eq!(
            import_one(&db.conn, folder, &at, &s).await.expect("import"),
            ImportOutcome::Imported
        );
        assert_eq!(
            import_one(&db.conn, folder, &at, &s)
                .await
                .expect("re-import"),
            ImportOutcome::Skipped
        );
    }

    #[tokio::test]
    async fn reimport_never_clobbers_a_manual_rename() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-import-lock").await;
        let at = AgentType::ClaudeCode;

        import_one(&db.conn, folder, &at, &summary("ext-1", Some("first prompt")))
            .await
            .expect("import");
        let id = find_id(&db.conn, "ext-1").await;
        conversation_service::update_title(&db.conn, id, "User Pick".into())
            .await
            .expect("rename");

        let outcome = import_one(&db.conn, folder, &at, &summary("ext-1", Some("AI Summary")))
            .await
            .expect("re-import");
        assert_eq!(
            outcome,
            ImportOutcome::Skipped,
            "a locked title must not be touched by import"
        );

        let got = conversation_service::get_by_id(&db.conn, id)
            .await
            .expect("get");
        assert_eq!(got.title.as_deref(), Some("User Pick"));
    }

    #[tokio::test]
    async fn reimport_with_no_title_keeps_the_existing_one() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-import-empty").await;
        let at = AgentType::ClaudeCode;

        import_one(&db.conn, folder, &at, &summary("ext-1", Some("kept title")))
            .await
            .expect("import");
        let id = find_id(&db.conn, "ext-1").await;

        // A parse that yields no title (or only whitespace) must not null the
        // existing title.
        assert_eq!(
            import_one(&db.conn, folder, &at, &summary("ext-1", None))
                .await
                .expect("none"),
            ImportOutcome::Skipped
        );
        assert_eq!(
            import_one(&db.conn, folder, &at, &summary("ext-1", Some("   ")))
                .await
                .expect("blank"),
            ImportOutcome::Skipped
        );
        let got = conversation_service::get_by_id(&db.conn, id)
            .await
            .expect("get");
        assert_eq!(got.title.as_deref(), Some("kept title"));
    }

    #[tokio::test]
    async fn reimport_skips_a_soft_deleted_conversation() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-import-deleted").await;
        let at = AgentType::ClaudeCode;

        import_one(&db.conn, folder, &at, &summary("ext-1", Some("original")))
            .await
            .expect("import");
        let id = find_id(&db.conn, "ext-1").await;
        conversation_service::soft_delete(&db.conn, id)
            .await
            .expect("soft delete");

        // A re-import must neither resurrect nor rewrite a deleted conversation.
        let outcome = import_one(&db.conn, folder, &at, &summary("ext-1", Some("AI Summary")))
            .await
            .expect("re-import");
        assert_eq!(outcome, ImportOutcome::Skipped);

        let row = conversation::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .expect("query")
            .expect("row still present");
        assert_eq!(row.title.as_deref(), Some("original"), "title untouched");
        assert!(row.deleted_at.is_some(), "must stay soft-deleted");
    }

    #[tokio::test]
    async fn reimport_skips_a_delegation_child() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-import-child").await;
        let at = AgentType::ClaudeCode;
        let at_str = serde_json::to_value(at)
            .expect("ser")
            .as_str()
            .expect("str")
            .to_string();

        // A root conversation to parent the child.
        import_one(&db.conn, folder, &at, &summary("parent-ext", Some("parent")))
            .await
            .expect("import parent");
        let parent_id = find_id(&db.conn, "parent-ext").await;

        // A delegation child carrying its own external_id, as a parser would
        // surface that child's session file on disk.
        let now = Utc::now();
        conversation::ActiveModel {
            id: NotSet,
            folder_id: Set(folder),
            title: Set(Some("child original".to_string())),
            title_locked: Set(false),
            agent_type: Set(at_str),
            status: Set(conversation::ConversationStatus::Completed),
            model: Set(None),
            git_branch: Set(None),
            external_id: Set(Some("child-ext".to_string())),
            parent_id: Set(Some(parent_id)),
            parent_tool_use_id: Set(None),
            delegation_call_id: Set(None),
            message_count: Set(1),
            created_at: Set(now),
            updated_at: Set(now),
            deleted_at: Set(None),
            pinned_at: Set(None),
        }
        .insert(&db.conn)
        .await
        .expect("insert child");

        let outcome = import_one(&db.conn, folder, &at, &summary("child-ext", Some("AI Summary")))
            .await
            .expect("re-import child");
        assert_eq!(
            outcome,
            ImportOutcome::Skipped,
            "a delegation child is never a sidebar row"
        );

        let child_id = find_id(&db.conn, "child-ext").await;
        let row = conversation::Entity::find_by_id(child_id)
            .one(&db.conn)
            .await
            .expect("query")
            .expect("child present");
        assert_eq!(
            row.title.as_deref(),
            Some("child original"),
            "child title untouched"
        );
    }
}
