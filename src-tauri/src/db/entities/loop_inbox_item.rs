use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Inbox category. Blocking ops = `approval` / `blocked` / `budget_exhausted`;
/// the second pane is `question` (an agent's AskUserQuestion).
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    #[sea_orm(string_value = "approval")]
    Approval,
    #[sea_orm(string_value = "blocked")]
    Blocked,
    #[sea_orm(string_value = "budget_exhausted")]
    BudgetExhausted,
    #[sea_orm(string_value = "question")]
    Question,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "handled")]
    Handled,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "loop_inbox_item")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub space_id: i32,
    pub issue_id: i32,
    /// Set for `question` items (the asking iteration; plain column).
    pub iteration_id: Option<i32>,
    pub kind: InboxKind,
    /// Stable dedupe key; a partial unique index forbids two pending items with
    /// the same `(issue_id, kind, subject_key)`.
    pub subject_key: String,
    /// JSON payload (shape depends on `kind`).
    pub payload: String,
    pub status: InboxStatus,
    /// JSON resolution recorded when handled.
    pub resolution: Option<String>,
    pub created_at: DateTimeUtc,
    pub handled_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
