use std::collections::{HashMap, HashSet};

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use crate::app_error::AppCommandError;
use crate::db::error::DbError;
use crate::db::service::{
    conversation_service, message_search_service,
    message_search_service::{MODE_FTS, MODE_SCAN, USER_MODE_FTS, USER_MODE_SCAN},
};
use crate::models::{
    AgentType, DbConversationSearchResult, DbConversationSummary, SearchIndexStatus,
    SearchMatchLocation, SearchMatchLocationKind,
};
use crate::search::normalizer::NormalizedBlockOffset;
use crate::search::query::{self, ShortTermQuery};

const DEFAULT_LIMIT: u64 = 50;
const SNIPPET_CONTEXT_CHARS: usize = 80;
const MAX_MATCH_LOCATIONS: usize = 200;
const MAX_QUERY_CHARS: usize = 256;

pub async fn search_conversations_core(
    conn: &DatabaseConnection,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
    query: String,
    limit: Option<u64>,
) -> Result<Vec<DbConversationSearchResult>, AppCommandError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 200);
    let state = message_search_service::ensure_search_state(conn).await?;
    let mut query = query.trim().to_string();
    // A pasted error message is a common content-search query; cap it so the
    // cost stays bounded instead of building one LIKE pattern and gram per
    // character.
    if query.chars().count() > MAX_QUERY_CHARS {
        query = query.chars().take(MAX_QUERY_CHARS).collect();
    }

    let title_summaries = conversation_service::list_all(
        conn,
        folder_ids.clone(),
        agent_type,
        if query.is_empty() {
            None
        } else {
            Some(query.clone())
        },
        None,
        None,
        false,
    )
    .await?;

    let terms = query::split_terms(&query);
    if query.is_empty() || !state.user_enabled {
        return Ok(title_only_results(title_summaries, &terms, limit as usize));
    }

    let effective_mode = match state.user_mode.as_str() {
        USER_MODE_SCAN => MODE_SCAN,
        USER_MODE_FTS => MODE_FTS,
        _ => state.mode.as_str(),
    };
    let scope = Scope::build(folder_ids.clone(), agent_type)?;
    let cap = scope.count(conn).await?;

    let mut candidate_ids: Option<HashSet<i32>> = None;
    let mut updated_at: HashMap<i32, chrono::DateTime<chrono::Utc>> = HashMap::new();
    for term in &terms {
        let rows = term_candidates(conn, term, effective_mode, state.short_fts_enabled, &scope, cap)
            .await?;
        let ids: HashSet<i32> = rows.iter().map(|(id, _)| *id).collect();
        for (id, at) in rows {
            updated_at.entry(id).or_insert(at);
        }
        candidate_ids = Some(match candidate_ids {
            None => ids,
            Some(mut existing) => {
                existing.retain(|id| ids.contains(id));
                existing
            }
        });
    }

    let candidate_set = candidate_ids.unwrap_or_default();
    if candidate_set.is_empty() {
        return Ok(title_only_results(title_summaries, &terms, limit as usize));
    }

    let mut title_results: Vec<DbConversationSearchResult> =
        Vec::with_capacity(limit as usize);
    let mut title_ids = HashSet::new();
    let mut needed_document_ids = HashSet::new();
    for summary in title_summaries.into_iter().take(limit as usize) {
        let summary_id = summary.id;
        title_ids.insert(summary_id);
        if candidate_set.contains(&summary_id) {
            needed_document_ids.insert(summary_id);
        }
        title_results.push(DbConversationSearchResult {
            summary,
            snippet_prefix: None,
            snippet_match: None,
            snippet_suffix: None,
            matches: Vec::new(),
        });
    }

    let remaining = limit as usize - title_results.len();
    let mut content_pick_ids = Vec::with_capacity(remaining);
    let mut candidates_sorted: Vec<i32> = candidate_set.iter().copied().collect();
    candidates_sorted.sort_by(|a, b| {
        updated_at
            .get(b)
            .cmp(&updated_at.get(a))
            .then_with(|| b.cmp(a))
    });
    for id in candidates_sorted {
        if title_ids.contains(&id) {
            continue;
        }
        if content_pick_ids.len() >= remaining {
            break;
        }
        needed_document_ids.insert(id);
        content_pick_ids.push(id);
    }

    let documents: HashMap<i32, (String, Vec<NormalizedBlockOffset>)> =
        message_search_service::list_documents_by_conversation(
            conn,
            &needed_document_ids.iter().copied().collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .map(|(conversation_id, text, blocks)| (conversation_id, (text, blocks)))
        .collect();

    let mut results = Vec::with_capacity(limit as usize);
    for mut result in title_results {
        let title_matches = build_title_match_locations(
            result.summary.title.as_deref().unwrap_or_default(),
            &terms,
        );
        result.matches.extend(title_matches);
        if candidate_set.contains(&result.summary.id) {
            let (text, blocks) = documents
                .get(&result.summary.id)
                .map(|(text, blocks)| (text.as_str(), blocks.as_slice()))
                .unwrap_or_default();
            result.matches
                .extend(build_content_match_locations(text, blocks, &terms));
            let (snippet_prefix, snippet_match, snippet_suffix) = build_snippet(text, &terms);
            result.snippet_prefix = snippet_prefix;
            result.snippet_match = snippet_match;
            result.snippet_suffix = snippet_suffix;
        }
        results.push(result);
    }

    if !content_pick_ids.is_empty() {
        let summaries = conversation_service::list_summaries_by_ids(conn, &content_pick_ids).await?;
        let summary_by_id: HashMap<i32, DbConversationSummary> = summaries
            .into_iter()
            .map(|summary| (summary.id, summary))
            .collect();
        for conversation_id in content_pick_ids {
            let Some(summary) = summary_by_id.get(&conversation_id) else {
                continue;
            };
            let (text, blocks) = documents
                .get(&conversation_id)
                .map(|(text, blocks)| (text.as_str(), blocks.as_slice()))
                .unwrap_or_default();
            let (snippet_prefix, snippet_match, snippet_suffix) = build_snippet(text, &terms);
            let matches = build_content_match_locations(text, blocks, &terms);
            results.push(DbConversationSearchResult {
                summary: summary.clone(),
                snippet_prefix,
                snippet_match,
                snippet_suffix,
                matches,
            });
        }
    }

    Ok(results)
}

