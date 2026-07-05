use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "chat_channel_thread_binding")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub channel_id: i32,
    pub channel_type: String,
    pub chat_id: String,
    pub thread_key: String,
    pub thread_kind: String,
    pub conversation_id: i32,
    pub connection_id: Option<String>,
    pub created_by_sender_id: String,
    pub display_title: Option<String>,
    pub title_sync_enabled: bool,
    pub provider_payload_json: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chat_channel::Entity",
        from = "Column::ChannelId",
        to = "super::chat_channel::Column::Id"
    )]
    ChatChannel,
    #[sea_orm(
        belongs_to = "super::conversation::Entity",
        from = "Column::ConversationId",
        to = "super::conversation::Column::Id"
    )]
    Conversation,
}

impl Related<super::chat_channel::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChatChannel.def()
    }
}

impl Related<super::conversation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversation.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
