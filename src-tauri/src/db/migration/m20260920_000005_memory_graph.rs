/// Memory graph tables (mem_node, mem_edge, FTS5).
///
/// Note: The memory graph lives in a separate `<data_dir>/memory.db` file,
/// not in the main Codeg database. Schema creation is handled by
/// LocalSqliteBackend::init_schema() in memory/sqlite.rs when the backend connects.
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // No-op: memory.db schema is created by LocalSqliteBackend
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // No-op: would be deleted if memory.db itself is deleted
        Ok(())
    }
}
