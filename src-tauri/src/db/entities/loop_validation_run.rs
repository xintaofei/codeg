use sea_orm::entity::prelude::*;

/// One deterministic validation pass (the issue's `validation_commands` run in
/// the worktree). Engine-run, no agent/conversation.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_validation_run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub space_id: i32,
    pub issue_id: i32,
    pub task_artifact_id: i32,
    /// The implement iteration that triggered this run (plain column).
    pub iteration_id: Option<i32>,
    /// JSON array of commands.
    pub commands: String,
    /// JSON array of exit codes.
    pub exit_codes: String,
    pub output: String,
    pub passed: bool,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
