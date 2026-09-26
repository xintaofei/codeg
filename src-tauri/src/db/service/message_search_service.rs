//! Storage for the ⌘K message search: the `message_fts` full-text index over
//! conversation messages, and the `message_fts_state` stamps recording which
//! version of each conversation it holds.
//!
//!   * **write** — [`index_conversation`] swaps one conversation's rows for a
//!     freshly parsed set inside a transaction and stamps the
//!     `conversation.updated_at` they reflect; [`mark_indexed`] drops the rows
//!     and stamps, for a transcript that could not be read. The indexer
//!     driving both lives in `commands::message_search`.
//!   * **read** — [`search_messages`] finds every word of a query anywhere in a
//!     message: words of three or more characters through the trigram index,
//!     shorter ones (common in Chinese: 登录, 修复) with LIKE.
//!
//! Hits join `conversation` and `folder` rather than storing a copy of their
//! fields, so a rename, a soft delete or a removed folder shows up without
//! re-indexing.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement, TransactionTrait, Value,
};
use serde::Serialize;

use crate::db::entities::conversation::{self, ConversationKind};
use crate::db::error::DbError;
use crate::models::message::{ContentBlock, MessageTurn, TurnRole};

/// A turn with more text than this (a pasted log, a dumped file) stays out of
/// the index: its snippets would be noise, and it would dominate the index.
const MAX_TURN_TEXT_BYTES: usize = 100_000;

/// Words shorter than this cannot be looked up in a trigram index, so they are
/// matched with LIKE, which scans the text.
const MIN_INDEXED_WORD_CHARS: usize = 3;

/// Excerpt of a hit found through the index: up to 64 tokens (the most
/// `snippet()` allows) around the best match. A trigram starts at every
/// character, so that is about 64 characters.
const SNIPPET_SQL: &str = "snippet(message_fts, 0, '[[mark]]', '[[/mark]]', '…', 64)";

/// Characters kept on each side of the first match when the excerpt is cut
/// here rather than by `snippet()`.
const EXCERPT_CONTEXT_CHARS: usize = 30;

const MARK_OPEN: &str = "[[mark]]";
const MARK_CLOSE: &str = "[[/mark]]";
const ELLIPSIS: &str = "…";

/// One search hit, as the ⌘K dialog renders it.
#[derive(Debug, Clone, Serialize)]
pub struct MessageSearchHit {
    pub conversation_id: i32,
    pub folder_id: i32,
    pub agent_type: String,
    pub title: Option<String>,
    /// Position of the matching turn in the parsed conversation.
    pub turn_idx: i32,
    /// `user`, `assistant` or `system`.
    pub role: String,
    /// Excerpt around the match with every matched word wrapped in
    /// `[[mark]]…[[/mark]]` — deliberately not HTML, so the frontend decides
    /// how to render the highlight.
    pub snippet: String,
    /// BM25 rank; lower is better. 0 when no word of the query was long enough
    /// for the index — those hits come most recently active first.
    pub rank: f64,
}

/// A conversation the indexer has to (re-)index.
#[derive(Debug, Clone)]
pub struct StaleConversation {
    pub id: i32,
    pub updated_at: DateTime<Utc>,
}

/// The searchable text of one turn: its text blocks, joined by newlines.
/// Reasoning, tool calls and results, and images stay out — the index holds
/// what a reader sees as the conversation's messages.
fn turn_text(turn: &MessageTurn) -> String {
    turn.blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn role_name(role: &TurnRole) -> &'static str {
    match role {
        TurnRole::User => "user",
        TurnRole::Assistant => "assistant",
        TurnRole::System => "system",
    }
}

