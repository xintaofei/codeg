use sea_orm_migration::prelude::*;

/// PK arena rounds and the conversation↔round link.
///
/// `pk_round` stores round-level metadata (task, agents, status, competition
/// options) that was previously in localStorage — moving it to the DB makes
/// rounds cross-device, server-mode compatible, and queryable.
///
/// `conversation.pk_round_id` links each contestant session to its round,
/// matching the `kind == 'pk'` invariant. Indexed for the sidebar's
/// per-round grouping query.
#[derive(DeriveMigrationName)]
pub struct Migration;

const IDX_PK_ROUND_FOLDER: &str = "idx_pk_round_folder";
const IDX_CONVERSATION_PK_ROUND: &str = "idx_conversation_pk_round";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PkRound::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PkRound::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    // Soft reference to the project folder (never a worktree).
                    .col(ColumnDef::new(PkRound::FolderId).integer().not_null())
                    .col(ColumnDef::new(PkRound::Task).text().not_null())
                    // JSON: { agents, permission_mode, bare_mode, effort }
                    .col(ColumnDef::new(PkRound::Config).text().not_null())
                    // ready | running | finished | canceled | interrupted
                    .col(
                        ColumnDef::new(PkRound::Status)
                            .string()
                            .not_null()
                            .default("ready"),
                    )
                    .col(ColumnDef::new(PkRound::FailureReason).string().null())
                    .col(
                        ColumnDef::new(PkRound::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PkRound::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PkRound::FinishedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PkRound::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name(IDX_PK_ROUND_FOLDER)
                    .table(PkRound::Table)
                    .col(PkRound::FolderId)
                    .to_owned(),
            )
            .await?;

        // Add pk_round_id column to conversation table.
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(ColumnDef::new(Conversation::PkRoundId).integer().null())
                    .to_owned(),
            )
            .await?;

        // SQLite cannot create an index for a column that has not been added
        // yet, so keep this after the ALTER TABLE above.
        manager
            .create_index(
                Index::create()
                    .name(IDX_CONVERSATION_PK_ROUND)
                    .table(Conversation::Table)
                    .col(Conversation::PkRoundId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .if_exists()
                    .name(IDX_CONVERSATION_PK_ROUND)
                    .table(Conversation::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::PkRoundId)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .if_exists()
                    .name(IDX_PK_ROUND_FOLDER)
                    .table(PkRound::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(PkRound::Table).if_exists().to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PkRound {
    Table,
    Id,
    FolderId,
    Task,
    Config,
    Status,
    FailureReason,
    CreatedAt,
    UpdatedAt,
    FinishedAt,
    DeletedAt,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    PkRoundId,
}

#[cfg(test)]
mod tests {
    use sea_orm::Database;

    use super::*;

    #[tokio::test]
    async fn conversation_round_index_follows_column_lifecycle() {
        let connection = Database::connect("sqlite::memory:").await.unwrap();
        let manager = SchemaManager::new(&connection);
        manager
            .create_table(
                Table::create()
                    .table(Conversation::Table)
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .to_owned(),
            )
            .await
            .unwrap();

        Migration.up(&manager).await.unwrap();
        assert!(manager
            .has_column("conversation", "pk_round_id")
            .await
            .unwrap());
        assert!(manager
            .has_index("conversation", IDX_CONVERSATION_PK_ROUND)
            .await
            .unwrap());

        Migration.down(&manager).await.unwrap();
        assert!(!manager
            .has_column("conversation", "pk_round_id")
            .await
            .unwrap());
        assert!(!manager
            .has_index("conversation", IDX_CONVERSATION_PK_ROUND)
            .await
            .unwrap());
    }
}