/// A visibility scope shared by every term query, so LIMIT is applied after
/// folder / agent / lifecycle filtering instead of over the whole corpus.
struct Scope {
    clause: String,
    params: Vec<sea_orm::Value>,
}

impl Scope {
    fn build(
        folder_ids: Option<Vec<i32>>,
        agent_type: Option<AgentType>,
    ) -> Result<Self, AppCommandError> {
        let mut clause = String::from(
            "c.deleted_at IS NULL AND c.kind != 'loop' AND c.parent_id IS NULL",
        );
        let mut params: Vec<sea_orm::Value> = Vec::new();
        match folder_ids {
            Some(ids) if !ids.is_empty() => {
                let placeholders = vec!["?"; ids.len()].join(", ");
                clause.push_str(&format!(" AND c.folder_id IN ({placeholders})"));
                for id in ids {
                    params.push(id.into());
                }
            }
            _ => {
                clause.push_str(
                    " AND c.folder_id IN (SELECT id FROM folder WHERE deleted_at IS NULL)",
                );
            }
        }
        if let Some(agent_type) = agent_type {
            let agent_str = serde_json::to_value(agent_type)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            clause.push_str(" AND c.agent_type = ?");
            params.push(agent_str.into());
        }
        Ok(Self { clause, params })
    }

    async fn count(&self, conn: &DatabaseConnection) -> Result<i64, AppCommandError> {
        let sql = format!("SELECT COUNT(*) FROM conversation c WHERE {}", self.clause);
        let row = conn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                self.params.clone(),
            ))
            .await
            .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
        let Some(row) = row else {
            return Ok(0);
        };
        row.try_get_by_index::<i64>(0)
            .map_err(|err| AppCommandError::from(DbError::Database(err)))
    }
}

