use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Singleton row describing the content-search index lifecycle.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "search_index_state")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub schema_version: i32,
    pub mode: String,
    pub threshold_mb: f64,
    pub short_fts_enabled: bool,
    pub indexed_conversation_count: i32,
    pub last_backfill_at: Option<DateTimeUtc>,
    pub user_enabled: bool,
    pub user_mode: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
