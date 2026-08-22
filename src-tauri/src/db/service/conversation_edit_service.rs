use std::collections::BTreeSet;

use chrono::Utc;
use sea_orm::sea_query::OnConflict;
use sea_orm::{DatabaseConnection, EntityTrait, Set};

use crate::db::entities::conversation_edit_hidden;
use crate::db::error::DbError;
use crate::models::message::MessageTurn;

/// Timestamps currently hidden for this conversation. Empty when the user
/// has never edited a message in it.
pub async fn get_hidden_timestamps(
    conn: &DatabaseConnection,
    conversation_id: i32,
) -> Result<BTreeSet<i64>, DbError> {
    let Some(row) = conversation_edit_hidden::Entity::find_by_id(conversation_id)
        .one(conn)
        .await?
    else {
        return Ok(BTreeSet::new());
    };
    Ok(parse_hidden_ts_json(&row.hidden_ts_json))
}

/// Union `added` into the stored hide set. An empty `added` is a no-op so a
/// caller that failed to resolve the edited turn cannot wipe the transcript.
pub async fn add_hidden_timestamps(
    conn: &DatabaseConnection,
    conversation_id: i32,
    added: &[i64],
) -> Result<BTreeSet<i64>, DbError> {
    if added.is_empty() {
        return get_hidden_timestamps(conn, conversation_id).await;
    }
    let mut hidden = get_hidden_timestamps(conn, conversation_id).await?;
    hidden.extend(added.iter().copied());
    let now = Utc::now();
    let json = serde_json::to_string(&hidden.iter().copied().collect::<Vec<i64>>())
        .unwrap_or_else(|_| "[]".to_string());
    let model = conversation_edit_hidden::ActiveModel {
        conversation_id: Set(conversation_id),
        hidden_ts_json: Set(json),
        updated_at: Set(now),
    };
    conversation_edit_hidden::Entity::insert(model)
        .on_conflict(
            OnConflict::column(conversation_edit_hidden::Column::ConversationId)
            .update_columns([
                conversation_edit_hidden::Column::HiddenTsJson,
                conversation_edit_hidden::Column::UpdatedAt,
            ])
            .to_owned(),
        )
        .exec(conn)
        .await?;
    Ok(hidden)
}

/// Drop turns whose timestamp is in `hidden`. Unparseable timestamps stay
/// visible — better a leftover than a silently swallowed turn.
pub fn filter_hidden_turns(turns: Vec<MessageTurn>, hidden: &BTreeSet<i64>) -> Vec<MessageTurn> {
    if hidden.is_empty() {
        return turns;
    }
    turns
        .into_iter()
        .filter(|turn| !hidden.contains(&turn.timestamp.timestamp_millis()))
        .collect()
}

fn parse_hidden_ts_json(raw: &str) -> BTreeSet<i64> {
    serde_json::from_str::<Vec<i64>>(raw)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use crate::models::message::{MessageTurn, TurnRole};

    fn turn(id: &str, ms: i64) -> MessageTurn {
        MessageTurn {
            id: id.to_string(),
            role: TurnRole::User,
            blocks: vec![],
            timestamp: Utc.timestamp_millis_opt(ms).single().expect("valid ms"),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
        }
    }

    #[test]
    fn filter_drops_only_hidden_timestamps() {
        let turns = vec![turn("a", 1000), turn("b", 2000), turn("c", 3000)];
        let hidden = BTreeSet::from([2000]);
        let kept: Vec<_> = filter_hidden_turns(turns, &hidden)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(kept, ["a", "c"]);
    }

    #[test]
    fn filter_is_noop_when_empty() {
        let turns = vec![turn("a", 1000)];
        let hidden = BTreeSet::new();
        assert_eq!(filter_hidden_turns(turns.clone(), &hidden).len(), 1);
    }

    #[test]
    fn parse_accepts_a_json_array_and_ignores_garbage() {
        assert_eq!(
            parse_hidden_ts_json("[1, 2, 2, 3]"),
            BTreeSet::from([1, 2, 3])
        );
        assert!(parse_hidden_ts_json("nope").is_empty());
    }
}