async fn term_candidates(
    conn: &DatabaseConnection,
    term: &str,
    mode: &str,
    short_fts_enabled: bool,
    scope: &Scope,
    cap: i64,
) -> Result<Vec<(i32, chrono::DateTime<chrono::Utc>)>, AppCommandError> {
    if cap <= 0 {
        return Ok(Vec::new());
    }
    let char_len = term.chars().count();
    if mode == MODE_FTS && char_len >= 3 {
        let Some(expression) = query::trigram_expression(term) else {
            return scan_term_candidates(conn, term, scope, cap).await;
        };
        return query_fts_candidates(
            conn,
            "message_search_trigram",
            &expression,
            term,
            scope,
            cap,
        )
        .await;
    }

    if mode == MODE_FTS && short_fts_enabled {
        let expression = match query::short_query(term) {
            ShortTermQuery::CjkUnigram { token } => format!("words : \"{token}\""),
            ShortTermQuery::CjkBigram { phrase } => format!("bigrams : \"{phrase}\""),
            ShortTermQuery::LatinPrefix { token } => format!("words : \"{token}\"*"),
        };
        return query_fts_candidates(conn, "message_search_short", &expression, term, scope, cap)
            .await;
    }

    scan_term_candidates(conn, term, scope, cap).await
}

async fn scan_term_candidates(
    conn: &DatabaseConnection,
    term: &str,
    scope: &Scope,
    cap: i64,
) -> Result<Vec<(i32, chrono::DateTime<chrono::Utc>)>, AppCommandError> {
    let sql = format!(
        "SELECT d.conversation_id, c.updated_at \
         FROM message_search_document d \
         JOIN conversation c ON c.id = d.conversation_id \
         WHERE instr(lower(d.text), lower(?)) > 0 \
           AND d.text LIKE ? ESCAPE '\\' \
           AND {} \
         ORDER BY c.updated_at DESC, d.conversation_id DESC \
         LIMIT ?",
        scope.clause
    );
    let mut params = vec![term.to_string().into(), query::like_pattern(term).into()];
    params.extend(scope.params.clone());
    params.push(cap.into());
    query_candidate_rows(conn, sql, params).await
}

async fn query_fts_candidates(
    conn: &DatabaseConnection,
    table: &str,
    expression: &str,
    term: &str,
    scope: &Scope,
    cap: i64,
) -> Result<Vec<(i32, chrono::DateTime<chrono::Utc>)>, AppCommandError> {
    // SQLite's LIKE only folds ASCII case, but the trigram tokenizer folds
    // Unicode. Keep the cheap SQL exact filter for ASCII terms and verify
    // non-ASCII terms in Rust so `ÉCOLE` stays a valid hit for `éco`.
    let ascii = term.is_ascii();
    let select = if ascii {
        "d.conversation_id, c.updated_at".to_string()
    } else {
        "d.conversation_id, c.updated_at, d.text".to_string()
    };
    let exact = if ascii {
        " AND d.text LIKE ? ESCAPE '\\'"
    } else {
        ""
    };
    let sql = format!(
        "SELECT {select} \
         FROM {table} \
         JOIN message_search_document d ON d.id = {table}.rowid \
         JOIN conversation c ON c.id = d.conversation_id \
         WHERE {table} MATCH ?{exact} AND {scope} \
         ORDER BY c.updated_at DESC, d.conversation_id DESC \
         LIMIT ?",
        scope = scope.clause
    );
    let mut params = vec![expression.to_string().into()];
    if ascii {
        params.push(query::like_pattern(term).into());
    }
    params.extend(scope.params.clone());
    params.push(cap.into());
    if ascii {
        return query_candidate_rows(conn, sql, params).await;
    }
    let rows = query_candidate_rows_with_text(conn, sql, params).await?;
    let lower = term.to_lowercase();
    Ok(rows
        .into_iter()
        .filter(|(_, _, text)| text.to_lowercase().contains(&lower))
        .map(|(id, updated_at, _)| (id, updated_at))
        .collect())
}

