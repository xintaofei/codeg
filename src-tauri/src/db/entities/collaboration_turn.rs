use sea_orm::entity::prelude::*;

/// One rework round inside a [`super::collaboration_session::Model`].
///
/// `state` lifecycle: accepted → preparing → dispatching → running →
/// (cancel_requested) → completed / failed / canceled / interrupted /
/// outcome_unknown. Transitions are CAS updates guarded by the current state
/// AND the owning `execution_id`; the partial unique index
/// `idx_collaboration_turn_one_active` makes two concurrently-active turns
/// per session impossible at the storage layer.
///
/// `execution_id` is minted by the coordinator per attach; late events from
/// an older connection carry a stale execution id and are rejected by
/// `settle`. `connection_id` is diagnostic only — it is NOT a restart-safe
/// fact. See the m20260905_000002 migration for the full contract.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "collaboration_turn")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub session_id: String,
    /// 1-based rework round within the session. UNIQUE per session.
    pub ordinal: i32,
    /// Caller-supplied idempotency key. UNIQUE per session: the same
    /// (session, request_id) pair always resolves to this turn.
    pub request_id: String,
    /// The original rework message, stored verbatim (dedup compares bytes).
    pub message: String,
    /// Who initiated the turn — constant `parent_agent` for this MVP.
    pub initiator_kind: String,
    pub initiator_parent_conversation_id: i32,
    /// The parent-side tool_use_id of the continue_with_session call, when
    /// one exists. Never fabricated.
    pub initiator_tool_use_id: Option<String>,
    pub state: String,
    /// Coordinator-minted execution identity for THIS attach; guards settle
    /// against late events from superseded connections.
    pub execution_id: String,
    /// Diagnostic only — NOT a restart-safe fact.
    pub connection_id: Option<String>,
    /// Bounded result text of the rework round (same 256 KiB policy as the
    /// delegation outcome store).
    pub result_text: Option<String>,
    pub text_truncated: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTimeUtc,
    pub started_at: Option<DateTimeUtc>,
    pub finished_at: Option<DateTimeUtc>,
    /// Monotonic row version — bumped on every transition so UI polling can
    /// discard stale snapshots.
    pub version: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
