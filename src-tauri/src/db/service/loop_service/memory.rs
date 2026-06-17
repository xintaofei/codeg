use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::loop_artifact_revision::ActorKind;
use crate::db::entities::loop_iteration::Stage;
use crate::db::entities::loop_memory::{self, MemoryKind, MemoryStatus, TrustTier};
use crate::db::error::DbError;
use crate::models::loops::LoopMemoryRow;

pub fn to_row(m: loop_memory::Model) -> LoopMemoryRow {
    LoopMemoryRow {
        id: m.id,
        kind: m.kind,
        source: m.source,
        title: m.title,
        summary: m.summary,
        content: m.content,
        trust_tier: m.trust_tier,
        status: m.status,
        superseded_by: m.superseded_by,
        source_issue_id: m.source_issue_id,
        source_artifact_id: m.source_artifact_id,
        produced_by_iteration_id: m.produced_by_iteration_id,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

/// Where an agent-/reflect-produced memory came from. Default = empty (human/UI).
#[derive(Default, Clone, Copy)]
pub struct MemoryProvenance {
    pub source_issue_id: Option<i32>,
    pub source_artifact_id: Option<i32>,
    pub produced_by_iteration_id: Option<i32>,
}

/// Memory kinds injected for a given stage (briefing §4.8 matrix). `constitution`
/// is handled separately by the briefing assembler, so it is never returned here.
fn kinds_for_stage(stage: Stage) -> Vec<MemoryKind> {
    use MemoryKind::*;
    match stage {
        Stage::Triage => vec![Constraint, Preference],
        Stage::Refine | Stage::Design => vec![Constraint, Decision, Preference],
        Stage::Plan => vec![Decision, Constraint],
        Stage::Implement => vec![Pitfall, Preference, Constraint],
        Stage::Review => vec![Constraint, Decision, Preference, Pitfall],
        // finalize summarizes; reuse the review-wide set.
        Stage::Finalize => vec![Constraint, Decision, Preference, Pitfall],
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_memory(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
    kind: MemoryKind,
    source: ActorKind,
    title: &str,
    summary: Option<&str>,
    content: &str,
    trust_tier: TrustTier,
    provenance: MemoryProvenance,
) -> Result<loop_memory::Model, DbError> {
    let now = Utc::now();
    Ok(loop_memory::ActiveModel {
        space_id: Set(space_id),
        kind: Set(kind),
        source: Set(source),
        title: Set(title.to_string()),
        summary: Set(summary.map(str::to_string)),
        content: Set(content.to_string()),
        trust_tier: Set(trust_tier),
        status: Set(MemoryStatus::Active),
        superseded_by: Set(None),
        source_issue_id: Set(provenance.source_issue_id),
        source_artifact_id: Set(provenance.source_artifact_id),
        produced_by_iteration_id: Set(provenance.produced_by_iteration_id),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(conn)
    .await?)
}

pub async fn update_memory(
    conn: &sea_orm::DatabaseConnection,
    id: i32,
    title: &str,
    content: &str,
    status: MemoryStatus,
) -> Result<(), DbError> {
    let row = loop_memory::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| {
            DbError::Database(sea_orm::DbErr::RecordNotFound(format!("loop_memory {id}")))
        })?;
    let mut active = row.into_active_model();
    active.title = Set(title.to_string());
    active.content = Set(content.to_string());
    active.status = Set(status);
    active.updated_at = Set(Utc::now());
    active.update(conn).await?;
    Ok(())
}

pub async fn delete_memory(conn: &sea_orm::DatabaseConnection, id: i32) -> Result<(), DbError> {
    loop_memory::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn list_memory(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
) -> Result<Vec<LoopMemoryRow>, DbError> {
    Ok(loop_memory::Entity::find()
        .filter(loop_memory::Column::SpaceId.eq(space_id))
        .order_by_desc(loop_memory::Column::Id)
        .all(conn)
        .await?
        .into_iter()
        .map(to_row)
        .collect())
}

/// Active memories to inject for `stage` (excludes constitution).
pub async fn list_active_for_stage(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
    stage: Stage,
) -> Result<Vec<loop_memory::Model>, DbError> {
    Ok(loop_memory::Entity::find()
        .filter(loop_memory::Column::SpaceId.eq(space_id))
        .filter(loop_memory::Column::Status.eq(MemoryStatus::Active))
        .filter(loop_memory::Column::Kind.is_in(kinds_for_stage(stage)))
        .order_by_asc(loop_memory::Column::Id)
        .all(conn)
        .await?)
}

/// The space constitution memories (always injected first by the briefing).
pub async fn list_constitution(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
) -> Result<Vec<loop_memory::Model>, DbError> {
    Ok(loop_memory::Entity::find()
        .filter(loop_memory::Column::SpaceId.eq(space_id))
        .filter(loop_memory::Column::Status.eq(MemoryStatus::Active))
        .filter(loop_memory::Column::Kind.eq(MemoryKind::Constitution))
        .order_by_asc(loop_memory::Column::Id)
        .all(conn)
        .await?)
}

/// The full memory index for a space's briefing: EVERY active memory except the
/// constitution (injected as full text separately), ordered by id ascending. No
/// stage filter, no relevance reorder, no scoring, no budget, no truncation — the
/// agent decides what to read via `loop_read_memory`. This is the de-engineered
/// recall path (§4.2). Index-size governance is by validity (superseded/archived
/// leave `active`), never by engine truncation.
pub async fn build_index(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
) -> Result<Vec<loop_memory::Model>, DbError> {
    Ok(loop_memory::Entity::find()
        .filter(loop_memory::Column::SpaceId.eq(space_id))
        .filter(loop_memory::Column::Status.eq(MemoryStatus::Active))
        .filter(loop_memory::Column::Kind.ne(MemoryKind::Constitution))
        .order_by_asc(loop_memory::Column::Id)
        .all(conn)
        .await?)
}

/// Fetch the memories named by `ids` that are still in the **active recall path**:
/// re-scoped to `space_id` (defense-in-depth — the manifest only holds this space's
/// ids, but re-scoping means a tampered manifest still cannot cross spaces), AND
/// `status = active` (a memory archived/superseded between dispatch and read leaves
/// the recall path, §4.6), AND `kind != constitution` (constitution is never in the
/// index). Anything filtered out returns no row, so the caller reports its handle as
/// `not_found`. Ordered by id ascending. Reads only — no usage write.
pub async fn get_for_read(
    conn: &sea_orm::DatabaseConnection,
    space_id: i32,
    ids: &[i32],
) -> Result<Vec<loop_memory::Model>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(loop_memory::Entity::find()
        .filter(loop_memory::Column::SpaceId.eq(space_id))
        .filter(loop_memory::Column::Id.is_in(ids.to_vec()))
        .filter(loop_memory::Column::Status.eq(MemoryStatus::Active))
        .filter(loop_memory::Column::Kind.ne(MemoryKind::Constitution))
        .order_by_asc(loop_memory::Column::Id)
        .all(conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::service::loop_service::space;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};

    async fn mem(
        conn: &sea_orm::DatabaseConnection,
        space_id: i32,
        kind: MemoryKind,
        title: &str,
    ) -> loop_memory::Model {
        create_memory(
            conn,
            space_id,
            kind,
            ActorKind::Agent,
            title,
            None,
            "body",
            TrustTier::Proposed,
            MemoryProvenance::default(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn build_index_is_all_active_non_constitution_by_id() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/repo-idx").await;
        let space = space::create_space(&db.conn, "S", folder).await.unwrap();

        // Seeded out of "kind order"; build_index must order by id, not kind. A
        // constitution, an archived, and a superseded memory are all excluded.
        let m1 = mem(&db.conn, space.id, MemoryKind::Pitfall, "p").await;
        let m2 = mem(&db.conn, space.id, MemoryKind::Decision, "d").await;
        let m3 = mem(&db.conn, space.id, MemoryKind::Constraint, "c").await;
        mem(&db.conn, space.id, MemoryKind::Constitution, "charter").await;
        let archived = mem(&db.conn, space.id, MemoryKind::Preference, "old-pref").await;
        let superseded = mem(&db.conn, space.id, MemoryKind::Decision, "old-dec").await;
        update_memory(&db.conn, archived.id, "old-pref", "body", MemoryStatus::Archived)
            .await
            .unwrap();
        update_memory(&db.conn, superseded.id, "old-dec", "body", MemoryStatus::Superseded)
            .await
            .unwrap();

        let index = build_index(&db.conn, space.id).await.unwrap();
        let ids: Vec<i32> = index.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![m1.id, m2.id, m3.id], "id-ascending, excludes the rest");
        assert!(index.iter().all(|m| m.kind != MemoryKind::Constitution));
    }

    #[tokio::test]
    async fn get_for_read_is_space_scoped() {
        let db = fresh_in_memory_db().await;
        let folder_a = seed_folder(&db, "/tmp/repo-a").await;
        let folder_b = seed_folder(&db, "/tmp/repo-b").await;
        let a = space::create_space(&db.conn, "A", folder_a).await.unwrap();
        let b = space::create_space(&db.conn, "B", folder_b).await.unwrap();
        let in_a = mem(&db.conn, a.id, MemoryKind::Decision, "a-mem").await;
        let in_b = mem(&db.conn, b.id, MemoryKind::Decision, "b-mem").await;

        // Space A asked for an A id + a B id: only the A row comes back.
        let rows = get_for_read(&db.conn, a.id, &[in_a.id, in_b.id]).await.unwrap();
        assert_eq!(rows.iter().map(|m| m.id).collect::<Vec<_>>(), vec![in_a.id]);
        assert!(get_for_read(&db.conn, a.id, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_memory_persists_summary_trust_and_provenance() {
        let db = fresh_in_memory_db().await;
        let folder = seed_folder(&db, "/tmp/repo-prov").await;
        let space = space::create_space(&db.conn, "S", folder).await.unwrap();
        let m = create_memory(
            &db.conn,
            space.id,
            MemoryKind::Pitfall,
            ActorKind::Agent,
            "title",
            Some("one-line summary"),
            "full body",
            TrustTier::Proposed,
            MemoryProvenance {
                source_issue_id: Some(7),
                source_artifact_id: None,
                produced_by_iteration_id: Some(42),
            },
        )
        .await
        .unwrap();
        assert_eq!(m.summary.as_deref(), Some("one-line summary"));
        assert_eq!(m.trust_tier, TrustTier::Proposed);
        assert_eq!(m.source_issue_id, Some(7));
        assert_eq!(m.produced_by_iteration_id, Some(42));
        assert_eq!(m.source_artifact_id, None);
        assert_eq!(m.superseded_by, None);
    }
}
