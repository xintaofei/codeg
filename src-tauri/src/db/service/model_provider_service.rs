use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, DatabaseConnection, EntityTrait, IntoActiveModel,
    QueryOrder, Set,
};

use crate::db::entities::model_provider;
use crate::db::error::DbError;

pub async fn create(
    conn: &DatabaseConnection,
    name: String,
    api_url: String,
    api_key: String,
    agent_types_json: String,
) -> Result<model_provider::Model, DbError> {
    let now = Utc::now();
    let active = model_provider::ActiveModel {
        id: NotSet,
        name: Set(name),
        api_url: Set(api_url),
        api_key: Set(api_key),
        agent_types_json: Set(agent_types_json),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(active.insert(conn).await?)
}

pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_types_json: Option<String>,
) -> Result<model_provider::Model, DbError> {
    let model = model_provider::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("model provider not found: {id}")))?;

    let mut active = model.into_active_model();
    if let Some(v) = name {
        active.name = Set(v);
    }
    if let Some(v) = api_url {
        active.api_url = Set(v);
    }
    if let Some(v) = api_key {
        active.api_key = Set(v);
    }
    if let Some(v) = agent_types_json {
        active.agent_types_json = Set(v);
    }
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    model_provider::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn get_by_id(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<Option<model_provider::Model>, DbError> {
    Ok(model_provider::Entity::find_by_id(id).one(conn).await?)
}

pub async fn list_all(conn: &DatabaseConnection) -> Result<Vec<model_provider::Model>, DbError> {
    Ok(model_provider::Entity::find()
        .order_by_asc(model_provider::Column::Id)
        .all(conn)
        .await?)
}
