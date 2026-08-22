use sea_orm::entity::prelude::*;

/// Per-conversation timestamps of transcript turns hidden by an edit.
/// See `crate::db::service::conversation_edit_service`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_edit_hidden")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub conversation_id: i32,
    /// JSON array of millisecond timestamps, e.g. `[1710000000000, …]`.
    pub hidden_ts_json: String,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
