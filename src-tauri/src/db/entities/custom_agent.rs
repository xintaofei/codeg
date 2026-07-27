use sea_orm::entity::prelude::*;

/// A user-registered ACP agent. See `crate::acp::custom_registry` for how a row
/// becomes launch metadata, and `crate::models::agent::AgentType::Custom` for
/// how it is referenced from conversations.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "custom_agent")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// ACP registry id — the agent's identity everywhere else in codeg.
    #[sea_orm(unique)]
    pub registry_id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// `npx` | `uvx` | `binary`.
    pub distribution_kind: String,
    /// The ACP registry `distribution` object, verbatim.
    pub spec_json: String,
    pub icon_url: Option<String>,
    /// User declaration that the agent reads the shared `.agents/skills`
    /// store — the gate for every skills surface. See
    /// `CustomAgentDef::skills_shared_store`.
    pub skills_shared_store: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
