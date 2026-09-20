use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// How an automation fires. `schedule` runs on its cron; `manual` only ever runs
/// via an explicit "Run now" (it has no `next_run_at` and the scheduler skips it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    #[sea_orm(string_value = "schedule")]
    Schedule,
    #[sea_orm(string_value = "manual")]
    Manual,
}

/// Where a fired run executes relative to the target folder. `worktree_per_run`
/// mints a fresh git worktree (branch `automation/<id>/run-<run_id>`) each fire so
/// runs never collide on a working tree; `shared_in_root` checks the branch out in
/// the root repo (serialized per root folder).
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    #[sea_orm(string_value = "worktree_per_run")]
    WorktreePerRun,
    #[sea_orm(string_value = "shared_in_root")]
    SharedInRoot,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "automation")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub enabled: bool,
    pub trigger_kind: TriggerKind,
    /// 5-field cron expression; `Some` iff `trigger_kind == Schedule`.
    pub cron: Option<String>,
    /// IANA timezone the cron is evaluated in (e.g. "Asia/Shanghai").
    pub timezone: String,
    /// Next fire instant, stored UTC. The scheduler's due key; recomputed forward
    /// after every fire so a restart catch-up fires at most once.
    pub next_run_at: Option<DateTimeUtc>,
    pub agent_type: String,
    /// Target root repo folder. `None` = folderless (reserved; v1 requires a folder).
    pub root_folder_id: Option<i32>,
    pub isolation: IsolationMode,
    /// Verbatim git ref (branch-tree `fullName`, not a display label).
    pub branch: Option<String>,
    pub is_remote_branch: bool,
    /// JSON snapshot of the captured composer state (prompt blocks, mode, config
    /// values, label cache). Replayed wholesale at fire; never parsed for queries.
    #[sea_orm(column_type = "Text")]
    pub config: String,
    pub last_run_at: Option<DateTimeUtc>,
    pub last_run_status: Option<String>,
    pub last_run_conversation_id: Option<i32>,
    /// Count of settled-failed runs the user hasn't seen yet; drives the sidebar
    /// badge. Cleared to 0 when the automations view is opened.
    pub unseen_failures: i32,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub deleted_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::automation_run::Entity")]
    Runs,
}

impl Related<super::automation_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Runs.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
