use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Set,
};

use crate::db::entities::conversation::ConversationKind;
use crate::db::entities::{conversation, folder};
use crate::db::error::DbError;
use crate::models::{AgentType, DbConversationSummary};

pub async fn create(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
    git_branch: Option<String>,
) -> Result<conversation::Model, DbError> {
    create_inner(
        conn,
        folder_id,
        agent_type,
        title,
        git_branch,
        None,
        ConversationKind::Regular,
    )
    .await
}

/// Mirror of [`create`] for folderless chat-mode conversations: identical row
/// shape but `kind = 'chat'`, so the sidebar routes the row to its flat "Chat"
/// section. Callers must pair it with the hidden chat folder created in the
/// same flow (`create_chat_conversation_core`).
pub async fn create_chat(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
    git_branch: Option<String>,
) -> Result<conversation::Model, DbError> {
    create_inner(
        conn,
        folder_id,
        agent_type,
        title,
        git_branch,
        None,
        ConversationKind::Chat,
    )
    .await
}

/// Mirror of [`create`] plus optional delegation linkage. Used by the
/// multi-agent broker when spawning a child sub-session — populates
/// `parent_id` / `parent_tool_use_id` / `delegation_call_id` so the lifecycle
/// subscriber and frontend can rebuild the parent ↔ child binding without
/// inspecting the live broker state. `kind` follows the invariant
/// `delegate ⟺ parent_id set`.
pub async fn create_with_delegation(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
    git_branch: Option<String>,
    delegation: Option<crate::acp::delegation::spawner::DelegationLink>,
) -> Result<conversation::Model, DbError> {
    let kind = if delegation.is_some() {
        ConversationKind::Delegate
    } else {
        ConversationKind::Regular
    };
    create_inner(conn, folder_id, agent_type, title, git_branch, delegation, kind).await
}

async fn create_inner(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: AgentType,
    title: Option<String>,
    git_branch: Option<String>,
    delegation: Option<crate::acp::delegation::spawner::DelegationLink>,
    kind: ConversationKind,
) -> Result<conversation::Model, DbError> {
    let at_str = serde_json::to_value(agent_type)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let now = Utc::now();
    let (parent_id, parent_tool_use_id, delegation_call_id) = match delegation {
        Some(link) => (
            Some(link.parent_conversation_id),
            Some(link.parent_tool_use_id),
            Some(link.delegation_call_id),
        ),
        None => (None, None, None),
    };
    let model = conversation::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        title: Set(title),
        title_locked: Set(false),
        agent_type: Set(at_str),
        status: Set(conversation::ConversationStatus::InProgress),
        kind: Set(kind),
        model: Set(None),
        git_branch: Set(git_branch),
        external_id: Set(None),
        parent_id: Set(parent_id),
        parent_tool_use_id: Set(parent_tool_use_id),
        delegation_call_id: Set(delegation_call_id),
        message_count: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        deleted_at: Set(None),
        pinned_at: Set(None),
    };
    Ok(model.insert(conn).await?)
}

pub async fn update_status(
    conn: &DatabaseConnection,
    conversation_id: i32,
    status: conversation::ConversationStatus,
) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.status = Set(status);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

