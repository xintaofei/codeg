use sea_orm::entity::prelude::*;

/// The exclusive write relationship over ONE delegation child conversation
/// (v2 design §4.2). Created when the first rework turn for a frozen
/// completed source is accepted; from then until `close_session` the
/// coordinator owns every write to the child session.
///
/// `state`: `open` (normal) / `blocked` (a turn ended `outcome_unknown` — the
/// host cannot prove what the agent did; no new turns) / `closed`
/// (explicitly ended; ordinary chat resumes, but the source is never
/// continuable again). See the m20260905_000002 migration for the contract.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "collaboration_session")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// The frozen completed delegation task this session continues. UNIQUE —
    /// one session per source, forever.
    pub source_task_id: String,
    pub parent_conversation_id: i32,
    /// The child conversation under exclusive coordination. UNIQUE — one
    /// session per child.
    pub child_conversation_id: i32,
    pub state: String,
    /// The verified resume binding copied from the frozen outcome at
    /// acceptance. Immutable from then on.
    pub resume_binding_json: String,
    pub created_at: DateTimeUtc,
    pub closed_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
