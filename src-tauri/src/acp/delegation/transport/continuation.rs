use serde::{Deserialize, Serialize};
use tokio::io;

use super::{message_round_trip, BrokerMessage, BrokerResponse};

/// Accept a rework turn on a frozen completed delegation source. Backs the
/// `continue_with_session` MCP tool (continuation group). The parent
/// connection id rides the same trusted token framing as every other
/// message; the listener resolves the persistent parent identity from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerContinueWithSessionRequest {
    pub token: String,
    pub source_task_id: String,
    pub request_id: String,
    pub message: String,
}

/// Poll one collaboration turn. Backs `get_session_turn_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerGetSessionTurnStatusRequest {
    pub token: String,
    pub turn_id: String,
    pub wait_ms: u64,
}

/// Cancel one collaboration turn. Backs `cancel_session_turn`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerCancelSessionTurnRequest {
    pub token: String,
    pub turn_id: String,
}

/// Close a collaboration session. Backs `close_session`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerCloseSessionRequest {
    pub token: String,
    pub session_id: String,
}

/// One-shot continuation round-trips: connect, send, read the envelope. The
/// `outcome` value carries the schema_version=1 wire DTO (TurnAck /
/// TurnReport / SessionSummary) on success, or a serialized
/// `ContinuationError` on rejection — the companion renders both as tool
/// content, keeping business errors out of the JSON-RPC error channel.
pub async fn client_continue_with_session_round_trip(
    socket_path: &str,
    req: &BrokerContinueWithSessionRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(
        socket_path,
        &BrokerMessage::ContinueWithSession(req.clone()),
    )
    .await
}

pub async fn client_get_session_turn_status_round_trip(
    socket_path: &str,
    req: &BrokerGetSessionTurnStatusRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(
        socket_path,
        &BrokerMessage::GetSessionTurnStatus(req.clone()),
    )
    .await
}

pub async fn client_cancel_session_turn_round_trip(
    socket_path: &str,
    req: &BrokerCancelSessionTurnRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(socket_path, &BrokerMessage::CancelSessionTurn(req.clone())).await
}

pub async fn client_close_session_round_trip(
    socket_path: &str,
    req: &BrokerCloseSessionRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(socket_path, &BrokerMessage::CloseSession(req.clone())).await
}
