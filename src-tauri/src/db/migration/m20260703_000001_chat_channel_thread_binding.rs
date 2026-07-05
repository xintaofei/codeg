use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ChatChannelThreadBinding::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ChannelId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ChannelType)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ChatId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ThreadKey)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ThreadKind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ConversationId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ConnectionId)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::CreatedBySenderId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::DisplayTitle)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::TitleSyncEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::ProviderPayloadJson)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatChannelThreadBinding::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cctb_channel_id")
                            .from(
                                ChatChannelThreadBinding::Table,
                                ChatChannelThreadBinding::ChannelId,
                            )
                            .to(ChatChannel::Table, ChatChannel::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cctb_conversation_id")
                            .from(
                                ChatChannelThreadBinding::Table,
                                ChatChannelThreadBinding::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_cctb_thread")
                    .table(ChatChannelThreadBinding::Table)
                    .col(ChatChannelThreadBinding::ChannelId)
                    .col(ChatChannelThreadBinding::ThreadKind)
                    .col(ChatChannelThreadBinding::ChatId)
                    .col(ChatChannelThreadBinding::ThreadKey)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_cctb_conversation")
                    .table(ChatChannelThreadBinding::Table)
                    .col(ChatChannelThreadBinding::ChannelId)
                    .col(ChatChannelThreadBinding::ConversationId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ChatChannelThreadBinding::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ChatChannelThreadBinding {
    Table,
    Id,
    ChannelId,
    ChannelType,
    ChatId,
    ThreadKey,
    ThreadKind,
    ConversationId,
    ConnectionId,
    CreatedBySenderId,
    DisplayTitle,
    TitleSyncEnabled,
    ProviderPayloadJson,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ChatChannel {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Id,
}