async fn query_candidate_rows(
    conn: &DatabaseConnection,
    sql: String,
    params: Vec<sea_orm::Value>,
) -> Result<Vec<(i32, chrono::DateTime<chrono::Utc>)>, AppCommandError> {
    let rows = conn
        .query_all(Statement::from_sql_and_values(DbBackend::Sqlite, sql, params))
        .await
        .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get_by_index::<i32>(0)?,
                row.try_get_by_index::<chrono::DateTime<chrono::Utc>>(1)?,
            ))
        })
        .collect::<Result<Vec<_>, sea_orm::DbErr>>()
        .map_err(|err| AppCommandError::from(DbError::Database(err)))
}

async fn query_candidate_rows_with_text(
    conn: &DatabaseConnection,
    sql: String,
    params: Vec<sea_orm::Value>,
) -> Result<Vec<(i32, chrono::DateTime<chrono::Utc>, String)>, AppCommandError> {
    let rows = conn
        .query_all(Statement::from_sql_and_values(DbBackend::Sqlite, sql, params))
        .await
        .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get_by_index::<i32>(0)?,
                row.try_get_by_index::<chrono::DateTime<chrono::Utc>>(1)?,
                row.try_get_by_index::<String>(2)?,
            ))
        })
        .collect::<Result<Vec<_>, sea_orm::DbErr>>()
        .map_err(|err| AppCommandError::from(DbError::Database(err)))
}

fn find_case_insensitive_char(haystack: &str, needle: &str) -> Option<usize> {
    let hay: Vec<char> = haystack.chars().collect();
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|start| {
        hay[*start..*start + needle.len()]
            .iter()
            .zip(&needle)
            .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
    })
}

fn build_snippet(text: &str, terms: &[String]) -> (Option<String>, Option<String>, Option<String>) {
    let hay: Vec<char> = text.chars().collect();
    let mut best: Option<(usize, usize)> = None;
    for term in terms {
        let needle: Vec<char> = term.chars().collect();
        if let Some(start) = find_case_insensitive_char(text, term) {
            if best.is_none_or(|(best_start, _)| start < best_start) {
                best = Some((start, needle.len()));
            }
        }
    }
    let Some((start, len)) = best else {
        return (None, None, None);
    };
    let prefix_start = start.saturating_sub(SNIPPET_CONTEXT_CHARS);
    let suffix_end = (start + len + SNIPPET_CONTEXT_CHARS).min(hay.len());
    let prefix = hay[prefix_start..start].iter().collect::<String>();
    let matched = hay[start..start + len].iter().collect::<String>();
    let suffix = hay[start + len..suffix_end].iter().collect::<String>();
    (
        (!prefix.is_empty()).then_some(prefix),
        Some(matched),
        (!suffix.is_empty()).then_some(suffix),
    )
}

fn build_title_match_locations(title: &str, terms: &[String]) -> Vec<SearchMatchLocation> {
    find_match_ranges(title, terms, MAX_MATCH_LOCATIONS)
        .into_iter()
        .map(|(char_start, char_end)| SearchMatchLocation {
            kind: SearchMatchLocationKind::Title,
            turn_id: None,
            block_index: None,
            char_start,
            char_end,
        })
        .collect()
}

/// Title-only results for queries with no content hits (or content search
/// disabled): the same shape as a full result, with no snippet fields.
fn title_only_results(
    summaries: Vec<DbConversationSummary>,
    terms: &[String],
    limit: usize,
) -> Vec<DbConversationSearchResult> {
    summaries
        .into_iter()
        .take(limit)
        .map(|summary| {
            let matches = build_title_match_locations(
                summary.title.as_deref().unwrap_or_default(),
                terms,
            );
            DbConversationSearchResult {
                summary,
                snippet_prefix: None,
                snippet_match: None,
                snippet_suffix: None,
                matches,
            }
        })
        .collect()
}

