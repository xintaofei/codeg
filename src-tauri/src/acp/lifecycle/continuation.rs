use std::sync::Arc;

use crate::acp::delegation::continuation::{
    CollaborationSessionState, ContinuationCoordinator, TurnTerminal,
};

/// Route an active collaboration execution's turn terminal to its coordinator.
/// Reserved children without a matching execution are also claimed while their
/// collaboration session remains open, quarantining stale or foreign events.
pub(super) async fn forward_turn_complete_to_continuation(
    collaboration: Option<&Arc<ContinuationCoordinator>>,
    connection_id: &str,
    conversation_id: i32,
    stop_reason: &str,
    last_text: Option<String>,
) -> bool {
    let Some(collab) = collaboration else {
        return false;
    };

    let owner = collab.execution_owner(connection_id).await;
    if let Some((turn_id, execution_id)) = owner {
        let terminal = stop_reason_to_terminal(stop_reason, last_text);
        match collab.settle(&turn_id, &execution_id, terminal).await {
            Ok(applied) => {
                if applied {
                    tracing::info!(
                        "[continuation][lifecycle] turn {turn_id} settled from                                      execution {execution_id}"
                    );
                    return true;
                }
                // A stale terminal for a superseded execution — quarantine it
                // (never let it fall through to the one-shot broker: that would
                // complete T0 again).
                tracing::info!(
                    "[continuation][lifecycle] stale terminal for turn                                  {turn_id} (execution {execution_id}) quarantined"
                );
                return true;
            }
            Err(e) => {
                tracing::warn!("[continuation][lifecycle] settle failed for turn {turn_id}: {e}");
                return true;
            }
        }
    }

    // Reserved-but-not-ours: a child under an open collaboration session with
    // no matching execution is a stale/foreign event. Ignore it entirely so it
    // cannot complete the old task either.
    if let Some(session) = collab.session_summary_for_child(conversation_id).await {
        if session.state != CollaborationSessionState::Closed {
            tracing::info!(
                "[continuation][lifecycle] event for reserved child {conversation_id} with no                              matching execution ignored"
            );
            return true;
        }
    }

    false
}

/// Route a child connection terminal (disconnect or error) to the collaboration
/// coordinator when that connection owns an execution. The outcome is unknown
/// because the send may have happened, so the turn settles `outcome_unknown`
/// and the session blocks. Returns `true` when the one-shot broker must not fire.
pub(super) async fn forward_disconnect_to_continuation(
    collaboration: Option<&Arc<ContinuationCoordinator>>,
    connection_id: &str,
) -> bool {
    let Some(collab) = collaboration else {
        return false;
    };
    let Some((turn_id, execution_id)) = collab.execution_owner(connection_id).await else {
        return false;
    };
    match collab.settle_unknown(&turn_id, &execution_id).await {
        Ok(applied) => {
            if !applied {
                tracing::info!(
                    "[continuation][lifecycle] disconnect arrived for already-settled turn                      {turn_id}; ignored"
                );
            }
        }
        Err(e) => {
            tracing::warn!("[continuation][lifecycle] unknown-settle failed for {turn_id}: {e}");
        }
    }
    true
}

/// Map a wire stop reason and the turn's last text onto its collaboration
/// terminal. This mirrors the one-shot broker's outcome mapping.
fn stop_reason_to_terminal(stop_reason: &str, last_text: Option<String>) -> TurnTerminal {
    match stop_reason {
        "end_turn" => TurnTerminal::Completed {
            text: last_text.unwrap_or_default(),
        },
        "cancelled" => TurnTerminal::Canceled,
        other => TurnTerminal::Failed {
            code: "child_failed".to_string(),
            message: format!("the rework round ended with stop reason `{other}`"),
        },
    }
}