/// Conditional status transition (CAS): write `new_status` only if the row's
/// current `status` equals `expected`. Returns `true` when the row was
/// updated. Used by the lifecycle subscriber on disconnect/error so a
/// concurrent user-driven `completed` (or a prior `pending_review` from
/// `TurnComplete`) cannot be silently overwritten.
pub async fn update_status_if(
    conn: &DatabaseConnection,
    conversation_id: i32,
    expected: conversation::ConversationStatus,
    new_status: conversation::ConversationStatus,
) -> Result<bool, DbError> {
    use sea_orm::sea_query::Expr;
    let result = conversation::Entity::update_many()
        .col_expr(conversation::Column::Status, Expr::value(new_status))
        .col_expr(conversation::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(conversation::Column::Id.eq(conversation_id))
        .filter(conversation::Column::Status.eq(expected))
        .exec(conn)
        .await?;
    Ok(result.rows_affected > 0)
}

/// Manual rename: set the title AND lock it. Once locked, the per-turn
/// auto-title backfill ([`refresh_auto_title`]) leaves this row alone, so the
/// user's hand-picked name survives every subsequent session-file parse.
pub async fn update_title(
    conn: &DatabaseConnection,
    conversation_id: i32,
    title: String,
) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.title = Set(Some(title));
    active.title_locked = Set(true);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

/// Auto-derive counterpart to [`update_title`]: write `title` ONLY when the row
/// is not user-locked and the value actually changed. Never sets `title_locked`
/// (the title stays eligible for future auto-refreshes, e.g. when an agent like
/// OpenCode regenerates its own session title) and deliberately does NOT bump
/// `updated_at` — a title backfill is metadata, not user activity, so it must
/// not float the row to the top of a recency-sorted sidebar. Returns `true`
/// when a row was written so the caller can broadcast a sidebar upsert.
///
/// Implemented as a single conditional UPDATE (`... WHERE id = ? AND
/// title_locked = false AND (title IS NULL OR title <> ?)`) so the lock/equality
/// checks and the write are atomic: a manual rename ([`update_title`], which
/// sets `title_locked = true`) that lands between a would-be read and the write
/// can never be clobbered, because the lock predicate is re-evaluated at write
/// time by the database. A non-existent row simply matches nothing (`false`).
pub async fn refresh_auto_title(
    conn: &DatabaseConnection,
    conversation_id: i32,
    title: String,
) -> Result<bool, DbError> {
    use sea_orm::sea_query::Expr;
    let title = title.trim();
    if title.is_empty() {
        return Ok(false);
    }
    let res = conversation::Entity::update_many()
        .col_expr(conversation::Column::Title, Expr::value(title))
        .filter(conversation::Column::Id.eq(conversation_id))
        .filter(conversation::Column::TitleLocked.eq(false))
        .filter(
            sea_orm::Condition::any()
                .add(conversation::Column::Title.is_null())
                .add(conversation::Column::Title.ne(title)),
        )
        .exec(conn)
        .await?;
    Ok(res.rows_affected > 0)
}

/// Pin or unpin a conversation. Sets `pinned_at = now()` when pinning, `NULL`
/// when unpinning. Only the `pinned_at` column is written — `updated_at` is
/// deliberately left untouched (SeaORM updates only the `Set` field), because
/// pinning is a view preference, not conversation activity, and must not float
/// the row to the top of a recency-sorted sidebar (same reasoning as
/// [`refresh_auto_title`]). The sidebar's "Pinned" section orders by `pinned_at`
/// descending, so a freshly pinned conversation jumps to the top.
pub async fn update_pin(
    conn: &DatabaseConnection,
    conversation_id: i32,
    pinned: bool,
) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.pinned_at = Set(pinned.then(Utc::now));
    active.update(conn).await?;
    Ok(())
}

