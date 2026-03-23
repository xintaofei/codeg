use sea_orm_migration::prelude::*;

mod m20260211_000001_init;
mod m20260219_000001_folder_command;
mod m20260221_000001_folder_is_open;
mod m20260226_000001_agent_setting;
mod m20260227_000001_folder_parent_branch;
mod m20260323_000001_lsp_server_setting;
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260211_000001_init::Migration),
            Box::new(m20260219_000001_folder_command::Migration),
            Box::new(m20260221_000001_folder_is_open::Migration),
            Box::new(m20260226_000001_agent_setting::Migration),
            Box::new(m20260227_000001_folder_parent_branch::Migration),
            Box::new(m20260323_000001_lsp_server_setting::Migration),
        ]
    }
}
