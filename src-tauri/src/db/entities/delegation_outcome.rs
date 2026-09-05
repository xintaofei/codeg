use sea_orm::entity::prelude::*;

/// One immutable COMPLETED delegation result. Keyed by the broker's `task_id`
/// (= the `delegation_call_id` stored on the child conversation row). Written
/// once by [`crate::db::service::delegation_outcome_service::insert_once`] when
/// the task's current execution wins the terminal race with a successful
/// outcome; later writes for the same task can never overwrite the stored
/// result.
///
/// `status` is always `completed` (enforced by the table CHECK) — this table
/// holds no canceled/failed history, so the upstream `resume_delegation` path
/// keeps working for interrupted tasks under the same id. See the
/// m20260905_000001 migration for the column-level contract.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "delegation_outcome")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub task_id: String,
    pub parent_conversation_id: i32,
    pub parent_tool_use_id: Option<String>,
    pub child_conversation_id: Option<i32>,
    pub agent_type: String,
    pub status: String,
    /// The bounded result text (UTF-8-safe capped by the broker's 256 KiB
    /// policy). Never null — only success results are frozen.
    pub text: String,
    pub duration_ms: i64,
    pub text_truncated: bool,
    pub completed_at: DateTimeUtc,
    pub schema_version: i32,
    /// Non-sensitive facts for strictly re-attaching the same external agent
    /// session (external session id, canonical cwd, agent type, execution
    /// config fingerprint). NULL when the binding could not be verified at the
    /// success moment — such a source is never continuable.
    pub resume_binding_json: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