fn build_content_match_locations(
    text: &str,
    blocks: &[NormalizedBlockOffset],
    terms: &[String],
) -> Vec<SearchMatchLocation> {
    find_match_ranges(text, terms, MAX_MATCH_LOCATIONS)
        .into_iter()
        .filter_map(|(char_start, char_end)| {
            let block = blocks
                .iter()
                .find(|block| char_start >= block.start && char_end <= block.end)?;
            Some(SearchMatchLocation {
                kind: SearchMatchLocationKind::Content,
                turn_id: Some(block.turn_id.clone()),
                block_index: Some(block.block_index),
                char_start: char_start - block.start + block.leading_trim,
                char_end: char_end - block.start + block.leading_trim,
            })
        })
        .collect()
}

fn find_match_ranges(text: &str, terms: &[String], max_matches: usize) -> Vec<(usize, usize)> {
    let hay: Vec<char> = text.chars().collect();
    let mut ranges = Vec::new();

    for term in terms {
        let needle: Vec<char> = term.chars().collect();
        if needle.is_empty() || needle.len() > hay.len() {
            continue;
        }
        let mut start = 0;
        while start + needle.len() <= hay.len() {
            let matched = hay[start..start + needle.len()]
                .iter()
                .zip(&needle)
                .all(|(left, right)| left.to_lowercase().eq(right.to_lowercase()));
            if matched {
                ranges.push((start, start + needle.len()));
                start += needle.len();
            } else {
                start += 1;
            }
        }
    }

    ranges.sort_by_key(|(start, end)| (*start, *end));
    let mut deduped: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    let mut last_end = None;
    for (start, end) in ranges {
        if last_end.is_some_and(|last| start < last) {
            continue;
        }
        deduped.push((start, end));
        last_end = Some(end);
        if deduped.len() >= max_matches {
            break;
        }
    }
    deduped
}

pub async fn get_search_index_status_core(
    conn: &DatabaseConnection,
) -> Result<SearchIndexStatus, AppCommandError> {
    let state = message_search_service::ensure_search_state(conn).await?;
    let visible = message_search_service::indexable_conversation_count(conn).await?;
    let indexed_count = message_search_service::get_search_state(conn)
        .await?
        .indexed_conversation_count;
    let progress = if !state.user_enabled || visible <= 0 {
        1.0
    } else {
        (indexed_count as f64 / visible as f64).clamp(0.0, 1.0)
    };
    Ok(SearchIndexStatus {
        mode: state.mode,
        user_enabled: state.user_enabled,
        user_mode: state.user_mode,
        indexed_conversation_count: indexed_count,
        visible_conversation_count: visible,
        building: false,
        progress,
    })
}

