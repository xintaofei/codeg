use super::*;

pub(super) async fn build_continuation_tool(
    ctx: &CompanionContext,
    inflight: Arc<InflightCalls>,
    id: Value,
    name: &str,
    arguments: Value,
    socket: String,
) -> LineAction {
    match name {
        "continue_with_session" => {
            let arg = |name: &str| -> Option<String> {
                arguments
                    .get(name)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            let (Some(source_task_id), Some(request_id), Some(message)) =
                (arg("source_task_id"), arg("request_id"), arg("message"))
            else {
                return LineAction::Respond(err(
                    id,
                    -32602,
                    "continue_with_session requires non-empty string source_task_id,                      request_id, and message",
                ));
            };
            let req = BrokerContinueWithSessionRequest {
                token: ctx.token.clone(),
                source_task_id,
                request_id,
                message,
            };
            let round_trip =
                Box::pin(
                    async move { client_continue_with_session_round_trip(&socket, &req).await },
                );
            register_and_spawn(inflight, id, None, round_trip, render_continuation_result).await
        }
        "get_session_turn_status" => {
            let Some(turn_id) = arguments
                .get("turn_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
            else {
                return LineAction::Respond(err(
                    id,
                    -32602,
                    "get_session_turn_status requires a non-empty string turn_id",
                ));
            };
            // Bounded wait only — an absent/zero wait_ms is an immediate poll;
            // anything above the 30s wire cap is clamped so a confused agent
            // cannot park the companion indefinitely.
            let wait_ms = arguments
                .get("wait_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .min(30_000);
            let req = BrokerGetSessionTurnStatusRequest {
                token: ctx.token.clone(),
                turn_id,
                wait_ms,
            };
            let round_trip =
                Box::pin(
                    async move { client_get_session_turn_status_round_trip(&socket, &req).await },
                );
            register_and_spawn(inflight, id, None, round_trip, render_continuation_result).await
        }
        "cancel_session_turn" => {
            let Some(turn_id) = arguments
                .get("turn_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
            else {
                return LineAction::Respond(err(
                    id,
                    -32602,
                    "cancel_session_turn requires a non-empty string turn_id",
                ));
            };
            let req = BrokerCancelSessionTurnRequest {
                token: ctx.token.clone(),
                turn_id,
            };
            let round_trip =
                Box::pin(async move { client_cancel_session_turn_round_trip(&socket, &req).await });
            register_and_spawn(inflight, id, None, round_trip, render_continuation_result).await
        }
        "close_session" => {
            let Some(session_id) = arguments
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
            else {
                return LineAction::Respond(err(
                    id,
                    -32602,
                    "close_session requires a non-empty string session_id",
                ));
            };
            let req = BrokerCloseSessionRequest {
                token: ctx.token.clone(),
                session_id,
            };
            let round_trip =
                Box::pin(async move { client_close_session_round_trip(&socket, &req).await });
            register_and_spawn(inflight, id, None, round_trip, render_continuation_result).await
        }
        _ => unreachable!("continuation dispatcher called for non-continuation tool"),
    }
}

/// Render a continuation result. On success `outcome` IS the schema_version=1
/// wire DTO; on a business rejection it is `{ "error": ContinuationError }`.
/// Both render as a successful tool result carrying the envelope in
/// `structuredContent` and the JSON in the text content — the same
/// "business errors are answers" contract the delegation status tools use.
fn render_continuation_result(outcome: &Value) -> Value {
    let is_error = outcome.get("error").is_some();
    serde_json::json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(outcome)
                .unwrap_or_else(|_| outcome.to_string()),
        }],
        "structuredContent": outcome.clone(),
        "isError": is_error,
    })
}
