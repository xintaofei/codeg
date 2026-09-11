use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // folder.is_chat (bool) → folder.kind (text enum)
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .add_column(
                        ColumnDef::new(Folder::Kind)
                            .text()
                            .not_null()
                            .default("regular"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .exec_stmt(
                Query::update()
                    .table(Folder::Table)
                    .value(Folder::Kind, "chat")
                    .and_where(Expr::col(Folder::IsChat).eq(true))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .drop_column(Folder::IsChat)
                    .to_owned(),
            )
            .await?;

        // conversation.kind
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(
                        ColumnDef::new(Conversation::Kind)
                            .text()
                            .not_null()
                            .default("regular"),
                    )
                    .to_owned(),
            )
            .await?;
        // Backfill order matters: delegation children first (they may live in
        // chat folders), then chat limited to top-level rows.
        manager
            .exec_stmt(
                Query::update()
                    .table(Conversation::Table)
                    .value(Conversation::Kind, "delegate")
                    .and_where(Expr::col(Conversation::ParentId).is_not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .exec_stmt(
                Query::update()
                    .table(Conversation::Table)
                    .value(Conversation::Kind, "chat")
                    .and_where(Expr::col(Conversation::ParentId).is_null())
                    .and_where(
                        Expr::col(Conversation::FolderId).in_subquery(
                            Query::select()
                                .column(Folder::Id)
                                .from(Folder::Table)
                                .and_where(Expr::col(Folder::Kind).eq("chat"))
                                .to_owned(),
                        ),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::Kind)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .add_column(
                        ColumnDef::new(Folder::IsChat)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .exec_stmt(
                Query::update()
                    .table(Folder::Table)
                    .value(Folder::IsChat, true)
                    .and_where(Expr::col(Folder::Kind).eq("chat"))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Folder::Table)
                    .drop_column(Folder::Kind)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Folder {
    Table,
    Id,
    Kind,
    IsChat,
}

#[derive(DeriveIden)]
enum Conversation {
    Table,
    Kind,
    ParentId,
    FolderId,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::migration::Migrator;

    fn sql(s: &str) -> Statement {
        Statement::from_string(DbBackend::Sqlite, s.to_owned())
    }

    async fn kind_of(conn: &sea_orm::DatabaseConnection, table: &str, id: i32) -> String {
        let row = conn
            .query_one(sql(&format!("SELECT kind FROM {table} WHERE id = {id}")))
            .await
            .expect("query")
            .expect("row");
        row.try_get::<String>("", "kind").expect("kind column")
    }

    /// Apply every migration except this one, seed legacy-shaped rows via raw
    /// SQL (the entities already reflect the post-migration schema), then run
    /// the remainder and assert the backfill: chat folder → kind='chat';
    /// delegation child → 'delegate' (even inside a chat folder); top-level
    /// chat conversation → 'chat'; everything else → 'regular'.
    #[tokio::test]
    async fn backfills_folder_and_conversation_kind() {
        let conn = Database::connect("sqlite::memory:").await.expect("db");
        // Run every migration up to (but not including) THIS one, located by name
        // rather than `total - 1`: migrations added *after* this file (e.g. the
        // automations tables) would otherwise let `total - 1` run the kind
        // migration here and drop `is_chat` before the legacy rows are inserted.
        let migrations = <Migrator as MigratorTrait>::migrations();
        let kind_idx = migrations
            .iter()
            .position(|m| m.name().contains("conversation_folder_kind"))
            .expect("kind migration is registered");
        Migrator::up(&conn, Some(kind_idx as u32))
            .await
            .expect("legacy migrations");

        let folder_cols = "(id, name, path, last_opened_at, created_at, updated_at, \
                           is_open, sort_order, color, is_chat)";
        conn.execute(sql(&format!(
            "INSERT INTO folder {folder_cols} VALUES \
             (1, 'repo', '/tmp/repo', '2026-01-01 00:00:00', '2026-01-01 00:00:00', \
              '2026-01-01 00:00:00', 1, 1, 'inherit', 0)"
        )))
        .await
        .expect("regular folder");
        conn.execute(sql(&format!(
            "INSERT INTO folder {folder_cols} VALUES \
             (2, 'Chat', '/tmp/chat', '2026-01-01 00:00:00', '2026-01-01 00:00:00', \
              '2026-01-01 00:00:00', 1, 2, 'inherit', 1)"
        )))
        .await
        .expect("chat folder");

        let conv_cols = "(id, folder_id, agent_type, status, message_count, \
                         title_locked, created_at, updated_at, parent_id)";
        // regular conversation in the regular folder
        conn.execute(sql(&format!(
            "INSERT INTO conversation {conv_cols} VALUES \
             (1, 1, 'claude_code', 'completed', 0, 0, '2026-01-01 00:00:00', \
              '2026-01-01 00:00:00', NULL)"
        )))
        .await
        .expect("regular conversation");
        // top-level chat conversation in the chat folder
        conn.execute(sql(&format!(
            "INSERT INTO conversation {conv_cols} VALUES \
             (2, 2, 'claude_code', 'completed', 0, 0, '2026-01-01 00:00:00', \
              '2026-01-01 00:00:00', NULL)"
        )))
        .await
        .expect("chat conversation");
        // delegation child living in the chat folder (delegate wins over chat)
        conn.execute(sql(&format!(
            "INSERT INTO conversation {conv_cols} VALUES \
             (3, 2, 'codex', 'completed', 0, 0, '2026-01-01 00:00:00', \
              '2026-01-01 00:00:00', 2)"
        )))
        .await
        .expect("delegate child");

        Migrator::up(&conn, None).await.expect("kind migration");

        assert_eq!(kind_of(&conn, "folder", 1).await, "regular");
        assert_eq!(kind_of(&conn, "folder", 2).await, "chat");
        assert_eq!(kind_of(&conn, "conversation", 1).await, "regular");
        assert_eq!(kind_of(&conn, "conversation", 2).await, "chat");
        assert_eq!(kind_of(&conn, "conversation", 3).await, "delegate");

        // is_chat is gone from folder
        let row = conn
            .query_one(sql(
                "SELECT COUNT(*) AS n FROM pragma_table_info('folder') \
                            WHERE name = 'is_chat'",
            ))
            .await
            .expect("pragma")
            .expect("row");
        let n: i32 = row.try_get("", "n").expect("count");
        assert_eq!(n, 0, "is_chat column must be dropped");
    }
}
