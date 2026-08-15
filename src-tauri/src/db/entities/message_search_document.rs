use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One normalized, searchable document per visible conversation.
///
/// Only user and assistant text blocks are stored here; system prompts,
/// thinking, tool calls, and images are excluded by the normalizer before this
/// row is written.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "message_search_document")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub conversation_id: i32,
    pub text: String,
    pub content_hash: String,
    pub source_ended_at: Option<DateTimeUtc>,
    pub source_message_count: i32,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