/// Persist the agent session id (`external_id`) for a conversation as a single
/// conditional UPDATE guarded on `deleted_at IS NULL`. A soft-deleted (or
/// missing) row matches nothing and the call is a silent no-op — it returns Ok
/// either way, since every caller treats "no live row" as nothing to do.
///
/// The `deleted_at IS NULL` guard matters because `SessionStarted` writes are
/// not serialized against deletes: a conversation can be soft-deleted while its
/// ACP connection stays live and bound (delete only soft-marks the row; it does
/// not disconnect the agent). A fork in particular emits `SessionStarted{S2}`
/// whose lifecycle handler calls this with the still-bound `conversation_id`;
/// without the guard that late write would re-point a deleted row's session id
/// from S1 to S2 and bump `updated_at`, half-resurrecting an invisible row.
/// Writing `WHERE id = ? AND deleted_at IS NULL` makes such a stale event a
/// no-op. (Fork's own current-row re-point in `persist_fork_outcome` is guarded
/// the same way.)
pub async fn update_external_id(
    conn: &DatabaseConnection,
    conversation_id: i32,
    external_id: String,
) -> Result<(), DbError> {
    use sea_orm::sea_query::Expr;
    conversation::Entity::update_many()
        .col_expr(conversation::Column::ExternalId, Expr::value(external_id))
        .col_expr(conversation::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(conversation::Column::Id.eq(conversation_id))
        .filter(conversation::Column::DeletedAt.is_null())
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn soft_delete(conn: &DatabaseConnection, conversation_id: i32) -> Result<(), DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;
    let mut active: conversation::ActiveModel = conv.into();
    active.deleted_at = Set(Some(Utc::now()));
    active.update(conn).await?;
    Ok(())
}

fn parse_agent_type(s: &str) -> AgentType {
    match serde_json::from_value(serde_json::Value::String(s.to_string())) {
        Ok(at) => at,
        Err(_) => {
            // DB has a value the enum does not recognise (manual edit or removed variant).
            // Fall back to ClaudeCode so the row stays readable, but log so resume-as-wrong-agent
            // regressions are traceable.
            tracing::warn!(
                "[conversation_service] unknown agent_type {s:?} in DB, falling back to ClaudeCode"
            );
            AgentType::ClaudeCode
        }
    }
}

fn conv_to_summary(r: conversation::Model) -> DbConversationSummary {
    let status = serde_json::to_value(&r.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{:?}", r.status));
    DbConversationSummary {
        id: r.id,
        folder_id: r.folder_id,
        title: r.title,
        title_locked: r.title_locked,
        agent_type: parse_agent_type(&r.agent_type),
        status,
        kind: r.kind.clone(),
        model: r.model,
        git_branch: r.git_branch,
        external_id: r.external_id,
        message_count: r.message_count as u32,
        // Pure mapper: `child_count` is backfilled by `fill_child_counts` over
        // the returned set, never queried per-row here.
        child_count: 0,
        created_at: r.created_at,
        updated_at: r.updated_at,
        pinned_at: r.pinned_at,
        parent_id: r.parent_id,
        parent_tool_use_id: r.parent_tool_use_id,
        delegation_call_id: r.delegation_call_id,
    }
}

/// Backfill each summary's `child_count` with its number of direct, non-deleted
/// delegation children using ONE `GROUP BY` aggregate over the whole set (never
/// per-row — no N+1). `child_count > 0` iff `list_children` would return rows
/// (same `parent_id == id AND deleted_at IS NULL` predicate), so the sidebar
/// chevron neither expands to nothing nor hides a real subtree. No-op on an
/// empty slice (avoids an `IN ()`).
async fn fill_child_counts(
    conn: &DatabaseConnection,
    summaries: &mut [DbConversationSummary],
) -> Result<(), DbError> {
    if summaries.is_empty() {
        return Ok(());
    }
    let ids: Vec<i32> = summaries.iter().map(|s| s.id).collect();
    let pairs: Vec<(Option<i32>, i64)> = conversation::Entity::find()
        .select_only()
        .column(conversation::Column::ParentId)
        .column_as(conversation::Column::Id.count(), "cnt")
        .filter(conversation::Column::ParentId.is_in(ids))
        .filter(conversation::Column::DeletedAt.is_null())
        .group_by(conversation::Column::ParentId)
        .into_tuple()
        .all(conn)
        .await?;
    let mut counts: std::collections::HashMap<i32, u32> =
        std::collections::HashMap::with_capacity(pairs.len());
    for (parent_id, cnt) in pairs {
        if let Some(pid) = parent_id {
            counts.insert(pid, cnt.max(0) as u32);
        }
    }
    for s in summaries.iter_mut() {
        s.child_count = counts.get(&s.id).copied().unwrap_or(0);
    }
    Ok(())
}

pub async fn get_by_id(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<DbConversationSummary, DbError> {
    let conv = conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("Conversation not found: {conversation_id}")))?;

    let mut summary = conv_to_summary(conv);
    fill_child_counts(conn, std::slice::from_mut(&mut summary)).await?;
    Ok(summary)
}

/// Look up a child conversation by its `delegation_call_id` (the broker's
/// `task_id`). Returns `Ok(None)` when no row matches — used by the broker's
/// `ChildStatusLookup` DB fallback to recover a delegation task's terminal
/// status after its in-memory result was evicted from the completed-cache.
/// Unlike [`get_by_id`] this never errors hard on "not found": a missing row
/// is a legitimate "unknown task" answer.
pub async fn get_by_delegation_call_id(
    conn: &DatabaseConnection,
    delegation_call_id: &str,
) -> Result<Option<DbConversationSummary>, DbError> {
    let conv = conversation::Entity::find()
        .filter(conversation::Column::DelegationCallId.eq(delegation_call_id))
        .filter(conversation::Column::DeletedAt.is_null())
        .one(conn)
        .await?;
    Ok(conv.map(conv_to_summary))
}

pub async fn list_by_folder(
    conn: &DatabaseConnection,
    folder_id: i32,
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    status: Option<String>,
) -> Result<Vec<DbConversationSummary>, DbError> {
    let mut query = conversation::Entity::find()
        .filter(conversation::Column::FolderId.eq(folder_id))
        .filter(conversation::Column::DeletedAt.is_null());

    // Filter by agent_type
    if let Some(ref at) = agent_type {
        let at_str = serde_json::to_value(at)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        query = query.filter(conversation::Column::AgentType.eq(at_str));
    }

    // Search by title
    if let Some(ref s) = search {
        if !s.is_empty() {
            query = query.filter(conversation::Column::Title.contains(s));
        }
    }

    // Filter by status
    if let Some(ref st) = status {
        if let Ok(status_enum) = serde_json::from_value::<conversation::ConversationStatus>(
            serde_json::Value::String(st.clone()),
        ) {
            query = query.filter(conversation::Column::Status.eq(status_enum));
        }
    }

    // Sort
    query = match sort_by.as_deref() {
        Some("oldest") => query.order_by_asc(conversation::Column::CreatedAt),
        _ => query.order_by_desc(conversation::Column::CreatedAt),
    };

    let rows = query.all(conn).await?;

    let mut summaries: Vec<DbConversationSummary> = rows.into_iter().map(conv_to_summary).collect();
    fill_child_counts(conn, &mut summaries).await?;

    Ok(summaries)
}

/// List conversations across folders. When `folder_ids` is `None`, queries all
/// When `folder_ids` is provided, results are scoped to that set. Otherwise
/// returns conversations across every non-deleted folder (open or not).
///
/// `include_children` controls visibility of delegation sub-sessions. When
/// `false` (the default for the top-level list), rows whose `parent_id` is
/// non-null are filtered out — they belong to their parent's tool-call view,
/// not the workspace conversation list. Rows with `kind = 'loop'` are always
/// excluded — they belong to the loops workbench.
pub async fn list_all(
    conn: &DatabaseConnection,
    folder_ids: Option<Vec<i32>>,
    agent_type: Option<AgentType>,
    search: Option<String>,
    sort_by: Option<String>,
    status: Option<String>,
    include_children: bool,
) -> Result<Vec<DbConversationSummary>, DbError> {
    let mut query = conversation::Entity::find().filter(conversation::Column::DeletedAt.is_null());

    // Loop-engineering runs never surface in the workspace conversation list —
    // their entry point is the loops workbench.
    query = query.filter(conversation::Column::Kind.ne(ConversationKind::Loop));

    if !include_children {
        // Root list: no parent_id, never kind=delegate, and no broker task id.
        // Invariant is parent_id set ⟺ delegate ⟺ delegation_call_id set;
        // all three filters keep a half-migrated / partially-backfilled row out
        // of the workspace list (children only surface via list_children).
        query = query
            .filter(conversation::Column::ParentId.is_null())
            .filter(conversation::Column::Kind.ne(ConversationKind::Delegate))
            .filter(conversation::Column::DelegationCallId.is_null());
    }

    match folder_ids {
        Some(ids) if !ids.is_empty() => {
            query = query.filter(conversation::Column::FolderId.is_in(ids));
        }
        _ => {
            // Exclude conversations whose folder was soft-deleted.
            let active_folder_ids: Vec<i32> = folder::Entity::find()
                .filter(folder::Column::DeletedAt.is_null())
                .all(conn)
                .await?
                .into_iter()
                .map(|m| m.id)
                .collect();
            if active_folder_ids.is_empty() {
                return Ok(Vec::new());
            }
            query = query.filter(conversation::Column::FolderId.is_in(active_folder_ids));
        }
    }

    if let Some(ref at) = agent_type {
        let at_str = serde_json::to_value(at)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        query = query.filter(conversation::Column::AgentType.eq(at_str));
    }

    if let Some(ref s) = search {
        if !s.is_empty() {
            query = query.filter(conversation::Column::Title.contains(s));
        }
    }

    if let Some(ref st) = status {
        if let Ok(status_enum) = serde_json::from_value::<conversation::ConversationStatus>(
            serde_json::Value::String(st.clone()),
        ) {
            query = query.filter(conversation::Column::Status.eq(status_enum));
        }
    }

    query = match sort_by.as_deref() {
        Some("oldest") => query.order_by_asc(conversation::Column::UpdatedAt),
        _ => query.order_by_desc(conversation::Column::UpdatedAt),
    };

    let rows = query.all(conn).await?;
    let mut summaries: Vec<DbConversationSummary> = rows.into_iter().map(conv_to_summary).collect();
    fill_child_counts(conn, &mut summaries).await?;
    Ok(summaries)
}

/// List delegation children of a single parent conversation, newest first
/// (`created_at` DESC), matching the sidebar's newest-on-top ordering so a
/// freshly-spawned sub-agent surfaces right under its parent. The only other
/// consumer (`inject_delegation_meta`) keys these by `parent_tool_use_id` and
/// is order-agnostic. Returns rows where `parent_id == parent_conversation_id`.
/// Soft-deleted children are filtered out so a removed sub-session stays hidden
/// in the parent's tool-call view too.
pub async fn list_children(
    conn: &DatabaseConnection,
    parent_conversation_id: i32,
) -> Result<Vec<DbConversationSummary>, DbError> {
    let rows = conversation::Entity::find()
        .filter(conversation::Column::ParentId.eq(parent_conversation_id))
        .filter(conversation::Column::DeletedAt.is_null())
        .order_by_desc(conversation::Column::CreatedAt)
        // Explicit id tie-break so same-timestamp siblings are deterministic and
        // match the frontend re-sort (which tie-breaks id DESC): the raw fetch
        // snapshot and a live-inserted child then land in the same order.
        .order_by_desc(conversation::Column::Id)
        .all(conn)
        .await?;
    let mut summaries: Vec<DbConversationSummary> = rows.into_iter().map(conv_to_summary).collect();
    fill_child_counts(conn, &mut summaries).await?;
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::spawner::DelegationLink;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};

    /// Build a parent + a delegation child for filter assertions.
    async fn seed_parent_with_child(conn: &DatabaseConnection, folder_id: i32) -> (i32, i32) {
        let parent = create(
            conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("P".into()),
            None,
        )
        .await
        .expect("parent");
        let link = DelegationLink {
            parent_conversation_id: parent.id,
            parent_tool_use_id: "tu-1".into(),
            delegation_call_id: "call-1".into(),
        };
        let child = create_with_delegation(
            conn,
            folder_id,
            AgentType::Codex,
            Some("C".into()),
            None,
            Some(link),
        )
        .await
        .expect("child");
        (parent.id, child.id)
    }

    #[tokio::test]
    async fn list_all_excludes_children_by_default() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-list-children-default").await;
        let (parent, _child) = seed_parent_with_child(&db.conn, folder).await;

        let rows = list_all(&db.conn, None, None, None, None, None, false)
            .await
            .expect("list");
        let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
        assert!(ids.contains(&parent), "parent must remain visible: {ids:?}");
        assert_eq!(
            rows.len(),
            1,
            "expected only the parent, got {} rows: {ids:?}",
            rows.len()
        );
    }

    #[tokio::test]
    async fn list_all_includes_children_when_requested() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-list-children-on").await;
        let (parent, child) = seed_parent_with_child(&db.conn, folder).await;

        let rows = list_all(&db.conn, None, None, None, None, None, true)
            .await
            .expect("list");
        let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
        assert!(
            ids.contains(&parent) && ids.contains(&child),
            "both parent + child must appear when include_children=true, got: {ids:?}",
        );
    }

    #[tokio::test]
    async fn list_children_returns_only_matching_parent() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-list-children-only").await;
        let (parent_a, child_a) = seed_parent_with_child(&db.conn, folder).await;
        let (_parent_b, _child_b) = seed_parent_with_child(&db.conn, folder).await;

        let rows = list_children(&db.conn, parent_a).await.expect("list");
        assert_eq!(
            rows.len(),
            1,
            "expected 1 child of parent_a, got {}",
            rows.len()
        );
        assert_eq!(rows[0].id, child_a);
        assert_eq!(rows[0].parent_id, Some(parent_a));
    }

    #[tokio::test]
    async fn list_children_orders_newest_first() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-list-children-order").await;
        let parent = create(&db.conn, folder, AgentType::ClaudeCode, Some("P".into()), None)
            .await
            .expect("parent");
        // Two children created oldest → newest under the same parent.
        let first = create_with_delegation(
            &db.conn,
            folder,
            AgentType::Codex,
            Some("first".into()),
            None,
            Some(DelegationLink {
                parent_conversation_id: parent.id,
                parent_tool_use_id: "tu-1".into(),
                delegation_call_id: "call-1".into(),
            }),
        )
        .await
        .expect("first child");
        let second = create_with_delegation(
            &db.conn,
            folder,
            AgentType::Codex,
            Some("second".into()),
            None,
            Some(DelegationLink {
                parent_conversation_id: parent.id,
                parent_tool_use_id: "tu-2".into(),
                delegation_call_id: "call-2".into(),
            }),
        )
        .await
        .expect("second child");

        // The sidebar shows sub-sessions newest-first so a freshly-spawned
        // sub-agent surfaces right under its parent.
        let rows = list_children(&db.conn, parent.id).await.expect("list");
        let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![second.id, first.id],
            "newest child must come first"
        );
    }

    #[tokio::test]
    async fn child_count_reflects_direct_children() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-child-count-direct").await;
        let (parent, child) = seed_parent_with_child(&db.conn, folder).await;

        // The root listing carries the parent's direct-child count so the
        // sidebar knows to show a chevron; the leaf child carries 0.
        let roots = list_all(&db.conn, None, None, None, None, None, false)
            .await
            .expect("list");
        let parent_row = roots.iter().find(|r| r.id == parent).expect("parent row");
        assert_eq!(parent_row.child_count, 1, "parent has one delegation child");

        let children = list_children(&db.conn, parent).await.expect("children");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, child);
        assert_eq!(children[0].child_count, 0, "leaf child has no children");
    }

    #[tokio::test]
    async fn child_count_counts_grandchildren_for_nested_chevron() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-child-count-nested").await;
        let (parent, child) = seed_parent_with_child(&db.conn, folder).await;

        // Delegate a grandchild from the child so the child itself becomes
        // expandable one level down.
        let link = DelegationLink {
            parent_conversation_id: child,
            parent_tool_use_id: "tu-2".into(),
            delegation_call_id: "call-2".into(),
        };
        create_with_delegation(
            &db.conn,
            folder,
            AgentType::Codex,
            Some("G".into()),
            None,
            Some(link),
        )
        .await
        .expect("grandchild");

        // list_children(parent) must report the child's OWN child_count (1) so
        // the recursive chevron appears on the nested row.
        let children = list_children(&db.conn, parent).await.expect("children");
        let child_row = children.iter().find(|r| r.id == child).expect("child row");
        assert_eq!(child_row.child_count, 1, "child has one grandchild");
    }

    #[tokio::test]
    async fn child_count_excludes_soft_deleted_children() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-child-count-deleted").await;
        let (parent, child) = seed_parent_with_child(&db.conn, folder).await;

        soft_delete(&db.conn, child).await.expect("soft delete child");

        // A removed sub-session must not keep the parent's chevron alive: the
        // aggregate filters deleted_at IS NULL, matching list_children.
        let roots = list_all(&db.conn, None, None, None, None, None, false)
            .await
            .expect("list");
        let parent_row = roots.iter().find(|r| r.id == parent).expect("parent row");
        assert_eq!(
            parent_row.child_count, 0,
            "soft-deleted child must not be counted"
        );
    }

    #[tokio::test]
    async fn update_pin_sets_and_clears_without_bumping_updated_at() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-update-pin").await;
        let conv = create(&db.conn, folder, AgentType::ClaudeCode, Some("c".into()), None)
            .await
            .expect("create");

        // Freshly created rows are unpinned, and the summary projection carries
        // the field through (conv_to_summary mapping).
        let before = get_by_id(&db.conn, conv.id).await.expect("get before");
        assert!(
            before.pinned_at.is_none(),
            "new conversation must be unpinned"
        );
        let updated_at_before = before.updated_at;

        // Pin → pinned_at populated; updated_at must NOT move (pin is a view
        // preference, not activity).
        update_pin(&db.conn, conv.id, true).await.expect("pin");
        let pinned = get_by_id(&db.conn, conv.id).await.expect("get pinned");
        assert!(pinned.pinned_at.is_some(), "pinned_at must be set after pin");
        assert_eq!(
            pinned.updated_at, updated_at_before,
            "pinning must not bump updated_at"
        );

        // Unpin → pinned_at cleared back to NULL; updated_at still unchanged.
        update_pin(&db.conn, conv.id, false).await.expect("unpin");
        let unpinned = get_by_id(&db.conn, conv.id).await.expect("get unpinned");
        assert!(
            unpinned.pinned_at.is_none(),
            "pinned_at must clear after unpin"
        );
        assert_eq!(
            unpinned.updated_at, updated_at_before,
            "unpinning must not bump updated_at"
        );
    }

    #[tokio::test]
    async fn list_children_excludes_soft_deleted() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-list-children-soft-del").await;
        let (parent, child) = seed_parent_with_child(&db.conn, folder).await;

        soft_delete(&db.conn, child).await.expect("soft delete");

        let rows = list_children(&db.conn, parent).await.expect("list");
        assert!(
            rows.is_empty(),
            "soft-deleted child must not appear: {rows:?}"
        );
    }

    #[tokio::test]
    async fn create_leaves_title_unlocked() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-title-unlocked").await;
        let row = create(&db.conn, folder, AgentType::ClaudeCode, Some("hi".into()), None)
            .await
            .expect("create");
        let summary = get_by_id(&db.conn, row.id).await.expect("get");
        assert!(!summary.title_locked, "new conversation must start unlocked");
    }

    #[tokio::test]
    async fn update_title_locks_the_title() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-title-lock").await;
        let row = create(&db.conn, folder, AgentType::ClaudeCode, None, None)
            .await
            .expect("create");
        update_title(&db.conn, row.id, "My name".into())
            .await
            .expect("rename");
        let summary = get_by_id(&db.conn, row.id).await.expect("get");
        assert_eq!(summary.title.as_deref(), Some("My name"));
        assert!(summary.title_locked, "manual rename must lock the title");
    }

    #[tokio::test]
    async fn update_external_id_skips_soft_deleted_row() {
        // A late/stale `SessionStarted` write — e.g. a fork's SessionStarted{S2}
        // landing after the user deleted the conversation — must NOT mutate a
        // soft-deleted row. `update_external_id` is guarded on `deleted_at IS
        // NULL`, so it is a silent no-op: the deleted row keeps its old
        // external_id and is never half-resurrected.
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-extid-deleted").await;
        let row = create(
            &db.conn,
            folder,
            AgentType::ClaudeCode,
            Some("Doomed".into()),
            None,
        )
        .await
        .expect("create");
        update_external_id(&db.conn, row.id, "session-S1".into())
            .await
            .expect("seed external_id");
        soft_delete(&db.conn, row.id).await.expect("soft delete");

        // The guarded write must no-op (Ok) without touching the deleted row.
        update_external_id(&db.conn, row.id, "session-S2".into())
            .await
            .expect("a stale SessionStarted write must be a no-op, not an error");

        // Inspect the raw row directly — `get_by_id` filters deleted rows out.
        let raw = conversation::Entity::find_by_id(row.id)
            .one(&db.conn)
            .await
            .expect("query")
            .expect("row still exists (soft-deleted)");
        assert!(raw.deleted_at.is_some(), "row must remain soft-deleted");
        assert_eq!(
            raw.external_id.as_deref(),
            Some("session-S1"),
            "a stale external_id write must not re-point a soft-deleted row"
        );

        // Sanity: the guard is not over-broad — a LIVE row still updates.
        let live = create(
            &db.conn,
            folder,
            AgentType::ClaudeCode,
            Some("Live".into()),
            None,
        )
        .await
        .expect("create live");
        update_external_id(&db.conn, live.id, "session-S9".into())
            .await
            .expect("live update");
        let live_raw = conversation::Entity::find_by_id(live.id)
            .one(&db.conn)
            .await
            .expect("query live")
            .expect("live row");
        assert_eq!(
            live_raw.external_id.as_deref(),
            Some("session-S9"),
            "a live row must still receive its external_id"
        );
    }

    #[tokio::test]
    async fn refresh_auto_title_writes_when_unlocked_and_changed() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-title-auto").await;
        let row = create(
            &db.conn,
            folder,
            AgentType::ClaudeCode,
            Some("old".into()),
            None,
        )
        .await
        .expect("create");

        let wrote = refresh_auto_title(&db.conn, row.id, "fresh".into())
            .await
            .expect("auto");
        assert!(wrote, "an unlocked, changed title must be written");

        let summary = get_by_id(&db.conn, row.id).await.expect("get");
        assert_eq!(summary.title.as_deref(), Some("fresh"));
        assert!(
            !summary.title_locked,
            "auto refresh must NOT lock — the title stays eligible for future refreshes"
        );
    }

    #[tokio::test]
    async fn refresh_auto_title_skips_when_unchanged_or_empty() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-title-auto-skip").await;
        let row = create(
            &db.conn,
            folder,
            AgentType::ClaudeCode,
            Some("same".into()),
            None,
        )
        .await
        .expect("create");

        assert!(
            !refresh_auto_title(&db.conn, row.id, "same".into())
                .await
                .expect("auto-same"),
            "identical title must be a no-op"
        );
        assert!(
            !refresh_auto_title(&db.conn, row.id, String::new())
                .await
                .expect("auto-empty"),
            "empty title must be a no-op"
        );
    }

    #[tokio::test]
    async fn refresh_auto_title_never_clobbers_a_locked_title() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-title-auto-locked").await;
        let row = create(&db.conn, folder, AgentType::ClaudeCode, None, None)
            .await
            .expect("create");
        update_title(&db.conn, row.id, "User pick".into())
            .await
            .expect("rename");

        let wrote = refresh_auto_title(&db.conn, row.id, "parser title".into())
            .await
            .expect("auto");
        assert!(!wrote, "a locked title must never be auto-overwritten");
        let summary = get_by_id(&db.conn, row.id).await.expect("get");
        assert_eq!(summary.title.as_deref(), Some("User pick"));
        assert!(summary.title_locked);
    }

    #[tokio::test]
    async fn refresh_auto_title_does_not_bump_updated_at() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/codeg-title-no-bump").await;
        let row = create(
            &db.conn,
            folder,
            AgentType::ClaudeCode,
            Some("old".into()),
            None,
        )
        .await
        .expect("create");
        let before = row.updated_at;

        let wrote = refresh_auto_title(&db.conn, row.id, "fresh".into())
            .await
            .expect("auto");
        assert!(wrote);

        let summary = get_by_id(&db.conn, row.id).await.expect("get");
        assert_eq!(summary.title.as_deref(), Some("fresh"));
        assert_eq!(
            summary.updated_at, before,
            "auto-title backfill is metadata, not activity — it must not bump updated_at"
        );
    }

    #[tokio::test]
    async fn create_paths_write_expected_kinds() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/kinds").await;

        let regular = create(&db.conn, folder_id, AgentType::ClaudeCode, None, None)
            .await
            .expect("regular");
        assert_eq!(regular.kind, ConversationKind::Regular);

        let chat = create_chat(&db.conn, folder_id, AgentType::ClaudeCode, None, None)
            .await
            .expect("chat");
        assert_eq!(chat.kind, ConversationKind::Chat);

        let child = create_with_delegation(
            &db.conn,
            folder_id,
            AgentType::Codex,
            None,
            None,
            Some(DelegationLink {
                parent_conversation_id: regular.id,
                parent_tool_use_id: "tu-kind".into(),
                delegation_call_id: "call-kind".into(),
            }),
        )
        .await
        .expect("delegate");
        assert_eq!(child.kind, ConversationKind::Delegate);
        assert_eq!(child.parent_id, Some(regular.id));
    }

    #[tokio::test]
    async fn list_all_excludes_loop_kind_rows() {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/loop-filter").await;
        let keep = create(
            &db.conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("keep".into()),
            None,
        )
        .await
        .expect("keep");
        let hide = create(
            &db.conn,
            folder_id,
            AgentType::ClaudeCode,
            Some("hide".into()),
            None,
        )
        .await
        .expect("hide");
        // No public write path mints kind='loop' yet (reserved for the loop
        // engine), so flip the row directly to exercise the filter.
        let mut active: conversation::ActiveModel = hide.into();
        active.kind = Set(ConversationKind::Loop);
        active.update(&db.conn).await.expect("flip kind");

        let rows = list_all(&db.conn, None, None, None, None, None, false)
            .await
            .expect("list");
        assert!(rows.iter().any(|r| r.id == keep.id), "regular row stays");
        assert!(
            !rows.iter().any(|r| r.title.as_deref() == Some("hide")),
            "loop row must be excluded"
        );
    }
}
