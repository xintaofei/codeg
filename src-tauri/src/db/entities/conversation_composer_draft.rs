use sea_orm::entity::prelude::*;

/// Per-conversation unsent composer text, shared across desktop / web / mobile.
/// See `crate::db::service::conversation_composer_draft_service`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_composer_draft")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub conversation_id: i32,
    pub text: String,
    pub revision: i64,
    pub origin: String,
    /// JSON array of jail / file-link refs. Never raw bytes. See the draft
    /// service for the schema and the omit-does-not-wipe PUT contract.
    pub attachments: String,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
