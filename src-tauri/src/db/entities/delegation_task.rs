use sea_orm::entity::prelude::*;

/// Durable admission record for one ordinary delegation execution.
///
/// Conversation rows are foreign-key parents. Soft deletion still hides
/// history from lookup, while physical deletion cascades the ledger row.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "delegation_task")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub task_id: String,
    pub parent_conversation_id: i32,
    pub child_conversation_id: i32,
    pub source_task_id: Option<String>,
    pub task: String,
    pub requested_working_dir: Option<String>,
    /// Current execution status. The terminal report is immutable and lives in
    /// `terminal_report`; this column is not derived from the child row.
    pub status: String,
    /// JSON snapshot of the exact terminal `DelegationTaskReport`.
    pub terminal_report: Option<String>,
    /// JSON [`ResumeBinding`](crate::db::service::delegation_task_service::ResumeBinding).
    pub resume_binding: String,
    pub released: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
