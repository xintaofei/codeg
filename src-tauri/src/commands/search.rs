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

pub async fn search_conversations_core(
    conn: &DatabaseConnection,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
    query: String,
    limit: Option<u64>,
) -> Result<Vec<DbConversationSearchResult>, AppCommandError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 200);
    let state = message_search_service::ensure_search_state(conn).await?;
    let query = query.trim().to_string();

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

    let per_term: Vec<HashMap<i32, Option<f64>>> = if effective_mode == MODE_FTS {
        let mut ranked = Vec::with_capacity(terms.len());
        for term in &terms {
            ranked.push(
                index_term_candidates(conn, term, &state, folder_ids.clone(), agent_type).await?,
            );
        }
        ranked
    } else {
        let mut scanned = Vec::with_capacity(terms.len());
        for term in &terms {
            scanned.push(scan_term_candidates(conn, term).await?);
        }
        scanned
    };

    let candidate_ids = intersect_candidate_ids(&per_term);
    if candidate_ids.is_empty() {
        return Ok(title_only_results(title_summaries, &terms, limit as usize));
    }

    let visible_summaries = conversation_service::list_all(
        conn,
        folder_ids.clone(),
        agent_type,
        None,
        None,
        None,
        false,
    )
    .await?;
    let visible: HashMap<i32, _> = visible_summaries
        .into_iter()
        .map(|summary| (summary.id, summary))
        .collect();
    let candidate_set: HashSet<i32> = candidate_ids.iter().copied().collect();
    let candidate_list: Vec<i32> = candidate_ids.into_iter().collect();

    let mut content_results: Vec<(f64, i32)> = Vec::new();
    for conversation_id in &candidate_list {
        if !visible.contains_key(conversation_id) {
            continue;
        }
        let mut worst: f64 = f64::MIN;
        let mut missing = false;
        for term_scores in &per_term {
            let score = if let Some(Some(rank)) = term_scores.get(conversation_id) {
                *rank
            } else {
                missing = true;
                f64::MAX
            };
            worst = worst.max(score);
        }
        if missing || worst == f64::MAX {
            continue;
        }
        content_results.push((worst, *conversation_id));
    }
    content_results.sort_by(|(score_a, _), (score_b, _)| {
        score_a
            .partial_cmp(score_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let result_limit = limit as usize;
    let mut title_results: Vec<DbConversationSearchResult> =
        Vec::with_capacity(result_limit.min(title_summaries.len()));
    let mut title_ids = HashSet::with_capacity(result_limit.min(title_summaries.len()));
    let mut needed_document_ids = HashSet::new();
    for summary in title_summaries.into_iter().take(result_limit) {
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

    let remaining = result_limit.saturating_sub(title_results.len());
    let mut content_picks = Vec::with_capacity(remaining);
    for (_, conversation_id) in content_results {
        if content_picks.len() >= remaining {
            break;
        }
        if title_ids.contains(&conversation_id) {
            continue;
        }
        let Some(summary) = visible.get(&conversation_id) else {
            continue;
        };
        needed_document_ids.insert(conversation_id);
        content_picks.push((conversation_id, summary.clone()));
    }

    let needed_document_list: Vec<i32> = needed_document_ids.into_iter().collect();
    let documents: HashMap<i32, (String, Vec<NormalizedBlockOffset>)> =
        message_search_service::list_documents_by_conversation(conn, &needed_document_list)
            .await?
            .into_iter()
            .map(|(conversation_id, text, blocks)| (conversation_id, (text, blocks)))
            .collect();

    let mut results = Vec::with_capacity(result_limit);
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
            let content_matches = build_content_match_locations(text, blocks, &terms);
            result.matches.extend(content_matches);
            let (snippet_prefix, snippet_match, snippet_suffix) = build_snippet(text, &terms);
            result.snippet_prefix = snippet_prefix;
            result.snippet_match = snippet_match;
            result.snippet_suffix = snippet_suffix;
        }
        results.push(result);
    }

    for (conversation_id, summary) in content_picks {
        let (text, blocks) = documents
            .get(&conversation_id)
            .map(|(text, blocks)| (text.as_str(), blocks.as_slice()))
            .unwrap_or_default();
        let (snippet_prefix, snippet_match, snippet_suffix) = build_snippet(text, &terms);
        let matches = build_content_match_locations(text, blocks, &terms);
        results.push(DbConversationSearchResult {
            summary,
            snippet_prefix,
            snippet_match,
            snippet_suffix,
            matches,
        });
    }

    Ok(results)
}

async fn scan_term_candidates(
    conn: &DatabaseConnection,
    term: &str,
) -> Result<HashMap<i32, Option<f64>>, AppCommandError> {
    let rows = conn
        .query_all(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT conversation_id, instr(lower(text), lower(?)) AS position \
             FROM message_search_document \
             WHERE text LIKE ? ESCAPE '\\'",
            [term.into(), query::like_pattern(term).into()],
        ))
        .await
        .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
    let mut out = HashMap::new();
    for row in rows {
        let conversation_id = row
            .try_get_by_index::<i32>(0)
            .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
        let position = row
            .try_get_by_index::<i64>(1)
            .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
        out.insert(conversation_id, (position > 0).then_some(position as f64));
    }
    Ok(out)
}

