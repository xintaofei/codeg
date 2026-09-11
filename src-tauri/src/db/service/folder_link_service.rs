use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Set,
};

use crate::db::entities::{folder, folder_link};
use crate::db::error::DbError;

pub async fn list_by_folder(
    conn: &DatabaseConnection,
    folder_id: i32,
) -> Result<Vec<folder_link::Model>, DbError> {
    Ok(folder_link::Entity::find()
        .filter(folder_link::Column::FolderId.eq(folder_id))
        .order_by_asc(folder_link::Column::Id)
        .all(conn)
        .await?)
}

pub async fn get_by_id(
    conn: &DatabaseConnection,
    link_id: i32,
) -> Result<Option<folder_link::Model>, DbError> {
    Ok(folder_link::Entity::find_by_id(link_id).one(conn).await?)
}

/// Every link paired with the path of the folder it belongs to. Feeds the
/// process-global authorization registry at startup; links whose folder row is
/// gone (soft-deleted or hard-removed) are dropped rather than granting access
/// against a root nothing can reach.
pub async fn list_all_with_root(
    conn: &DatabaseConnection,
) -> Result<Vec<(String, String)>, DbError> {
    let links = folder_link::Entity::find().all(conn).await?;
    if links.is_empty() {
        return Ok(vec![]);
    }

    let folders = folder::Entity::find()
        .filter(folder::Column::DeletedAt.is_null())
        .all(conn)
        .await?;
    let paths: std::collections::HashMap<i32, String> =
        folders.into_iter().map(|f| (f.id, f.path)).collect();

    Ok(links
        .into_iter()
        .filter_map(|l| {
            paths
                .get(&l.folder_id)
                .map(|root| (root.clone(), l.target_path))
        })
        .collect())
}

pub async fn insert(
    conn: &DatabaseConnection,
    folder_id: i32,
    name: &str,
    target_path: &str,
) -> Result<folder_link::Model, DbError> {
    let now = Utc::now();
    let model = folder_link::ActiveModel {
        id: NotSet,
        folder_id: Set(folder_id),
        name: Set(name.to_string()),
        target_path: Set(target_path.to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(model.insert(conn).await?)
}

pub async fn rename(
    conn: &DatabaseConnection,
    link_id: i32,
    new_name: &str,
) -> Result<Option<folder_link::Model>, DbError> {
    let Some(existing) = folder_link::Entity::find_by_id(link_id).one(conn).await? else {
        return Ok(None);
    };
    let mut active: folder_link::ActiveModel = existing.into();
    active.name = Set(new_name.to_string());
    active.updated_at = Set(Utc::now());
    Ok(Some(active.update(conn).await?))
}

/// Hard-delete a link row. Returns the removed row so the caller can drop the
/// on-disk symlink and revoke the matching authorization.
pub async fn delete(
    conn: &DatabaseConnection,
    link_id: i32,
) -> Result<Option<folder_link::Model>, DbError> {
    let Some(existing) = folder_link::Entity::find_by_id(link_id).one(conn).await? else {
        return Ok(None);
    };
    folder_link::Entity::delete_by_id(link_id)
        .exec(conn)
        .await?;
    Ok(Some(existing))
}