async fn stamp<C: ConnectionTrait>(
    conn: &C,
    conversation_id: i32,
    source_updated_at: DateTime<Utc>,
) -> Result<(), DbError> {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO message_fts_state (conversation_id, indexed_updated_at, indexed_at) \
         VALUES (?, ?, ?) \
         ON CONFLICT(conversation_id) DO UPDATE SET \
           indexed_updated_at = excluded.indexed_updated_at, \
           indexed_at = excluded.indexed_at",
        [
            conversation_id.into(),
            source_updated_at.to_rfc3339().into(),
            Utc::now().to_rfc3339().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Replace one conversation's rows with `turns` and stamp the
/// `conversation.updated_at` they were parsed at, in one transaction: a search
/// never sees the conversation half-written, and the stamp never claims rows
/// that did not land. Returns how many turns were indexed.
pub async fn index_conversation(
    conn: &DatabaseConnection,
    conversation_id: i32,
    source_updated_at: DateTime<Utc>,
    turns: &[MessageTurn],
) -> Result<u32, DbError> {
    let backend = conn.get_database_backend();
    let txn = conn.begin().await?;
    txn.execute(Statement::from_sql_and_values(
        backend,
        "DELETE FROM message_fts WHERE conversation_id = ?",
        [conversation_id.into()],
    ))
    .await?;

    let mut indexed = 0;
    for (idx, turn) in turns.iter().enumerate() {
        let text = turn_text(turn);
        if text.is_empty() || text.len() > MAX_TURN_TEXT_BYTES {
            continue;
        }
        txn.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO message_fts (content, conversation_id, turn_idx, role) \
             VALUES (?, ?, ?, ?)",
            [
                text.into(),
                conversation_id.into(),
                (idx as i32).into(),
                role_name(&turn.role).into(),
            ],
        ))
        .await?;
        indexed += 1;
    }

    stamp(&txn, conversation_id, source_updated_at).await?;
    txn.commit().await?;
    Ok(indexed)
}

/// Stamp a conversation as indexed at `source_updated_at` with no rows — for
/// one whose transcript could not be read. Rows from an earlier, readable
/// version go in the same transaction: search still lists the conversation, so
/// they would keep returning text that can no longer be opened, and the stamp
/// would keep them until the conversation changed. Retrying on every pass
/// would not make it readable; its next real change retries by itself.
pub async fn mark_indexed(
    conn: &DatabaseConnection,
    conversation_id: i32,
    source_updated_at: DateTime<Utc>,
) -> Result<(), DbError> {
    index_conversation(conn, conversation_id, source_updated_at, &[]).await?;
    Ok(())
}

/// Up to `limit` conversations that were never indexed or changed since, most
/// recently active first. Only conversations a search can return are listed
/// (top-level, not a loop run, not deleted), and one touched at or after
/// `settled_before` is left for a later pass.
pub async fn list_stale_conversations(
    conn: &DatabaseConnection,
    settled_before: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<StaleConversation>, DbError> {
    let indexed: HashMap<i32, String> = conn
        .query_all(Statement::from_string(
            conn.get_database_backend(),
            "SELECT conversation_id, indexed_updated_at FROM message_fts_state",
        ))
        .await?
        .into_iter()
        .filter_map(|row| {
            Some((
                row.try_get_by_index::<i32>(0).ok()?,
                row.try_get_by_index::<String>(1).ok()?,
            ))
        })
        .collect();

    let candidates: Vec<(i32, DateTime<Utc>)> = conversation::Entity::find()
        .select_only()
        .column(conversation::Column::Id)
        .column(conversation::Column::UpdatedAt)
        .filter(conversation::Column::DeletedAt.is_null())
        .filter(conversation::Column::ParentId.is_null())
        .filter(conversation::Column::Kind.ne(ConversationKind::Loop))
        .filter(conversation::Column::UpdatedAt.lt(settled_before))
        .order_by_desc(conversation::Column::UpdatedAt)
        .into_tuple()
        .all(conn)
        .await?;

    Ok(candidates
        .into_iter()
        .filter(|(id, updated_at)| indexed.get(id) != Some(&updated_at.to_rfc3339()))
        .take(limit)
        .map(|(id, updated_at)| StaleConversation { id, updated_at })
        .collect())
}

/// The whitespace-separated words of a query, split by how they are matched.
/// Every word has to appear in the message, anywhere, in any order.
struct QueryWords<'q> {
    /// Long enough for the trigram index.
    indexed: Vec<&'q str>,
    /// Too short for it: matched with LIKE.
    short: Vec<&'q str>,
}

impl<'q> QueryWords<'q> {
    fn parse(query: &'q str) -> Self {
        let (indexed, short) = query
            .split_whitespace()
            .partition(|word| word.chars().count() >= MIN_INDEXED_WORD_CHARS);
        Self { indexed, short }
    }
}