pub async fn set_search_settings_core(
    conn: &DatabaseConnection,
    enabled: bool,
    user_mode: String,
) -> Result<(), AppCommandError> {
    // Persist only: rebuilding / clearing the index is deferred to the
    // background worker so the settings request never blocks on a full rebuild.
    message_search_service::set_search_user_settings(conn, enabled, &user_mode).await?;
    Ok(())
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn search_conversations(
    db: tauri::State<'_, crate::db::AppDatabase>,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
    query: String,
    limit: Option<u64>,
) -> Result<Vec<DbConversationSearchResult>, AppCommandError> {
    search_conversations_core(&db.conn, folder_ids, agent_type, query, limit).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_search_index_status(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<SearchIndexStatus, AppCommandError> {
    get_search_index_status_core(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn set_search_settings(
    app: tauri::AppHandle,
    db: tauri::State<'_, crate::db::AppDatabase>,
    enabled: bool,
    user_mode: String,
) -> Result<(), AppCommandError> {
    set_search_settings_core(&db.conn, enabled, user_mode).await?;
    {
        use tauri::Manager;
        if let Some(indexer) =
            app.try_state::<std::sync::Arc<crate::search::indexer::MessageSearchIndexer>>()
        {
            indexer.request_mode_sync();
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::message_search_service::SyncFlags;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::search::normalizer::{NormalizedBlockOffset, NormalizedDocument};

    fn doc(text: &str) -> NormalizedDocument {
        NormalizedDocument {
            text: text.to_string(),
            content_hash: crate::search::normalizer::sha256_hex(text),
            blocks: Vec::new(),
        }
    }

    fn doc_with_block(text: &str, turn_id: &str, block_index: usize) -> NormalizedDocument {
        NormalizedDocument {
            blocks: vec![NormalizedBlockOffset {
                turn_id: turn_id.to_string(),
                block_index,
                start: 0,
                end: text.chars().count(),
                leading_trim: 0,
            }],
            text: text.to_string(),
            content_hash: crate::search::normalizer::sha256_hex(text),
        }
    }

    #[tokio::test]
    async fn scan_search_returns_content_snippet() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-command").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        message_search_service::upsert_document(
            &db.conn,
            conversation_id,
            &doc_with_block("前缀 你好世界 后缀", "turn-1", 0),
            None,
            1,
            SyncFlags::default(),
        )
        .await
        .expect("doc");

        let results = search_conversations_core(
            &db.conn,
            Some(vec![folder_id]),
            None,
            "世界".to_string(),
            Some(10),
        )
        .await
        .expect("search");
        let result = results
            .iter()
            .find(|result| result.summary.id == conversation_id)
            .expect("content result");
        assert_eq!(result.snippet_match.as_deref(), Some("世界"));
        assert!(result.matches.iter().any(|match_location| {
            match_location.kind == SearchMatchLocationKind::Content
                && match_location.turn_id.as_deref() == Some("turn-1")
                && match_location.block_index == Some(0)
                && match_location.char_start == 5
                && match_location.char_end == 7
        }));
    }

    #[tokio::test]
    async fn title_matches_rank_before_content() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-title").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        conversation_service::update_title(&db.conn, conversation_id, "世界标题".to_string())
            .await
            .expect("title");
        message_search_service::upsert_document(
            &db.conn,
            conversation_id,
            &doc_with_block("正文 世界", "turn-1", 0),
            None,
            1,
            SyncFlags::default(),
        )
        .await
        .expect("doc");

        let results = search_conversations_core(
            &db.conn,
            Some(vec![folder_id]),
            None,
            "世界".to_string(),
            Some(10),
        )
        .await
        .expect("search");
        assert_eq!(results[0].summary.id, conversation_id);
        assert_eq!(results[0].snippet_match.as_deref(), Some("世界"));
        assert!(
            results[0]
                .matches
                .iter()
                .any(|match_location| match_location.kind == SearchMatchLocationKind::Title)
        );
        assert!(
            results[0]
                .matches
                .iter()
                .any(|match_location| match_location.kind == SearchMatchLocationKind::Content)
        );
    }

    #[tokio::test]
    async fn fts_mode_uses_trigram_index() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-fts").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn)
            .await
            .expect("state");
        message_search_service::set_search_mode(&db.conn, MODE_FTS, false)
            .await
            .expect("mode");
        message_search_service::upsert_document(
            &db.conn,
            conversation_id,
            &doc("会话记录"),
            None,
            1,
            SyncFlags {
                trigram: true,
                short: false,
            },
        )
        .await
        .expect("doc");

        let results = search_conversations_core(
            &db.conn,
            Some(vec![folder_id]),
            None,
            "会话".to_string(),
            Some(10),
        )
        .await
        .expect("search");
        assert!(
            results
                .iter()
                .any(|result| result.summary.id == conversation_id)
        );
    }

    #[tokio::test]
    async fn search_limit_applies_to_title_and_content_together() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-limit").await;
        for index in 0..5 {
            let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
            conversation_service::update_title(
                &db.conn,
                conversation_id,
                format!("common {index}"),
            )
            .await
            .expect("title");
        }

        let results = search_conversations_core(
            &db.conn,
            Some(vec![folder_id]),
            None,
            "common".to_string(),
            Some(2),
        )
        .await
        .expect("search");
        assert_eq!(results.len(), 2);
    }
    #[tokio::test]
    async fn fts_trigram_path_finds_three_char_hits() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-trigram").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn).await.expect("state");
        message_search_service::set_search_mode(&db.conn, MODE_FTS, false).await.expect("mode");
        message_search_service::upsert_document(
            &db.conn, conversation_id, &doc("会话记录全文"), None, 1,
            SyncFlags { trigram: true, short: false },
        )
        .await
        .expect("doc");
        let results = search_conversations_core(
            &db.conn, Some(vec![folder_id]), None, "会话记录".to_string(), Some(10),
        )
        .await
        .expect("search");
        assert!(results.iter().any(|r| r.summary.id == conversation_id));
    }

    #[tokio::test]
    async fn fts_short_table_supports_cjk_bigram_queries() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-short").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn).await.expect("state");
        message_search_service::set_search_mode(&db.conn, MODE_FTS, true).await.expect("mode");
        message_search_service::upsert_document(
            &db.conn, conversation_id, &doc("搜索聊天记录"), None, 1,
            SyncFlags { trigram: true, short: true },
        )
        .await
        .expect("doc");
        let results = search_conversations_core(
            &db.conn, Some(vec![folder_id]), None, "聊天".to_string(), Some(10),
        )
        .await
        .expect("search");
        assert!(results.iter().any(|r| r.summary.id == conversation_id));
    }

    #[tokio::test]
    async fn folder_scope_filters_content_candidates() {
        let db = fresh_in_memory_db().await;
        let first = seed_folder(&db, "/tmp/search-folder-a").await;
        let second = seed_folder(&db, "/tmp/search-folder-b").await;
        let in_scope = seed_conversation(&db, first, AgentType::Codex).await;
        let out_of_scope = seed_conversation(&db, second, AgentType::Codex).await;
        for (conversation_id, text) in [(in_scope, "needle alpha"), (out_of_scope, "needle beta")] {
            message_search_service::upsert_document(
                &db.conn, conversation_id, &doc(text), None, 1, SyncFlags::default(),
            )
            .await
            .expect("doc");
        }
        let results = search_conversations_core(
            &db.conn, Some(vec![first]), None, "needle".to_string(), Some(10),
        )
        .await
        .expect("search");
        assert!(results.iter().any(|r| r.summary.id == in_scope));
        assert!(!results.iter().any(|r| r.summary.id == out_of_scope));
    }

    #[tokio::test]
    async fn content_results_are_ordered_by_recent_activity() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-order").await;
        let older = seed_conversation(&db, folder_id, AgentType::Codex).await;
        let newer = seed_conversation(&db, folder_id, AgentType::Codex).await;
        for (conversation_id, at) in [
            (older, chrono::Utc::now() - chrono::Duration::hours(2)),
            (newer, chrono::Utc::now()),
        ] {
            conversation_service::refresh_external_activity(&db.conn, conversation_id, at, 1)
                .await
                .expect("activity");
            message_search_service::upsert_document(
                &db.conn, conversation_id, &doc("common term"), None, 1, SyncFlags::default(),
            )
            .await
            .expect("doc");
        }
        let results = search_conversations_core(
            &db.conn, Some(vec![folder_id]), None, "common".to_string(), Some(10),
        )
        .await
        .expect("search");
        assert_eq!(results[0].summary.id, newer);
        assert_eq!(results[1].summary.id, older);
    }

    #[tokio::test]
    async fn fts_folds_non_ascii_case_for_trigram_terms() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/search-accent").await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        message_search_service::ensure_search_state(&db.conn).await.expect("state");
        message_search_service::set_search_mode(&db.conn, MODE_FTS, true).await.expect("mode");
        message_search_service::upsert_document(
            &db.conn, conversation_id, &doc("ÉCOLE 资料"), None, 1,
            SyncFlags { trigram: true, short: true },
        )
        .await
        .expect("doc");
        let results = search_conversations_core(
            &db.conn, Some(vec![folder_id]), None, "éco".to_string(), Some(10),
        )
        .await
        .expect("search");
        assert!(results.iter().any(|r| r.summary.id == conversation_id));
    }
}