//! Resolve an issue's effective Loop Contract config. An issue either uses its
//! own stored `config`, or — when `config_inherits` — inherits the space's
//! `default_config`, resolved at read time so a space-default change propagates
//! to every inheriting issue without rewriting their rows.

use sea_orm::{DatabaseConnection, EntityTrait};

use crate::db::entities::{loop_issue, loop_space};
use crate::models::loops::IssueConfig;

/// The config the engine should act on for `issue`. Inheriting issues read the
/// space `default_config` (or the engine default when the space has none);
/// non-inheriting issues parse their own `config`. Malformed JSON falls back to
/// the engine default rather than erroring — the engine must always have a
/// config to act on.
pub async fn effective_config(conn: &DatabaseConnection, issue: &loop_issue::Model) -> IssueConfig {
    if issue.config_inherits {
        if let Ok(Some(space)) = loop_space::Entity::find_by_id(issue.space_id).one(conn).await {
            if let Some(json) = space.default_config.as_deref() {
                if let Ok(cfg) = serde_json::from_str::<IssueConfig>(json) {
                    return cfg;
                }
            }
        }
        return IssueConfig::default();
    }
    serde_json::from_str(&issue.config).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::loop_issue::IssuePriority;
    use crate::db::service::loop_service::{issue, space};
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, QueryFilter};

    async fn fetch_issue(db: &crate::db::AppDatabase, id: i32) -> loop_issue::Model {
        loop_issue::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
    }

    /// Create a space + issue; returns (db, space_id, issue_id).
    async fn seed() -> (crate::db::AppDatabase, i32, i32) {
        let db = fresh_in_memory_db().await;
        let folder_id = seed_folder(&db, "/tmp/cfg-resolver").await;
        let space = space::create_space(&db.conn, "S", folder_id).await.unwrap();
        let detail = issue::create_issue(
            &db.conn,
            space.id,
            "Issue",
            "body",
            IssuePriority::Medium,
            &IssueConfig::default(),
        )
        .await
        .unwrap();
        (db, space.id, detail.row.id)
    }

    async fn set_space_default(db: &crate::db::AppDatabase, space_id: i32, json: Option<String>) {
        loop_space::Entity::update_many()
            .col_expr(loop_space::Column::DefaultConfig, Expr::value(json))
            .filter(loop_space::Column::Id.eq(space_id))
            .exec(&db.conn)
            .await
            .unwrap();
    }

    async fn set_inherits(db: &crate::db::AppDatabase, issue_id: i32, inherits: bool) {
        loop_issue::Entity::update_many()
            .col_expr(loop_issue::Column::ConfigInherits, Expr::value(inherits))
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&db.conn)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn inheriting_issue_resolves_space_default() {
        let (db, space_id, issue_id) = seed().await;
        let space_default = IssueConfig {
            max_attempts: 99,
            ..IssueConfig::default()
        };
        set_space_default(
            &db,
            space_id,
            Some(serde_json::to_string(&space_default).unwrap()),
        )
        .await;
        set_inherits(&db, issue_id, true).await;

        let cfg = effective_config(&db.conn, &fetch_issue(&db, issue_id).await).await;
        assert_eq!(cfg.max_attempts, 99, "inherits the space default");
    }

    #[tokio::test]
    async fn inheriting_issue_with_no_space_default_uses_engine_default() {
        let (db, _space_id, issue_id) = seed().await;
        set_inherits(&db, issue_id, true).await;

        let cfg = effective_config(&db.conn, &fetch_issue(&db, issue_id).await).await;
        assert_eq!(cfg.max_attempts, IssueConfig::default().max_attempts);
    }

    #[tokio::test]
    async fn custom_issue_uses_its_own_config() {
        let (db, space_id, issue_id) = seed().await;
        // A space default exists, but the issue is not inheriting → ignored.
        set_space_default(
            &db,
            space_id,
            Some(serde_json::to_string(&IssueConfig {
                max_attempts: 99,
                ..IssueConfig::default()
            })
            .unwrap()),
        )
        .await;
        let own = IssueConfig {
            max_attempts: 42,
            ..IssueConfig::default()
        };
        loop_issue::Entity::update_many()
            .col_expr(
                loop_issue::Column::Config,
                Expr::value(serde_json::to_string(&own).unwrap()),
            )
            .filter(loop_issue::Column::Id.eq(issue_id))
            .exec(&db.conn)
            .await
            .unwrap();
        // config_inherits stays false (default).

        let cfg = effective_config(&db.conn, &fetch_issue(&db, issue_id).await).await;
        assert_eq!(cfg.max_attempts, 42, "uses its own config, not the space default");
    }
}
