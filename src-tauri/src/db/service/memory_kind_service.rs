use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::memory_kind;
use crate::db::error::DbError;
use crate::models::{MemoryKind, MemoryKindDraft, MemoryMode};

fn mode_to_string(mode: MemoryMode) -> &'static str {
    match mode {
        MemoryMode::Auto => "auto",
        MemoryMode::OnRequest => "on_request",
        MemoryMode::Off => "off",
    }
}

fn mode_from_string(mode: &str) -> MemoryMode {
    match mode {
        "on_request" => MemoryMode::OnRequest,
        "off" => MemoryMode::Off,
        _ => MemoryMode::Auto,
    }
}

fn key_from_name(name: &str) -> String {
    let mut key = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    key = key.trim_matches('_').to_string();
    if key.is_empty() {
        "custom".into()
    } else {
        key
    }
}

fn to_info(model: memory_kind::Model) -> MemoryKind {
    MemoryKind {
        id: model.id,
        key: model.key,
        name: model.name,
        instruction: model.instruction,
        mode: mode_from_string(&model.mode),
        builtin: model.builtin,
        enabled: model.enabled,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub async fn list(conn: &DatabaseConnection) -> Result<Vec<MemoryKind>, DbError> {
    Ok(memory_kind::Entity::find()
        .order_by_asc(memory_kind::Column::Id)
        .all(conn)
        .await?
        .into_iter()
        .map(to_info)
        .collect())
}

pub async fn get(conn: &DatabaseConnection, id: i32) -> Result<MemoryKind, DbError> {
    memory_kind::Entity::find_by_id(id)
        .one(conn)
        .await?
        .map(to_info)
        .ok_or_else(|| DbError::NotFound(format!("memory kind {id} not found")))
}

pub async fn create(
    conn: &DatabaseConnection,
    draft: MemoryKindDraft,
) -> Result<MemoryKind, DbError> {
    if draft.name.trim().is_empty() || draft.instruction.trim().is_empty() {
        return Err(DbError::Validation(
            "memory kind name and instruction are required".into(),
        ));
    }
    let base_key = key_from_name(&draft.name);
    let mut key = base_key.clone();
    let mut suffix = 2;
    while memory_kind::Entity::find()
        .filter(memory_kind::Column::Key.eq(&key))
        .one(conn)
        .await?
        .is_some()
    {
        key = format!("{base_key}_{suffix}");
        suffix += 1;
    }
    let now = Utc::now();
    Ok(to_info(
        memory_kind::ActiveModel {
            id: NotSet,
            key: Set(key),
            name: Set(draft.name.trim().into()),
            instruction: Set(draft.instruction.trim().into()),
            mode: Set(mode_to_string(draft.mode).into()),
            builtin: Set(false),
            enabled: Set(true),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(conn)
        .await?,
    ))
}

pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    draft: MemoryKindDraft,
) -> Result<MemoryKind, DbError> {
    if draft.instruction.trim().is_empty() {
        return Err(DbError::Validation(
            "memory kind instruction is required".into(),
        ));
    }
    let existing = memory_kind::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("memory kind {id} not found")))?;
    let mut active = existing.clone().into_active_model();
    if !existing.builtin {
        if draft.name.trim().is_empty() {
            return Err(DbError::Validation(
                "memory kind name is required".into(),
            ));
        }
        active.name = Set(draft.name.trim().into());
    }
    active.instruction = Set(draft.instruction.trim().into());
    active.mode = Set(mode_to_string(draft.mode).into());
    active.updated_at = Set(Utc::now());
    Ok(to_info(active.update(conn).await?))
}

pub async fn set_enabled(
    conn: &DatabaseConnection,
    id: i32,
    enabled: bool,
) -> Result<MemoryKind, DbError> {
    let existing = memory_kind::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("memory kind {id} not found")))?;
    let mut active = existing.into_active_model();
    active.enabled = Set(enabled);
    active.updated_at = Set(Utc::now());
    Ok(to_info(active.update(conn).await?))
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    let existing = memory_kind::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("memory kind {id} not found")))?;
    if existing.builtin {
        return Err(DbError::Conflict(
            "builtin memory kinds cannot be deleted".into(),
        ));
    }
    memory_kind::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    fn draft(name: &str) -> MemoryKindDraft {
        MemoryKindDraft {
            name: name.into(),
            instruction: "remember it".into(),
            mode: MemoryMode::Auto,
        }
    }

    #[tokio::test]
    async fn cyrillic_names_get_distinct_keys_without_unique_violation() {
        let db = fresh_in_memory_db().await;
        // Every non-ASCII name collapses to the same "custom" fallback key,
        // so creating several must not panic on the UNIQUE(key) constraint.
        let first = create(&db.conn, draft("Заметка")).await.expect("create 1");
        let second = create(&db.conn, draft("Другая")).await.expect("create 2");
        let third = create(&db.conn, draft("Третья")).await.expect("create 3");
        assert_eq!(first.key, "custom");
        assert_eq!(second.key, "custom_2");
        assert_eq!(third.key, "custom_3");
    }

    #[tokio::test]
    async fn builtin_kinds_cannot_be_deleted() {
        let db = fresh_in_memory_db().await;
        let kinds = list(&db.conn).await.expect("list");
        let builtin = kinds.iter().find(|k| k.builtin).expect("seeded builtin");
        assert!(matches!(
            delete(&db.conn, builtin.id).await,
            Err(DbError::Conflict(_))
        ));
    }
}