/// FTS5 query requiring every indexed word. Each word is a quoted string, so
/// operators and punctuation (`OR`, `-`, `*`, `:`, `"`) are matched as text
/// instead of failing to parse.
fn match_expression(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// LIKE pattern matching `word` anywhere, its own `%`, `_` and `\` taken
/// literally (the query declares `ESCAPE '\'`).
fn like_pattern(word: &str) -> String {
    let mut pattern = String::with_capacity(word.len() + 2);
    pattern.push('%');
    for c in word.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

/// Best `limit` hits for `query` across every conversation the sidebar lists:
/// top-level, not a loop run, not deleted, in a folder that is not removed.
///
/// Hits found through the index are ranked by BM25. A query whose words are
/// all too short for the index scans the text instead, and its hits come most
/// recently active first.
pub async fn search_messages(
    conn: &DatabaseConnection,
    query: &str,
    limit: u32,
) -> Result<Vec<MessageSearchHit>, DbError> {
    let words = QueryWords::parse(query);
    let by_index = !words.indexed.is_empty();
    if !by_index && words.short.is_empty() {
        return Ok(Vec::new());
    }

    let mut conditions = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    if by_index {
        conditions.push("message_fts MATCH ?");
        values.push(match_expression(&words.indexed).into());
    }
    for word in &words.short {
        conditions.push("f.content LIKE ? ESCAPE '\\'");
        values.push(like_pattern(word).into());
    }
    values.push(i64::from(limit).into());

    // Without a MATCH there is neither `snippet()` nor a rank: the whole text
    // comes back, and the excerpt is cut from it below.
    let (text, rank, order) = if by_index {
        (SNIPPET_SQL, "rank", "rank")
    } else {
        ("f.content", "0.0", "c.updated_at DESC, f.turn_idx DESC")
    };
    let conditions = conditions.join(" AND ");
    let sql = format!(
        "SELECT c.id, c.folder_id, c.agent_type, c.title, f.turn_idx, f.role, {text}, {rank} \
         FROM message_fts f \
         JOIN conversation c ON c.id = f.conversation_id \
         JOIN folder fo ON fo.id = c.folder_id \
         WHERE {conditions} \
           AND c.deleted_at IS NULL \
           AND c.parent_id IS NULL \
           AND c.kind <> 'loop' \
           AND fo.deleted_at IS NULL \
         ORDER BY {order} \
         LIMIT ?"
    );
    let rows = conn
        .query_all(Statement::from_sql_and_values(
            conn.get_database_backend(),
            sql,
            values,
        ))
        .await?;

    let short_words = fold_words(&words.short);
    let mut hits = Vec::with_capacity(rows.len());
    for row in rows {
        let text: String = row.try_get_by_index(6)?;
        hits.push(MessageSearchHit {
            conversation_id: row.try_get_by_index(0)?,
            folder_id: row.try_get_by_index(1)?,
            agent_type: row.try_get_by_index(2)?,
            title: row.try_get_by_index(3)?,
            turn_idx: row.try_get_by_index(4)?,
            role: row.try_get_by_index(5)?,
            // `snippet()` marks only what the index matched.
            snippet: if by_index {
                mark_outside_marks(&text, &short_words)
            } else {
                excerpt(&text, &short_words)
            },
            rank: row.try_get_by_index(7)?,
        });
    }
    Ok(hits)
}

fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Words as lower-cased characters, for [`word_ranges`].
fn fold_words(words: &[&str]) -> Vec<Vec<char>> {
    words
        .iter()
        .map(|word| word.chars().map(fold_char).collect())
        .collect()
}

/// Char ranges of the case-insensitive occurrences of `words` in `text`, in
/// order and without overlaps; where several words start at the same place,
/// the longest wins.
fn word_ranges(text: &[char], words: &[Vec<char>]) -> Vec<(usize, usize)> {
    let folded: Vec<char> = text.iter().copied().map(fold_char).collect();
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < folded.len() {
        let longest = words
            .iter()
            .filter(|word| !word.is_empty() && folded[at..].starts_with(word))
            .map(Vec::len)
            .max();
        match longest {
            Some(len) => {
                ranges.push((at, at + len));
                at += len;
            }
            None => at += 1,
        }
    }
    ranges
}

/// `text` with each of `ranges` wrapped in mark tags.
fn mark_ranges(text: &[char], ranges: &[(usize, usize)]) -> String {
    let mut out = String::with_capacity(text.len() + ranges.len() * 17);
    let mut at = 0;
    for &(start, end) in ranges {
        out.extend(&text[at..start]);
        out.push_str(MARK_OPEN);
        out.extend(&text[start..end]);
        out.push_str(MARK_CLOSE);
        at = end;
    }
    out.extend(&text[at..]);
    out
}

/// Excerpt for a hit found without the index: the text around the first
/// occurrence of a word, cut on character boundaries, with every occurrence in
/// it marked the way `snippet()` marks indexed matches.
fn excerpt(text: &str, words: &[Vec<char>]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let ranges = word_ranges(&chars, words);
    let (first_start, first_end) = ranges.first().copied().unwrap_or((0, 0));
    let start = first_start.saturating_sub(EXCERPT_CONTEXT_CHARS);
    let end = (first_end + EXCERPT_CONTEXT_CHARS).min(chars.len());
    let inside: Vec<(usize, usize)> = ranges
        .iter()
        .filter(|&&(from, to)| from >= start && to <= end)
        .map(|&(from, to)| (from - start, to - start))
        .collect();

    let mut out = String::new();
    if start > 0 {
        out.push_str(ELLIPSIS);
    }
    out.push_str(&mark_ranges(&chars[start..end], &inside));
    if end < chars.len() {
        out.push_str(ELLIPSIS);
    }
    out
}

/// `snippet` with `words` marked as well, outside the marks FTS5 placed around
/// what the index matched.
fn mark_outside_marks(snippet: &str, words: &[Vec<char>]) -> String {
    if words.is_empty() {
        return snippet.to_owned();
    }
    let mut out = String::with_capacity(snippet.len());
    let mut rest = snippet;
    while !rest.is_empty() {
        let open = rest.find(MARK_OPEN).unwrap_or(rest.len());
        let close = rest[open..]
            .find(MARK_CLOSE)
            .map_or(rest.len(), |at| open + at + MARK_CLOSE.len());
        let plain: Vec<char> = rest[..open].chars().collect();
        out.push_str(&mark_ranges(&plain, &word_ranges(&plain, words)));
        out.push_str(&rest[open..close]);
        rest = &rest[close..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::spawner::DelegationLink;
    use crate::db::service::{conversation_service, folder_service};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
    use crate::models::agent::AgentType;

    fn turn(role: &str, text: &str) -> MessageTurn {
        serde_json::from_value(serde_json::json!({
            "id": format!("t-{text}"),
            "role": role,
            "blocks": [{ "type": "text", "text": text }],
            "timestamp": "2026-09-24T10:00:00Z",
        }))
        .expect("turn fixture")
    }

    async fn search(conn: &DatabaseConnection, query: &str) -> Vec<MessageSearchHit> {
        search_messages(conn, query, 10).await.expect("search")
    }

    fn ids(hits: &[MessageSearchHit]) -> Vec<i32> {
        hits.iter().map(|h| h.conversation_id).collect()
    }

    /// One indexed conversation per text, each with a single user turn.
    async fn seed_texts(db: &crate::db::AppDatabase, texts: &[&str]) -> Vec<i32> {
        let folder = seed_folder(db, "/tmp/fts-proj").await;
        let mut ids = Vec::new();
        for text in texts {
            let id = seed_conversation(db, folder, AgentType::OpenCode).await;
            index_conversation(&db.conn, id, Utc::now(), &[turn("user", text)])
                .await
                .unwrap();
            ids.push(id);
        }
        ids
    }

    #[tokio::test]
    async fn indexed_text_is_found_and_deleted_conversations_are_not() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/fts-proj").await;
        let a = seed_conversation(&db, folder, AgentType::OpenCode).await;
        let b = seed_conversation(&db, folder, AgentType::Codex).await;
        index_conversation(
            &db.conn,
            a,
            Utc::now(),
            &[
                turn("user", "why does the upload retry loop race"),
                turn("assistant", "the retry loop re-enters before the lock"),
            ],
        )
        .await
        .unwrap();
        index_conversation(
            &db.conn,
            b,
            Utc::now(),
            &[turn("user", "unrelated upload talk")],
        )
        .await
        .unwrap();

        let hits = search(&db.conn, "retry loop").await;
        assert_eq!(hits.len(), 2, "both turns of `a` mention both words");
        assert!(hits.iter().all(|h| h.conversation_id == a));
        assert!(hits[0].snippet.contains("[[mark]]"));
        assert_eq!(search(&db.conn, "upload").await.len(), 2);

        conversation_service::soft_delete(&db.conn, a)
            .await
            .unwrap();
        assert!(search(&db.conn, "retry loop").await.is_empty());
        let hits = search(&db.conn, "upload").await;
        assert_eq!(ids(&hits), [b]);
        assert_eq!(hits[0].agent_type, "codex");
        assert_eq!(hits[0].role, "user");
    }

    #[tokio::test]
    async fn words_are_found_inside_words_and_unspaced_text() {
        let db = fresh_in_memory_db().await;
        let seeded = seed_texts(
            &db,
            &[
                "the upload retry loop races",
                "修复登录问题，然后重新部署",
                "ログインの問題を修正しました",
            ],
        )
        .await;
        let (en, zh, ja) = (seeded[0], seeded[1], seeded[2]);

        // Part of a word, in any case, through the index.
        let hits = search(&db.conn, "RETR").await;
        assert_eq!(ids(&hits), [en]);
        assert_eq!(
            hits[0].snippet,
            "the upload [[mark]]retr[[/mark]]y loop races"
        );

        // A two-character Chinese word inside a run of characters: too short
        // for the index, found by LIKE.
        let hits = search(&db.conn, "登录").await;
        assert_eq!(ids(&hits), [zh]);
        assert_eq!(
            hits[0].snippet,
            "修复[[mark]]登录[[/mark]]问题，然后重新部署"
        );

        // A four-character one, through the index.
        let hits = search(&db.conn, "登录问题").await;
        assert_eq!(ids(&hits), [zh]);
        assert_eq!(
            hits[0].snippet,
            "修复[[mark]]登录问题[[/mark]]，然后重新部署"
        );

        // Japanese, both ways.
        let hits = search(&db.conn, "ログイン").await;
        assert_eq!(ids(&hits), [ja]);
        assert_eq!(
            hits[0].snippet,
            "[[mark]]ログイン[[/mark]]の問題を修正しました"
        );
        let hits = search(&db.conn, "問題").await;
        assert_eq!(ids(&hits), [ja]);
        assert_eq!(
            hits[0].snippet,
            "ログインの[[mark]]問題[[/mark]]を修正しました"
        );
    }

    #[tokio::test]
    async fn short_and_long_words_must_all_match() {
        let db = fresh_in_memory_db().await;
        let seeded = seed_texts(
            &db,
            &[
                "先修复登录问题，再修复部署",
                "登录问题仍然存在",
                "the db call hit a timeout",
                "a timeout in the parser",
            ],
        )
        .await;

        let hits = search(&db.conn, "修复 登录问题").await;
        assert_eq!(ids(&hits), [seeded[0]]);
        // The index marks the long word, and the short one is marked too.
        assert_eq!(
            hits[0].snippet,
            "先[[mark]]修复[[/mark]][[mark]]登录问题[[/mark]]，再[[mark]]修复[[/mark]]部署"
        );

        let hits = search(&db.conn, "DB timeout").await;
        assert_eq!(ids(&hits), [seeded[2]]);
        assert_eq!(
            hits[0].snippet,
            "the [[mark]]db[[/mark]] call hit a [[mark]]timeout[[/mark]]"
        );
    }

    #[tokio::test]
    async fn short_words_order_hits_by_recency() {
        let db = fresh_in_memory_db().await;
        let seeded = seed_texts(&db, &["one 登录", "two 登录", "three 登录"]).await;
        // A rename bumps `updated_at`, making the middle one the most recent.
        conversation_service::update_title(&db.conn, seeded[1], "renamed".into())
            .await
            .unwrap();

        let hits = search(&db.conn, "登录").await;
        assert_eq!(ids(&hits), [seeded[1], seeded[2], seeded[0]]);
    }

    #[tokio::test]
    async fn query_text_is_matched_literally() {
        let db = fresh_in_memory_db().await;
        let seeded = seed_texts(
            &db,
            &[
                "fix: parse \"config\" (or not) -> panic*",
                "coverage is at 100%",
                "a_b c\\d",
            ],
        )
        .await;
        let (code, percent, underscore) = (seeded[0], seeded[1], seeded[2]);

        // FTS5 syntax and punctuation are text to find, not operators.
        for query in [
            "\"config\"",
            "(or",
            "not)",
            "->",
            "panic*",
            "fix:",
            "OR",
            "NOT",
            "-",
        ] {
            assert_eq!(
                ids(&search(&db.conn, query).await),
                [code],
                "query {query:?}"
            );
        }
        assert!(search(&db.conn, "->>").await.is_empty());

        // LIKE wildcards in short words match only themselves.
        assert_eq!(ids(&search(&db.conn, "%").await), [percent]);
        assert_eq!(ids(&search(&db.conn, "0%").await), [percent]);
        assert_eq!(ids(&search(&db.conn, "_").await), [underscore]);
        assert_eq!(ids(&search(&db.conn, "\\").await), [underscore]);

        for query in ["", "   "] {
            assert!(search(&db.conn, query).await.is_empty(), "query {query:?}");
        }
    }

    #[tokio::test]
    async fn reindexing_replaces_the_conversation_rows() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/fts-proj").await;
        let id = seed_conversation(&db, folder, AgentType::OpenCode).await;
        index_conversation(&db.conn, id, Utc::now(), &[turn("user", "first draft")])
            .await
            .unwrap();
        index_conversation(&db.conn, id, Utc::now(), &[turn("user", "second draft")])
            .await
            .unwrap();

        assert!(search(&db.conn, "first").await.is_empty());
        assert_eq!(search(&db.conn, "draft").await.len(), 1);
    }

    #[tokio::test]
    async fn hits_are_limited_to_conversations_the_sidebar_lists() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/fts-proj").await;
        let parent = seed_conversation(&db, folder, AgentType::OpenCode).await;
        let child = conversation_service::create_with_delegation(
            &db.conn,
            folder,
            AgentType::Codex,
            None,
            None,
            Some(DelegationLink {
                parent_conversation_id: parent,
                parent_tool_use_id: "tu-1".into(),
                delegation_call_id: "call-1".into(),
            }),
        )
        .await
        .unwrap()
        .id;
        let removed = seed_folder(&db, "/tmp/fts-removed").await;
        let in_removed = seed_conversation(&db, removed, AgentType::OpenCode).await;
        for id in [parent, child, in_removed] {
            index_conversation(&db.conn, id, Utc::now(), &[turn("user", "shared needle")])
                .await
                .unwrap();
        }
        folder_service::soft_delete_folder(&db.conn, removed)
            .await
            .unwrap();

        assert_eq!(ids(&search(&db.conn, "needle").await), [parent]);
        assert_eq!(ids(&search(&db.conn, "ne").await), [parent]);
    }

    #[tokio::test]
    async fn stale_list_follows_the_recorded_stamps() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/fts-proj").await;
        let id = seed_conversation(&db, folder, AgentType::OpenCode).await;
        let row = conversation::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let later = row.updated_at + chrono::Duration::seconds(1);
        let stale = |settled_before| list_stale_conversations(&db.conn, settled_before, 10);

        // Touched at or after the cut-off: not settled yet.
        assert!(stale(row.updated_at).await.unwrap().is_empty());
        // Settled and never indexed.
        let listed = stale(later).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);

        // Indexed (or marked) at its current version: nothing to do.
        mark_indexed(&db.conn, id, row.updated_at).await.unwrap();
        assert!(stale(later).await.unwrap().is_empty());

        // Stamped at an older version: stale again.
        mark_indexed(&db.conn, id, row.updated_at - chrono::Duration::seconds(5))
            .await
            .unwrap();
        assert_eq!(stale(later).await.unwrap().len(), 1);
    }

    #[test]
    fn like_patterns_take_wildcards_literally() {
        assert_eq!(like_pattern("登录"), "%登录%");
        assert_eq!(like_pattern("5%"), "%5\\%%");
        assert_eq!(like_pattern("a_\\"), "%a\\_\\\\%");
    }

    #[test]
    fn excerpt_is_cut_around_the_first_match_with_every_match_marked() {
        let words = fold_words(&["登录"]);
        let text = format!("{}登录然后再登录{}", "前".repeat(40), "后".repeat(40));
        assert_eq!(
            excerpt(&text, &words),
            format!(
                "…{}[[mark]]登录[[/mark]]然后再[[mark]]登录[[/mark]]{}…",
                "前".repeat(30),
                "后".repeat(25)
            )
        );

        // Case-insensitive, keeping the text's own case; short text is whole.
        let words = fold_words(&["db"]);
        assert_eq!(
            excerpt("The DB is down", &words),
            "The [[mark]]DB[[/mark]] is down"
        );
    }

    #[test]
    fn short_words_are_marked_outside_the_index_marks() {
        let words = fold_words(&["修复"]);
        assert_eq!(
            mark_outside_marks("…先修复[[mark]]登录问题[[/mark]]再修复…", &words),
            "…先[[mark]]修复[[/mark]][[mark]]登录问题[[/mark]]再[[mark]]修复[[/mark]]…"
        );
        assert_eq!(
            mark_outside_marks("a [[mark]]b[[/mark]]", &[]),
            "a [[mark]]b[[/mark]]"
        );
    }
}