async fn index_term_candidates(
    conn: &DatabaseConnection,
    term: &str,
    state: &crate::db::entities::search_index_state::Model,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
) -> Result<HashMap<i32, Option<f64>>, AppCommandError> {
    let char_len = term.chars().count();
    if char_len >= 3 {
        let Some(expression) = query::trigram_expression(term) else {
            return scan_term_candidates(conn, term).await;
        };
        let limit =
            message_search_service::visible_conversation_count(conn, folder_ids, agent_type)
                .await?;
        return query_indexed_term(
            conn,
            "message_search_trigram",
            &expression,
            &query::like_pattern(term),
            limit,
        )
        .await;
    }

    if !state.short_fts_enabled {
        return scan_term_candidates(conn, term).await;
    }
    let expression = match query::short_query(term) {
        ShortTermQuery::CjkUnigram { token } => format!("words : \"{token}\""),
        ShortTermQuery::CjkBigram { phrase } => format!("bigrams : \"{phrase}\""),
        ShortTermQuery::LatinPrefix { token } => format!("words : \"{token}\"*"),
    };
    let exact = if matches!(query::short_query(term), ShortTermQuery::LatinPrefix { .. }) {
        format!("{}%", query::escape_like(term))
    } else {
        query::like_pattern(term)
    };
    let limit =
        message_search_service::visible_conversation_count(conn, folder_ids, agent_type).await?;
    query_indexed_term(conn, "message_search_short", &expression, &exact, limit).await
}

async fn query_indexed_term(
    conn: &DatabaseConnection,
    table: &str,
    expression: &str,
    exact_pattern: &str,
    limit: i64,
) -> Result<HashMap<i32, Option<f64>>, AppCommandError> {
    let sql = format!(
        "SELECT d.conversation_id, bm25({table}) AS rank \
         FROM {table} \
         JOIN message_search_document d ON d.id = {table}.rowid \
         WHERE {table} MATCH ? AND d.text LIKE ? ESCAPE '\\' \
         ORDER BY rank LIMIT ?"
    );
    let rows = conn
        .query_all(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            [
                expression.to_string().into(),
                exact_pattern.to_string().into(),
                limit.into(),
            ],
        ))
        .await
        .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
    let mut out = HashMap::new();
    for row in rows {
        let conversation_id = row
            .try_get_by_index::<i32>(0)
            .map_err(|err| AppCommandError::from(DbError::Database(err)))?;
        let rank = row
            .try_get_by_index::<f64>(1)
            .map_err(|err| AppCommandError::from(DbError::Database(err)))
            .unwrap_or(f64::MAX);
        out.insert(conversation_id, Some(rank));
    }
    Ok(out)
}

fn intersect_candidate_ids(per_term: &[HashMap<i32, Option<f64>>]) -> HashSet<i32> {
    let mut iter = per_term.iter();
    let Some(first) = iter.next() else {
        return HashSet::new();
    };
    let mut ids: HashSet<i32> = first.keys().copied().collect();
    for term in iter {
        ids.retain(|id| term.contains_key(id));
    }
    ids
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
    let progress = if visible > 0 {
        (indexed_count as f64 / visible as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };
    Ok(SearchIndexStatus {
        mode: state.mode.clone(),
        user_enabled: state.user_enabled,
        user_mode: state.user_mode.clone(),
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
    message_search_service::set_search_user_settings(conn, enabled, &user_mode).await?;
    crate::search::indexer::sync_mode_and_progress(
        conn,
        &crate::web::event_bridge::EventEmitter::Noop,
    )
    .await?;
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
    db: tauri::State<'_, crate::db::AppDatabase>,
    enabled: bool,
    user_mode: String,
) -> Result<(), AppCommandError> {
    set_search_settings_core(&db.conn, enabled, user_mode).await
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
}
