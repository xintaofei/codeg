//! Companion-side MCP protocol — the bits that live inside the `codeg-mcp`
//! binary but are factored out into the library so they can be unit-tested
//! without spawning the binary.
//!
//! The companion speaks newline-delimited JSON-RPC 2.0 on stdio:
//! one request → one response per line, with concurrent dispatch so
//! `notifications/cancelled` can race an in-flight `tools/call`. It exposes up
//! to four tools — `delegate_to_agent` (async; returns a `task_id` ack),
//! `get_delegation_status` (poll/long-poll for the result), `cancel_delegation`,
//! and `check_user_feedback` (pull the user's mid-turn steering notes) — whose
//! schemas are embedded at compile time from [`TOOL_SCHEMA_JSON`] and gated by
//! the `--features` groups (delegation / feedback). Only `delegate_to_agent`
//! registers a broker-side cancel handle; canceling a status / cancel / feedback
//! round-trip merely suppresses its response — and for `check_user_feedback`
//! also skips the delivery commit, so a cancelled note stays pending.
//!
//! Notifications (id = None) produce no response, matching MCP's expectation
//! that `notifications/initialized` etc. are fire-and-forget.
//!
//! Cancellation flow per the MCP 2024-11-05 / 2025-11-25 cancellation utility:
//!
//! 1. Companion receives `tools/call` with JSON-RPC `id = X`, mints an opaque
//!    `external_handle`, registers `X → (handle, cancel_tx)` in
//!    [`InflightCalls`], and kicks off the broker round-trip.
//! 2. If `notifications/cancelled` for `requestId = X` arrives, the
//!    notification handler pops the entry, fires `cancel_tx`, and sends a
//!    `BrokerMessage::Cancel { external_handle }` to the broker.
//! 3. The `tools/call` task observes `cancel_tx`, abandons its UDS read,
//!    and returns `None` — the binary suppresses the response per spec.
//! 4. If the round-trip completes before the cancel arrives, the entry is
//!    removed normally and the response goes out on stdout; a late cancel
//!    notification finds nothing and is silently ignored.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

use crate::acp::delegation::transport::{
    client_cancel, client_cancel_task_round_trip, client_commit_feedback,
    client_feedback_round_trip, client_round_trip, client_status_round_trip, BrokerCancelRequest,
    BrokerCancelTaskRequest, BrokerCommitFeedbackRequest, BrokerFeedbackRequest, BrokerRequest,
    BrokerResponse, BrokerStatusRequest,
};

/// Upper bound on one broker-side cancel round-trip. Bounds both
/// `handle_cancel_notification` (so stdin dispatch can't stall behind a
/// stuck UDS connect/read) and the shutdown-drain loop (so an
/// unresponsive listener can't keep the EOF / watchdog path hung). 500 ms
/// is generous for a same-host UDS exchange and short enough that a user
/// won't notice the bound being hit. Misses are absorbed by the codeg
/// main side's `cancel_by_parent` cascade when the parent ACP connection
/// eventually ends.
const BROKER_CANCEL_BUDGET: Duration = Duration::from_millis(500);

/// Wrap `client_cancel` in [`BROKER_CANCEL_BUDGET`] so callers can fire
/// a synchronous cancel without worrying about a hung listener freezing
/// them. Both success, transport error, and timeout collapse to `()` —
/// callers couldn't usefully react anyway, and the broker has independent
/// cancel backstops (parent / child disconnect cascades) if this one
/// misses.
async fn send_broker_cancel(socket_path: &str, req: &BrokerCancelRequest) {
    let _ = tokio::time::timeout(BROKER_CANCEL_BUDGET, client_cancel(socket_path, req)).await;
}

/// Static MCP tool schema. Lives next to this module so codeg-mcp ships
/// a single embedded copy — no runtime file IO, no version skew with the
/// broker's [`super::types::DelegationRequest`].
pub const TOOL_SCHEMA_JSON: &str = include_str!("tool_schema.json");

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    /// MCP notifications carry no `id`. We dispatch a response only when this
    /// is `Some`.
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

pub fn ok(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(result),
        error: None,
    }
}

pub fn err(id: Value, code: i64, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: None,
        }),
    }
}

/// Which tool groups this companion exposes. One `codeg-mcp` process can carry
/// the delegation tools, the feedback tool, or both — gated independently so
/// each feature can be toggled in settings without the other. Passed in via the
/// `--features` arg at launch; a tool whose group is off is hidden from
/// `tools/list` and rejected on `tools/call`.
#[derive(Debug, Clone, Copy)]
pub struct CompanionFeatures {
    pub delegation: bool,
    pub feedback: bool,
}

impl CompanionFeatures {
    /// Parse the comma-joined `--features` value (e.g. `delegation,feedback`).
    /// Unknown tokens are ignored. An absent value (`None`) defaults to
    /// delegation-only — backward compatible with a parent that predates
    /// feature gating (companion + listener ship together, so post-upgrade the
    /// parent always passes an explicit `--features`).
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(s) = raw else {
            return Self {
                delegation: true,
                feedback: false,
            };
        };
        let mut f = Self {
            delegation: false,
            feedback: false,
        };
        for tok in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            match tok {
                "delegation" => f.delegation = true,
                "feedback" => f.feedback = true,
                _ => {}
            }
        }
        f
    }

    /// Whether the named MCP tool is exposed under the enabled feature groups.
    pub fn allows_tool(&self, name: &str) -> bool {
        match name {
            "check_user_feedback" => self.feedback,
            "delegate_to_agent" | "get_delegation_status" | "cancel_delegation" => self.delegation,
            _ => false,
        }
    }
}

/// Process arguments threaded through every `tools/call` so the dispatcher
/// can build a [`BrokerRequest`] without re-parsing argv per call.
#[derive(Debug, Clone)]
pub struct CompanionContext {
    pub parent_connection_id: String,
    pub socket_path: String,
    pub token: String,
    /// Tool groups this launch exposes (see [`CompanionFeatures`]).
    pub features: CompanionFeatures,
}

/// Per-in-flight-call state. The companion stashes one of these per
/// `tools/call` so a subsequent `notifications/cancelled` for the same
/// JSON-RPC `id` can wake the round-trip task and trigger a broker-side
/// cancel.
pub struct InflightEntry {
    /// Companion-minted opaque handle threaded through the broker, for the
    /// `delegate_to_agent` tool ONLY — a `notifications/cancelled` during its
    /// setup must tear down the just-started child via the broker's
    /// `cancel_by_external_handle`. `None` for `get_delegation_status` /
    /// `cancel_delegation`: canceling those round-trips only suppresses the
    /// response (no broker-side cancel — the query/cancel itself must not touch
    /// the task).
    external_handle: Option<String>,
    /// Tripped by the cancel handler to wake the round-trip task.
    cancel_tx: oneshot::Sender<()>,
}

/// `request_id_key(id) → InflightEntry`. Keyed by a string form of the
/// JSON-RPC `id` so we can compare against the `requestId` payload of
/// `notifications/cancelled` which is itself a JSON value (numbers serialize
/// as their canonical string form here).
#[derive(Default)]
pub struct InflightCalls {
    inner: Mutex<HashMap<String, InflightEntry>>,
}

impl InflightCalls {
    pub fn new() -> Self {
        Self::default()
    }

    async fn register(&self, id_key: String, entry: InflightEntry) {
        self.inner.lock().await.insert(id_key, entry);
    }

    async fn take(&self, id_key: &str) -> Option<InflightEntry> {
        self.inner.lock().await.remove(id_key)
    }

    /// Drain every in-flight entry, clearing the registry. Called at
    /// companion shutdown so we can fire one broker cancel per pending
    /// delegation — without this the broker would park on `rx.await` for
    /// each entry until the parent ACP connection's `cancel_by_parent`
    /// fires (or never, if the agent CLI keeps running after only the
    /// MCP child died).
    pub async fn drain_all(&self) -> Vec<InflightEntry> {
        let mut map = self.inner.lock().await;
        map.drain().map(|(_k, v)| v).collect()
    }
}

/// Canonicalize a JSON-RPC `id` to a string suitable as a `HashMap` key.
/// JSON-RPC permits string OR number ids; we collapse both via
/// `serde_json::to_string` so a numeric `42` and string `"42"` stay
/// distinct (which the spec also requires).
pub fn request_id_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| String::from("null"))
}

/// Dispatch verdict for a single inbound stdin line.
pub enum LineAction {
    /// Synchronous response — write `resp` to stdout immediately.
    Respond(JsonRpcResponse),
    /// Asynchronous tools/call — the binary should spawn the round-trip
    /// task and only write a response if the future returns `Some`.
    Spawn(SpawnedCall),
    /// Notification or no-op (parse errors with `id = null`). Nothing to
    /// emit on stdout.
    Silent,
}

/// Resolution of a spawned `tools/call`: the response to relay to the agent
/// (`None` = cancellation won, so suppress per the MCP spec) plus an optional
/// action the binary runs ONLY after that response is successfully written to
/// the agent's stdout.
///
/// `after_relay` exists for `check_user_feedback`: marking the pulled notes
/// `Delivered` (the broker `CommitFeedback`) must happen strictly AFTER the
/// agent actually receives them. Committing any earlier — at listener read
/// time, or right after the round-trip but before the stdout relay — would mark
/// a note delivered that a failed/never-reached write (or a companion dying mid
/// teardown) never put in front of the agent, breaking at-least-once delivery.
/// Every other tool leaves this `None`.
pub struct SpawnResult {
    pub response: Option<JsonRpcResponse>,
    pub after_relay: Option<futures_util::future::BoxFuture<'static, ()>>,
}

/// Materialized async tools/call ready to drive in a tokio task. The binary
/// awaits `future` to obtain the [`SpawnResult`]: it writes `response` (when
/// `Some`) and, on a successful write, runs `after_relay` (when `Some`).
pub struct SpawnedCall {
    /// JSON-RPC `id` of the original `tools/call` so the binary can stamp
    /// the response.
    pub request_id: Value,
    /// String form of `request_id` for inflight bookkeeping.
    pub request_id_key: String,
    /// The future that performs the UDS round-trip racing the cancel channel
    /// and resolves to the [`SpawnResult`] to relay (and optionally commit).
    pub future: futures_util::future::BoxFuture<'static, SpawnResult>,
}

/// Parse a stdin line and produce a [`LineAction`]. The binary handles the
/// IO side; this function is pure aside from registering the inflight
/// entry on `tools/call` so unit tests can drive it without stdio.
pub async fn dispatch_line(
    ctx: &CompanionContext,
    inflight: Arc<InflightCalls>,
    line: &str,
) -> LineAction {
    let req: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return LineAction::Respond(err(Value::Null, -32700, format!("parse error: {e}")));
        }
    };

    // Notifications carry no id — no response goes out. Cancellation is
    // the only notification we act on.
    if req.id.is_none() {
        if req.method == "notifications/cancelled" {
            handle_cancel_notification(ctx, &inflight, &req.params).await;
        }
        return LineAction::Silent;
    }

    let id = req.id.expect("checked is_none");
    match req.method.as_str() {
        "initialize" => LineAction::Respond(ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "codeg-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "tools": {} },
            }),
        )),
        "tools/list" => {
            // The embedded schema is a JSON array of every tool the companion
            // can carry; filter to the groups enabled for this launch so a
            // disabled feature's tools never surface to the LLM.
            let all: Value = match serde_json::from_str(TOOL_SCHEMA_JSON) {
                Ok(v) => v,
                Err(e) => {
                    return LineAction::Respond(err(
                        id,
                        -32603,
                        format!("embedded schema invalid: {e}"),
                    ));
                }
            };
            let tools = match all.as_array() {
                Some(arr) => Value::Array(
                    arr.iter()
                        .filter(|t| {
                            t.get("name")
                                .and_then(|v| v.as_str())
                                .map(|n| ctx.features.allows_tool(n))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect(),
                ),
                None => all,
            };
            LineAction::Respond(ok(id, json!({ "tools": tools })))
        }
        "tools/call" => build_tools_call_spawn(ctx.clone(), inflight, id, req.params).await,
        _ => LineAction::Respond(err(id, -32601, format!("method not found: {}", req.method))),
    }
}

/// Build the spawned-call descriptor for a `tools/call` (or, when the
/// arguments are obviously bogus, a synchronous error response). Registers
/// the inflight entry and returns a future the binary should drive.
async fn build_tools_call_spawn(
    ctx: CompanionContext,
    inflight: Arc<InflightCalls>,
    id: Value,
    params: Value,
) -> LineAction {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let socket = ctx.socket_path.clone();
    // Defense in depth: tools/list already hides tools whose feature group is
    // off, but a misbehaving client could still call one by name. A disabled
    // tool is rejected uniformly as "unknown tool" — indistinguishable from a
    // genuinely nonexistent one (no leak that the feature exists but is off),
    // and matching the legacy unknown-tool rejection shape.
    if !ctx.features.allows_tool(&name) {
        return LineAction::Respond(err(id, -32602, format!("unknown tool: {name}")));
    }
    match name.as_str() {
        "delegate_to_agent" => {
            // MCP clients (Codex / Claude Code) generally do NOT populate
            // `_meta.tool_use_id` when calling an MCP server. We still surface it
            // when present (the most precise binding), but a missing one is
            // expected — the broker falls back to claiming the most recent
            // `delegate_to_agent` tool_call_id observed on the parent's ACP
            // event stream.
            let tool_use_id = params
                .get("_meta")
                .and_then(|m| m.get("tool_use_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Mint an external_handle so a `notifications/cancelled` during setup
            // tears down the just-started child via `cancel_by_external_handle`.
            let external_handle = uuid::Uuid::new_v4().to_string();
            let req = BrokerRequest {
                token: ctx.token.clone(),
                parent_connection_id: ctx.parent_connection_id.clone(),
                parent_tool_use_id: tool_use_id,
                external_handle: Some(external_handle.clone()),
                input: arguments,
            };
            let round_trip = Box::pin(async move { client_round_trip(&socket, &req).await });
            register_and_spawn(
                inflight,
                id,
                Some(external_handle),
                round_trip,
                render_task_report,
            )
            .await
        }
        "get_delegation_status" => {
            // Normalize the `task_ids` array: trim, drop empty/whitespace
            // entries, de-dup (order-preserving). A non-string entry violates the
            // schema's `items: string` contract and is rejected outright (rather
            // than silently polling a subset); an all-empty / missing array maps
            // to `Ok(empty)`, rejected below.
            let task_ids = match normalize_status_task_ids(&arguments) {
                Ok(ids) if !ids.is_empty() => ids,
                Ok(_) => {
                    return LineAction::Respond(err(
                        id,
                        -32602,
                        "get_delegation_status requires a non-empty task_ids array \
                         (one or more task ids)",
                    ));
                }
                Err(msg) => return LineAction::Respond(err(id, -32602, msg)),
            };
            let wait_ms = arguments.get("wait_ms").and_then(|v| v.as_u64());
            let req = BrokerStatusRequest {
                token: ctx.token.clone(),
                task_ids,
                wait_ms,
            };
            // No external_handle: canceling a status query only suppresses its
            // response — it must not touch the task itself. The status round-trip
            // returns a `{tasks:[..]}` envelope, so it renders via
            // `render_status_result` — uniformly one `{tasks:[..]}` entry per id,
            // whether the poll asked for a single id or a whole fan-out.
            let round_trip = Box::pin(async move { client_status_round_trip(&socket, &req).await });
            register_and_spawn(inflight, id, None, round_trip, render_status_result).await
        }
        "cancel_delegation" => {
            let task_id = match arguments.get("task_id").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return LineAction::Respond(err(
                        id,
                        -32602,
                        "cancel_delegation requires a non-empty string task_id",
                    ));
                }
            };
            let req = BrokerCancelTaskRequest {
                token: ctx.token.clone(),
                task_id,
            };
            let round_trip =
                Box::pin(async move { client_cancel_task_round_trip(&socket, &req).await });
            register_and_spawn(inflight, id, None, round_trip, render_task_report).await
        }
        "check_user_feedback" => {
            let req = BrokerFeedbackRequest {
                token: ctx.token.clone(),
            };
            // Feedback uses a dedicated spawn so it can COMMIT delivery only when
            // the round-trip wins the cancel race (i.e. the result actually goes
            // to the agent). A cancel that suppresses the response sends no
            // commit, leaving the notes pending for the next check.
            register_and_spawn_feedback(inflight, id, socket, ctx.token.clone(), req).await
        }
        other => LineAction::Respond(err(id, -32602, format!("unknown tool: {other}"))),
    }
}

/// Register the inflight entry and build the [`SpawnedCall`] that races the
/// broker round-trip against the cancel signal. `external_handle` is `Some` only
/// for `delegate_to_agent` (so a cancel during setup tears the child down);
/// `None` for status/cancel queries (a cancel only suppresses the response).
///
/// `render` maps the broker's `BrokerResponse.outcome` into the MCP `tools/call`
/// result body: `delegate_to_agent` / `cancel_delegation` pass
/// [`render_task_report`] (a single report); `get_delegation_status` passes
/// [`render_status_result`] (always a `{tasks:[..]}` envelope, one entry per id).
async fn register_and_spawn(
    inflight: Arc<InflightCalls>,
    id: Value,
    external_handle: Option<String>,
    round_trip: futures_util::future::BoxFuture<'static, std::io::Result<BrokerResponse>>,
    render: fn(&Value) -> Value,
) -> LineAction {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let id_key = request_id_key(&id);
    inflight
        .register(
            id_key.clone(),
            InflightEntry {
                external_handle,
                cancel_tx,
            },
        )
        .await;

    let id_for_response = id.clone();
    let id_key_for_task = id_key.clone();
    let inflight_for_task = inflight.clone();
    let future = Box::pin(async move {
        // Race the UDS round-trip against the cancel signal. Cancel wins →
        // suppress the response per MCP spec; for `delegate_to_agent` the cancel
        // notification handler is responsible for dispatching the broker-side
        // `Cancel` (status/cancel queries carry no external_handle, so nothing
        // is dispatched).
        let response = tokio::select! {
            biased;
            _ = cancel_rx => {
                let _ = inflight_for_task.take(&id_key_for_task).await;
                None
            }
            rt = round_trip => {
                let _ = inflight_for_task.take(&id_key_for_task).await;
                match rt {
                    Ok(resp) => Some(ok(id_for_response, render(&resp.outcome))),
                    Err(e) => Some(err(
                        id_for_response,
                        -32603,
                        format!("broker round-trip failed: {e}"),
                    )),
                }
            }
        };
        // Delegation / status / cancel have no post-relay step.
        SpawnResult {
            response,
            after_relay: None,
        }
    });

    LineAction::Spawn(SpawnedCall {
        request_id: id,
        request_id_key: id_key,
        future,
    })
}

/// `check_user_feedback`-specific spawn. Like [`register_and_spawn`], but it
/// carries an `after_relay` commit — a `CommitFeedback` round-trip marking the
/// pulled notes `Delivered` — that the binary runs ONLY after it successfully
/// writes this response to the agent's stdout (the listener does not commit at
/// read time). Two guards compose to make delivery at-least-once. First, if the
/// cancel branch wins the biased select the result is `response: None` with no
/// `after_relay`, so the check is suppressed and never committed (the notes stay
/// pending for the next check). Second, when the round-trip wins, `after_relay`
/// is built but only fires once the stdout relay succeeds; a failed or
/// never-reached write (a dying companion, a broken agent stdin) skips the
/// commit entirely. So a note flips to `Delivered` only after it was actually
/// put in front of the agent. The sole irreducible boundary is the agent
/// crashing after the bytes are flushed to its stdin but before it reads them —
/// at which point the note is moot (the agent will not act on it), the correct
/// semantics for a delivered best-effort steering side-channel.
async fn register_and_spawn_feedback(
    inflight: Arc<InflightCalls>,
    id: Value,
    socket: String,
    token: String,
    req: BrokerFeedbackRequest,
) -> LineAction {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let id_key = request_id_key(&id);
    inflight
        .register(
            id_key.clone(),
            InflightEntry {
                external_handle: None,
                cancel_tx,
            },
        )
        .await;

    let id_for_response = id.clone();
    let id_key_for_task = id_key.clone();
    let inflight_for_task = inflight.clone();
    let future = Box::pin(async move {
        tokio::select! {
            biased;
            _ = cancel_rx => {
                // Cancelled before delivery → suppress AND do not commit.
                let _ = inflight_for_task.take(&id_key_for_task).await;
                SpawnResult {
                    response: None,
                    after_relay: None,
                }
            }
            rt = client_feedback_round_trip(&socket, &req) => {
                let _ = inflight_for_task.take(&id_key_for_task).await;
                match rt {
                    Ok(resp) => {
                        // Relay-then-commit: render the agent-facing result now,
                        // but defer the `CommitFeedback` to `after_relay` so it
                        // fires ONLY after the binary writes this response to the
                        // agent's stdout. A dead/failed relay skips the commit,
                        // leaving the notes pending for the next check
                        // (at-least-once at the agent-facing boundary).
                        let outcome = resp.outcome;
                        let response = ok(id_for_response, render_feedback_result(&outcome));
                        let commit: futures_util::future::BoxFuture<'static, ()> =
                            Box::pin(async move {
                                commit_feedback_after_delivery(&socket, &token, &outcome).await;
                            });
                        SpawnResult {
                            response: Some(response),
                            after_relay: Some(commit),
                        }
                    }
                    Err(e) => SpawnResult {
                        response: Some(err(
                            id_for_response,
                            -32603,
                            format!("broker round-trip failed: {e}"),
                        )),
                        after_relay: None,
                    },
                }
            }
        }
    });

    LineAction::Spawn(SpawnedCall {
        request_id: id,
        request_id_key: id_key,
        future,
    })
}

/// Send a `CommitFeedback` for the note ids the listener embedded in the
/// response (`_commit_ids`). Fire-and-forget, bounded by [`BROKER_CANCEL_BUDGET`]:
/// a failed commit just leaves the notes pending for the next check.
async fn commit_feedback_after_delivery(socket: &str, token: &str, outcome: &Value) {
    let ids: Vec<String> = outcome
        .get("_commit_ids")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return;
    }
    let req = BrokerCommitFeedbackRequest {
        token: token.to_string(),
        ids,
    };
    let _ = tokio::time::timeout(BROKER_CANCEL_BUDGET, client_commit_feedback(socket, &req)).await;
}

/// Handle a `notifications/cancelled` notification. Looks up the in-flight
/// call by `requestId` and fires its cancel channel. Unknown ids are
/// silently ignored per MCP spec.
async fn handle_cancel_notification(
    ctx: &CompanionContext,
    inflight: &Arc<InflightCalls>,
    params: &Value,
) {
    let request_id = match params.get("requestId") {
        Some(v) => v.clone(),
        None => return,
    };
    let id_key = request_id_key(&request_id);
    let Some(entry) = inflight.take(&id_key).await else {
        return;
    };
    let _ = entry.cancel_tx.send(());
    // Only `delegate_to_agent` carries an external_handle. For
    // `get_delegation_status` / `cancel_delegation` there is nothing to cancel
    // broker-side — suppressing the (possibly long-poll) response is the whole
    // effect, and dispatching a broker `Cancel` would wrongly target a task.
    let Some(external_handle) = entry.external_handle else {
        return;
    };
    // Single broker-side cancel per notification: the round-trip task
    // observes `cancel_rx` and only suppresses its response. If we ALSO
    // dispatched a cancel from the task we'd hit the broker twice — the
    // first call drains the pending entry, the second buffers the handle
    // in `pre_canceled_handles` with no consumer (silent leak).
    //
    // Synchronous, bounded by `BROKER_CANCEL_BUDGET`. Detaching via
    // `tokio::spawn` would race the runtime shutdown: if stdin closes
    // before the spawned task scheduled its UDS connect, the runtime
    // drops it and the broker never gets the cancel. The bounded await
    // here guarantees the cancel either lands or hits a known cap
    // before the next stdin line is read.
    let cancel_req = BrokerCancelRequest {
        token: ctx.token.clone(),
        external_handle,
        reason: params
            .get("reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    };
    send_broker_cancel(&ctx.socket_path, &cancel_req).await;
}

/// Drain every in-flight `tools/call` entry and dispatch a broker cancel
/// for each. Called at companion shutdown (stdin EOF, parent-watchdog
/// fire) so the broker doesn't hold a `pending` row open forever waiting
/// for a `TurnComplete` whose response we couldn't deliver anyway. Each
/// cancel is bounded by [`BROKER_CANCEL_BUDGET`] so a hung listener
/// can't pin shutdown — the codeg main side's `cancel_by_parent` cascade
/// is the eventual backstop for any cancel that times out here.
pub async fn drain_and_cancel_all(
    ctx: &CompanionContext,
    inflight: &Arc<InflightCalls>,
    reason: &str,
) {
    for entry in inflight.drain_all().await {
        // Wake the round-trip task if it's still scheduled, so it can
        // exit promptly when the runtime tears down.
        let _ = entry.cancel_tx.send(());
        // Only delegate_to_agent entries hold an external_handle worth a
        // broker-side cancel; status/cancel queries have nothing to tear down.
        let Some(external_handle) = entry.external_handle else {
            continue;
        };
        let cancel_req = BrokerCancelRequest {
            token: ctx.token.clone(),
            external_handle,
            reason: Some(reason.to_string()),
        };
        send_broker_cancel(&ctx.socket_path, &cancel_req).await;
    }
}

/// Normalize the MCP `get_delegation_status` arguments into the wire `task_ids`
/// list. Reads the `task_ids` array, trims each entry, drops empty / whitespace
/// strings, and de-duplicates while preserving first-seen order. A non-string
/// entry violates the schema's `items: string` contract, so the whole call is
/// rejected (`Err`) instead of silently polling a subset — otherwise a malformed
/// `{"task_ids":[123,"abc"]}` would quietly resolve to just `abc`. `Ok(empty)`
/// means nothing usable was supplied (missing array, or all empty/whitespace);
/// the caller rejects both `Err` and `Ok(empty)` with `-32602`. Empty strings are
/// dropped (not rejected): `items` carries no `minLength`, so `""` satisfies the
/// schema and is treated as a formatting nicety. No upper bound on the count: a
/// fan-out can be arbitrarily wide.
fn normalize_status_task_ids(arguments: &Value) -> Result<Vec<String>, String> {
    let Some(arr) = arguments.get("task_ids").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for v in arr {
        let Some(s) = v.as_str() else {
            return Err(
                "get_delegation_status task_ids must contain only string task ids".to_string(),
            );
        };
        let trimmed = s.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    Ok(out)
}

/// Render the `get_delegation_status` round-trip outcome (always a
/// `{ "tasks": [..] }` envelope from the broker) into an MCP `tools/call`
/// result. EVERY poll renders through [`render_batch_report`] — a single id and
/// a fan-out take the SAME path — so the shape the LLM and frontend see is
/// uniform: a `{ "tasks": [..] }` object with one entry per requested id (one
/// entry for a single id), each carrying its `task_id` + `status`. A bare report
/// with no `tasks` array (older / unexpected shape) is wrapped as a one-element
/// batch so the output stays uniform.
pub fn render_status_result(outcome: &Value) -> Value {
    match outcome.get("tasks").and_then(|v| v.as_array()) {
        Some(tasks) => render_batch_report(tasks),
        None => render_batch_report(std::slice::from_ref(outcome)),
    }
}

/// Render a `get_delegation_status` result as a `{ "tasks": [..] }` batch — the
/// single rendering path for every poll, whether it carries one report or many.
/// The `content` text is the compact `{ "tasks": [..] }` JSON so hosts that
/// persist only `CallToolResult.content` text (e.g. Claude Code) can still
/// recover every task; `structuredContent` carries the same shape for hosts that
/// keep it. `isError` is set only when EVERY task failed — a coarse signal (a
/// lone failed task therefore flags `isError`, matching the old single-report
/// behavior); the frontend derives per-task badges from the structured reports,
/// not from this flag.
fn render_batch_report(tasks: &[Value]) -> Value {
    let all_failed = !tasks.is_empty()
        && tasks
            .iter()
            .all(|t| t.get("status").and_then(|v| v.as_str()) == Some("failed"));
    let envelope = json!({ "tasks": tasks });
    let text = serde_json::to_string(&envelope).unwrap_or_else(|_| String::from("{\"tasks\":[]}"));
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": all_failed,
        "structuredContent": envelope,
    })
}

/// Map a serialized [`super::types::DelegationTaskReport`] into MCP `tools/call`
/// result content. Shared by `delegate_to_agent` and `cancel_delegation`, which
/// each resolve to a single report; `get_delegation_status` no longer uses this
/// path — it always renders via [`render_status_result`] / [`render_batch_report`].
/// Kept separate so unit tests can assert the mapping without a real socket.
///
/// The human-readable `content` text is the result for a `completed` task and
/// the `message` (status note / failure reason) otherwise. `isError` is set
/// ONLY for `failed` — `running` (ack), `canceled` (a successful cancel or a
/// canceled task), and `unknown` are all valid tool results the LLM should read
/// rather than treat as errors. The full report rides along in
/// `structuredContent` so the frontend can read `status` + the child ids.
/// Map the `check_user_feedback` round-trip outcome (a `{ count, feedback:[..] }`
/// envelope from the listener) into an MCP `tools/call` result.
///
/// The human-readable `content` text is the steering the LLM acts on: when
/// notes are present it frames them as high-priority user corrections and asks
/// the agent to adjust and acknowledge; when empty it says so plainly. The raw
/// envelope rides along in `structuredContent`. `isError` is always `false` — a
/// successful check with no feedback is a valid result, not an error.
pub fn render_feedback_result(outcome: &Value) -> Value {
    let count = outcome.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    let text = if count == 0 {
        "No new feedback from the user. Continue with your current plan.".to_string()
    } else {
        let mut s = format!(
            "The user sent {count} message(s) while you were working. Treat this as \
             high-priority steering: adjust your current approach to honor it now, and \
             briefly acknowledge what you changed.\n"
        );
        if let Some(notes) = outcome.get("feedback").and_then(|v| v.as_array()) {
            for (i, note) in notes.iter().enumerate() {
                let body = note.get("text").and_then(|v| v.as_str()).unwrap_or("");
                s.push_str(&format!("{}. {}\n", i + 1, body));
            }
        }
        s
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
        // Rebuild the structured payload from count + feedback only — the
        // listener's internal `_commit_ids` must not leak to the agent's host.
        "structuredContent": {
            "count": count,
            "feedback": outcome.get("feedback").cloned().unwrap_or_else(|| json!([])),
        },
    })
}

pub fn render_task_report(report: &Value) -> Value {
    let status = report.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let is_error = status == "failed";
    let report_str = |key: &str| {
        report
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    };
    let text = if status == "completed" {
        // Prefer the result text; fall back to `message` so the DB-fallback note
        // ("Result no longer cached; open child session N…") for an evicted
        // result isn't rendered as empty content.
        report_str("text")
            .or_else(|| report_str("message"))
            .unwrap_or("")
            .to_string()
    } else {
        report_str("message")
            .or_else(|| report_str("text"))
            .unwrap_or("")
            .to_string()
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
        "structuredContent": report.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CompanionContext {
        // Delegation-only by default so the existing delegation-focused tests
        // keep seeing exactly the three delegation tools.
        ctx_with(CompanionFeatures {
            delegation: true,
            feedback: false,
        })
    }

    fn ctx_with(features: CompanionFeatures) -> CompanionContext {
        CompanionContext {
            parent_connection_id: "p1".into(),
            socket_path: "/tmp/codeg-mcp-companion-test-nope.sock".into(),
            token: "tok".into(),
            features,
        }
    }

    async fn dispatch_for_test(line: &str) -> LineAction {
        dispatch_line(&ctx(), Arc::new(InflightCalls::new()), line).await
    }

    async fn dispatch_with_features(features: CompanionFeatures, line: &str) -> LineAction {
        dispatch_line(&ctx_with(features), Arc::new(InflightCalls::new()), line).await
    }

    fn unwrap_respond(action: LineAction) -> JsonRpcResponse {
        match action {
            LineAction::Respond(r) => r,
            LineAction::Spawn(_) => panic!("expected Respond, got Spawn"),
            LineAction::Silent => panic!("expected Respond, got Silent"),
        }
    }

    #[tokio::test]
    async fn initialize_returns_protocol_version() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "codeg-mcp");
    }

    #[tokio::test]
    async fn tools_list_returns_three_delegation_tools() {
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"delegate_to_agent"));
        assert!(names.contains(&"get_delegation_status"));
        assert!(names.contains(&"cancel_delegation"));
        // delegate_to_agent schema still enumerates all 6 agent types.
        let delegate = tools
            .iter()
            .find(|t| t["name"] == "delegate_to_agent")
            .unwrap();
        let agents = delegate["inputSchema"]["properties"]["agent_type"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(agents.len(), 6);
        // get_delegation_status takes a single id param — task_ids (required) —
        // plus wait_ms. The legacy single `task_id` param is gone.
        let status = tools
            .iter()
            .find(|t| t["name"] == "get_delegation_status")
            .unwrap();
        assert!(status["inputSchema"]["properties"]["task_id"].is_null());
        assert!(status["inputSchema"]["properties"]["task_ids"].is_object());
        assert!(status["inputSchema"]["properties"]["wait_ms"].is_object());
        let required = status["inputSchema"]["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "task_ids"));
    }

    #[tokio::test]
    async fn get_delegation_status_without_task_ids_rejected() {
        let line = r#"{
            "jsonrpc":"2.0",
            "id":11,
            "method":"tools/call",
            "params": { "name": "get_delegation_status", "arguments": {} }
        }"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32602);
        assert!(e.message.contains("task_ids"));
    }

    #[tokio::test]
    async fn notifications_initialized_produces_no_response() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let action = dispatch_for_test(line).await;
        assert!(matches!(action, LineAction::Silent));
    }

    #[tokio::test]
    async fn parse_error_returns_null_id_error() {
        let line = "not json";
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32700);
        assert!(e.message.contains("parse"));
        assert_eq!(resp.id, Value::Null);
    }

    #[tokio::test]
    async fn unknown_method_returns_32601() {
        let line = r#"{"jsonrpc":"2.0","id":9,"method":"resources/list"}"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32601);
    }

    #[tokio::test]
    async fn tools_call_with_unknown_tool_rejected_synchronously() {
        let line = r#"{
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params": {
                "name": "other_tool",
                "arguments": {},
                "_meta": {"tool_use_id": "tu1"}
            }
        }"#;
        let resp = unwrap_respond(dispatch_for_test(line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32602);
        assert!(e.message.contains("other_tool"));
    }

    #[tokio::test]
    async fn tools_call_registers_inflight_and_returns_spawn() {
        let inflight = Arc::new(InflightCalls::new());
        let line = r#"{
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params": {
                "name": "delegate_to_agent",
                "arguments": {"agent_type": "codex", "task": "x"}
            }
        }"#;
        let action = dispatch_line(&ctx(), inflight.clone(), line).await;
        match action {
            LineAction::Spawn(call) => {
                assert_eq!(call.request_id_key, request_id_key(&Value::from(4)));
            }
            _ => panic!("expected Spawn"),
        }
        // The inflight registry should now have an entry for id=4.
        let map = inflight.inner.lock().await;
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&request_id_key(&Value::from(4))));
    }

    #[tokio::test]
    async fn cancel_notification_fires_inflight_cancel_channel() {
        let inflight = Arc::new(InflightCalls::new());
        // Pre-seed an inflight entry with a known cancel_tx; verify the
        // notification handler trips it.
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        inflight
            .register(
                request_id_key(&Value::from(7)),
                InflightEntry {
                    external_handle: Some("h-7".into()),
                    cancel_tx,
                },
            )
            .await;

        let line = r#"{
            "jsonrpc":"2.0",
            "method":"notifications/cancelled",
            "params": {"requestId": 7, "reason": "user requested"}
        }"#;
        let action = dispatch_line(&ctx(), inflight.clone(), line).await;
        assert!(matches!(action, LineAction::Silent));
        // The cancel channel should now be tripped (best-effort
        // `client_cancel` to a bogus socket failed silently — that's fine).
        assert!(cancel_rx.try_recv().is_ok());
        // Entry has been pulled.
        let map = inflight.inner.lock().await;
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn cancel_for_unknown_request_id_is_silent_noop() {
        let inflight = Arc::new(InflightCalls::new());
        let line = r#"{
            "jsonrpc":"2.0",
            "method":"notifications/cancelled",
            "params": {"requestId": 999}
        }"#;
        let action = dispatch_line(&ctx(), inflight.clone(), line).await;
        assert!(matches!(action, LineAction::Silent));
        assert!(inflight.inner.lock().await.is_empty());
    }

    #[test]
    fn render_task_report_running_ack_is_not_error() {
        let report = json!({
            "task_id": "t1",
            "status": "running",
            "child_conversation_id": 42,
            "message": "running in background"
        });
        let rendered = render_task_report(&report);
        assert_eq!(rendered["isError"], false);
        assert_eq!(rendered["content"][0]["text"], "running in background");
        assert_eq!(rendered["structuredContent"]["status"], "running");
        assert_eq!(rendered["structuredContent"]["child_conversation_id"], 42);
    }

    #[test]
    fn render_task_report_completed_surfaces_text() {
        let report = json!({
            "task_id": "t1",
            "status": "completed",
            "child_conversation_id": 42,
            "text": "the result"
        });
        let rendered = render_task_report(&report);
        assert_eq!(rendered["isError"], false);
        assert_eq!(rendered["content"][0]["text"], "the result");
        assert_eq!(rendered["structuredContent"]["status"], "completed");
    }

    #[test]
    fn render_task_report_failed_is_error() {
        let report = json!({
            "status": "failed",
            "error_code": "spawn_failed",
            "message": "spawn failed: agent missing"
        });
        let rendered = render_task_report(&report);
        assert_eq!(rendered["isError"], true);
        assert_eq!(
            rendered["content"][0]["text"],
            "spawn failed: agent missing"
        );
        assert_eq!(rendered["structuredContent"]["error_code"], "spawn_failed");
    }

    #[test]
    fn render_task_report_canceled_is_not_error() {
        // A successful cancel (or a canceled task) is a valid result, not an
        // error the LLM should treat as a failure.
        let report = json!({
            "task_id": "t1",
            "status": "canceled",
            "error_code": "canceled",
            "message": "canceled: canceled by request"
        });
        let rendered = render_task_report(&report);
        assert_eq!(rendered["isError"], false);
        assert_eq!(rendered["structuredContent"]["status"], "canceled");
    }

    #[test]
    fn render_task_report_completed_without_text_falls_back_to_message() {
        // DB-fallback for an evicted completed result: status completed, no
        // text, only a message. The content must not be empty.
        let report = json!({
            "task_id": "t1",
            "status": "completed",
            "child_conversation_id": 7,
            "message": "Result no longer cached; open child session 7 for the full output."
        });
        let rendered = render_task_report(&report);
        assert_eq!(rendered["isError"], false);
        assert_eq!(
            rendered["content"][0]["text"],
            "Result no longer cached; open child session 7 for the full output."
        );
    }

    // -- Batch get_delegation_status normalization + rendering -------------

    #[tokio::test]
    async fn get_delegation_status_bare_task_id_now_rejected() {
        // The legacy single `task_id` param is gone: a bare `{task_id}` no longer
        // resolves to a poll — it's an empty task set and must be rejected,
        // steering the caller to `task_ids`.
        let line = json!({
            "jsonrpc": "2.0", "id": 20, "method": "tools/call",
            "params": { "name": "get_delegation_status", "arguments": { "task_id": "abc" } }
        })
        .to_string();
        let resp = unwrap_respond(dispatch_for_test(&line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32602);
        assert!(e.message.contains("task_ids"));
    }

    #[tokio::test]
    async fn get_delegation_status_accepts_task_ids_array() {
        let line = json!({
            "jsonrpc": "2.0", "id": 21, "method": "tools/call",
            "params": { "name": "get_delegation_status", "arguments": { "task_ids": ["a", "b"] } }
        })
        .to_string();
        assert!(matches!(
            dispatch_for_test(&line).await,
            LineAction::Spawn(_)
        ));
    }

    #[tokio::test]
    async fn get_delegation_status_empty_task_ids_rejected() {
        // An absent, empty, or all-whitespace array yields no usable ids.
        for args in [json!({ "task_ids": [] }), json!({ "task_ids": ["  "] })] {
            let line = json!({
                "jsonrpc": "2.0", "id": 22, "method": "tools/call",
                "params": { "name": "get_delegation_status", "arguments": args }
            })
            .to_string();
            let resp = unwrap_respond(dispatch_for_test(&line).await);
            let e = resp.error.expect("empty task_ids must be rejected");
            assert_eq!(e.code, -32602);
            assert!(e.message.contains("task_ids"));
        }
    }

    #[tokio::test]
    async fn get_delegation_status_non_string_task_id_rejected() {
        // A non-string entry violates the schema's `items: string` contract — the
        // whole call is rejected, NOT silently narrowed to the valid ids. Both a
        // lone non-string and a mixed `[123, "abc"]` must fail.
        for args in [
            json!({ "task_ids": [123] }),
            json!({ "task_ids": [123, "abc"] }),
        ] {
            let line = json!({
                "jsonrpc": "2.0", "id": 23, "method": "tools/call",
                "params": { "name": "get_delegation_status", "arguments": args }
            })
            .to_string();
            let resp = unwrap_respond(dispatch_for_test(&line).await);
            let e = resp
                .error
                .expect("non-string task_ids entry must be rejected");
            assert_eq!(e.code, -32602);
            assert!(e.message.contains("task_ids"));
        }
    }

    #[test]
    fn normalize_status_task_ids_dedups_preserves_order() {
        // Trim each entry, drop "", collapse the duplicate "a", keep first-seen
        // order.
        let args = json!({ "task_ids": [" a ", "b", "a", "", "c"] });
        assert_eq!(
            normalize_status_task_ids(&args).unwrap(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn normalize_status_task_ids_rejects_non_string_entry() {
        // A non-string survivor alongside valid ids is a hard error, not a
        // silent drop.
        assert!(normalize_status_task_ids(&json!({ "task_ids": [123] })).is_err());
        assert!(normalize_status_task_ids(&json!({ "task_ids": ["a", 123] })).is_err());
        assert!(normalize_status_task_ids(&json!({ "task_ids": [true] })).is_err());
    }

    #[test]
    fn normalize_status_task_ids_empty_when_none_usable() {
        // Missing, empty, and all-blank arrays all yield no ids; a bare legacy
        // `task_id` is no longer read. (These are `Ok(empty)`, not errors.)
        assert!(normalize_status_task_ids(&json!({})).unwrap().is_empty());
        assert!(normalize_status_task_ids(&json!({ "task_ids": [] }))
            .unwrap()
            .is_empty());
        assert!(normalize_status_task_ids(&json!({ "task_ids": ["  "] }))
            .unwrap()
            .is_empty());
        assert!(normalize_status_task_ids(&json!({ "task_id": "abc" }))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn render_status_result_single_renders_as_one_element_batch() {
        // A single-id poll now renders through the SAME `{tasks:[..]}` envelope as
        // a fan-out (unified shape) — NOT the bare single-report path. The
        // structured batch carries the one task with its id + status, and the
        // content text is the `{tasks:[..]}` JSON (not the bare result text).
        let report = json!({
            "task_id": "t1", "status": "completed",
            "child_conversation_id": 42, "text": "the result"
        });
        let rendered = render_status_result(&json!({ "tasks": [report.clone()] }));
        let tasks = rendered["structuredContent"]["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "t1");
        assert_eq!(tasks[0]["status"], "completed");
        // Content text is the compact {tasks:[..]} JSON, recoverable by
        // content-only hosts — not the raw "the result" string.
        let text = rendered["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["tasks"][0]["text"], "the result");
        assert_eq!(rendered["isError"], false);
    }

    #[test]
    fn render_status_result_bare_report_wrapped_as_one_element_batch() {
        // Defensive: an outcome with no `tasks` array (older / unexpected shape) is
        // wrapped into a one-element batch so the output stays uniformly
        // `{tasks:[..]}`. A lone failed task flags `isError` (all-failed).
        let report = json!({
            "task_id": "t1", "status": "failed",
            "error_code": "spawn_failed", "message": "spawn failed"
        });
        let rendered = render_status_result(&report);
        let tasks = rendered["structuredContent"]["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "t1");
        assert_eq!(tasks[0]["status"], "failed");
        assert_eq!(rendered["isError"], true);
    }

    #[test]
    fn render_batch_report_carries_tasks_and_parseable_text() {
        let envelope = json!({ "tasks": [
            { "task_id": "t1", "status": "completed", "text": "r1" },
            { "task_id": "t2", "status": "running", "message": "Running." },
        ] });
        let rendered = render_status_result(&envelope);
        // structuredContent carries the whole batch.
        assert_eq!(
            rendered["structuredContent"]["tasks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // The content text is the compact {tasks:[..]} JSON, recoverable by hosts
        // that persist only CallToolResult.content text (e.g. Claude Code).
        let text = rendered["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["tasks"][0]["task_id"], "t1");
        assert_eq!(parsed["tasks"][1]["status"], "running");
        // Mixed statuses → not all failed → not flagged as an error.
        assert_eq!(rendered["isError"], false);
    }

    #[test]
    fn render_batch_report_is_error_only_when_all_failed() {
        let all_failed = json!({ "tasks": [
            { "task_id": "t1", "status": "failed", "message": "x" },
            { "task_id": "t2", "status": "failed", "message": "y" },
        ] });
        assert_eq!(render_status_result(&all_failed)["isError"], true);
        let mixed = json!({ "tasks": [
            { "task_id": "t1", "status": "failed" },
            { "task_id": "t2", "status": "canceled" },
        ] });
        assert_eq!(render_status_result(&mixed)["isError"], false);
    }

    // -- check_user_feedback feature gating + rendering --------------------

    const FEEDBACK_ONLY: CompanionFeatures = CompanionFeatures {
        delegation: false,
        feedback: true,
    };
    const BOTH: CompanionFeatures = CompanionFeatures {
        delegation: true,
        feedback: true,
    };

    fn list_tool_names(action: LineAction) -> Vec<String> {
        let resp = unwrap_respond(action);
        resp.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn features_parse_defaults_and_tokens() {
        // Absent → delegation-only (backward compatible).
        let def = CompanionFeatures::parse(None);
        assert!(def.delegation && !def.feedback);
        // Explicit list, whitespace + unknown tokens tolerated.
        let both = CompanionFeatures::parse(Some(" delegation , feedback ,bogus"));
        assert!(both.delegation && both.feedback);
        let fb = CompanionFeatures::parse(Some("feedback"));
        assert!(!fb.delegation && fb.feedback);
        // Empty string → nothing enabled.
        let none = CompanionFeatures::parse(Some(""));
        assert!(!none.delegation && !none.feedback);
    }

    #[tokio::test]
    async fn tools_list_hides_feedback_when_disabled() {
        // Default ctx is delegation-only: check_user_feedback must not appear.
        let names = list_tool_names(
            dispatch_for_test(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).await,
        );
        assert!(!names.contains(&"check_user_feedback".to_string()));
        assert_eq!(names.len(), 3);
    }

    #[tokio::test]
    async fn tools_list_includes_feedback_when_enabled() {
        let names = list_tool_names(
            dispatch_with_features(BOTH, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).await,
        );
        assert!(names.contains(&"check_user_feedback".to_string()));
        assert_eq!(names.len(), 4);
    }

    #[tokio::test]
    async fn tools_list_feedback_only_hides_delegation_tools() {
        let names = list_tool_names(
            dispatch_with_features(
                FEEDBACK_ONLY,
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            )
            .await,
        );
        assert_eq!(names, vec!["check_user_feedback".to_string()]);
    }

    #[tokio::test]
    async fn check_user_feedback_spawns_when_enabled() {
        let line = json!({
            "jsonrpc": "2.0", "id": 30, "method": "tools/call",
            "params": { "name": "check_user_feedback", "arguments": {} }
        })
        .to_string();
        assert!(matches!(
            dispatch_with_features(FEEDBACK_ONLY, &line).await,
            LineAction::Spawn(_)
        ));
    }

    #[tokio::test]
    async fn check_user_feedback_rejected_as_unknown_when_feature_off() {
        // Delegation-only ctx: the feedback tool is indistinguishable from a
        // nonexistent one (-32602 unknown tool), not a "disabled" leak.
        let line = json!({
            "jsonrpc": "2.0", "id": 31, "method": "tools/call",
            "params": { "name": "check_user_feedback", "arguments": {} }
        })
        .to_string();
        let resp = unwrap_respond(dispatch_for_test(&line).await);
        let e = resp.error.unwrap();
        assert_eq!(e.code, -32602);
        assert!(e.message.contains("unknown tool"));
    }

    #[tokio::test]
    async fn delegate_rejected_as_unknown_when_delegation_off() {
        // Feedback-only ctx: delegation tools are hidden + rejected uniformly.
        let line = json!({
            "jsonrpc": "2.0", "id": 32, "method": "tools/call",
            "params": { "name": "delegate_to_agent", "arguments": {"agent_type":"codex","task":"x"} }
        })
        .to_string();
        let resp = unwrap_respond(dispatch_with_features(FEEDBACK_ONLY, &line).await);
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn render_feedback_empty_is_not_error_and_says_no_feedback() {
        let rendered = render_feedback_result(&json!({ "count": 0, "feedback": [] }));
        assert_eq!(rendered["isError"], false);
        let text = rendered["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("No new feedback"));
        assert_eq!(rendered["structuredContent"]["count"], 0);
    }

    #[test]
    fn render_feedback_lists_notes_as_high_priority_steering() {
        let outcome = json!({
            "count": 2,
            "feedback": [
                { "text": "use the existing UserService", "created_at": "2026-06-07T00:00:00Z" },
                { "text": "skip the migration", "created_at": "2026-06-07T00:00:01Z" },
            ]
        });
        let rendered = render_feedback_result(&outcome);
        assert_eq!(rendered["isError"], false);
        let text = rendered["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("high-priority steering"));
        assert!(text.contains("1. use the existing UserService"));
        assert!(text.contains("2. skip the migration"));
        // Structured payload carries the notes for hosts that keep it.
        assert_eq!(rendered["structuredContent"]["count"], 2);
    }

    #[test]
    fn render_feedback_strips_internal_commit_ids() {
        // The listener embeds `_commit_ids` for the companion to echo back; they
        // must NEVER leak into the agent-facing result (content or structured).
        let outcome = json!({
            "count": 1,
            "feedback": [{ "text": "note", "created_at": "2026-06-07T00:00:00Z" }],
            "_commit_ids": ["secret-id-1"],
        });
        let rendered = render_feedback_result(&outcome);
        assert!(rendered["structuredContent"].get("_commit_ids").is_none());
        assert_eq!(rendered["structuredContent"]["count"], 1);
        assert_eq!(rendered["structuredContent"]["feedback"][0]["text"], "note");
        let text = rendered["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("secret-id-1"));
    }

    // -- commit-on-delivery protocol (the at-least-once delivery guarantee) ---

    #[cfg(unix)]
    fn feedback_resp_with_ids(ids: &[&str]) -> BrokerResponse {
        BrokerResponse {
            outcome: json!({
                "count": 1,
                "feedback": [{ "text": "steer", "created_at": "x" }],
                "_commit_ids": ids,
            }),
        }
    }

    /// When the round-trip wins (no cancel), the companion COMMITS delivery by
    /// sending a `CommitFeedback` with the listener's `_commit_ids`.
    #[cfg(unix)]
    #[tokio::test]
    async fn feedback_spawn_commits_after_delivery() {
        use crate::acp::delegation::transport::{read_frame, write_frame, BrokerMessage};
        use tokio::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("fb.sock").to_string_lossy().to_string();
        let listener = UnixListener::bind(&sock).unwrap();
        let committed = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let committed2 = committed.clone();
        let server = tokio::spawn(async move {
            // 1) Feedback round-trip → respond with notes + _commit_ids.
            let (mut c1, _) = listener.accept().await.unwrap();
            let _: BrokerResponse = match read_frame::<_, BrokerMessage>(&mut c1).await.unwrap() {
                BrokerMessage::Feedback(_) => {
                    write_frame(&mut c1, &feedback_resp_with_ids(&["f1"])).await.unwrap();
                    BrokerResponse { outcome: Value::Null }
                }
                other => panic!("expected Feedback, got {other:?}"),
            };
            // 2) CommitFeedback → record the ids.
            let (mut c2, _) = listener.accept().await.unwrap();
            if let BrokerMessage::CommitFeedback(req) = read_frame(&mut c2).await.unwrap() {
                committed2.lock().await.push(req.ids);
            }
            write_frame(&mut c2, &BrokerResponse { outcome: Value::Null }).await.unwrap();
        });

        let inflight = Arc::new(InflightCalls::new());
        let action = register_and_spawn_feedback(
            inflight,
            Value::from(1),
            sock,
            "tok".into(),
            BrokerFeedbackRequest { token: "tok".into() },
        )
        .await;
        let LineAction::Spawn(call) = action else {
            panic!("expected Spawn")
        };
        let result = call.future.await;
        let resp = result.response.expect("feedback result");
        assert_eq!(resp.result.unwrap()["structuredContent"]["count"], 1);
        // The commit is deferred to `after_relay`, which the binary runs ONLY
        // after a successful stdout write — drive it here to simulate that relay.
        result
            .after_relay
            .expect("feedback must carry a post-relay commit")
            .await;
        server.await.unwrap();
        assert_eq!(*committed.lock().await, vec![vec!["f1".to_string()]]);
    }

    /// When a cancel wins the select, the companion suppresses the response AND
    /// sends NO commit — so the notes stay pending for the next check.
    #[cfg(unix)]
    #[tokio::test]
    async fn feedback_spawn_cancel_sends_no_commit() {
        use crate::acp::delegation::transport::{read_frame, write_frame, BrokerMessage};
        use tokio::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("fb.sock").to_string_lossy().to_string();
        let listener = UnixListener::bind(&sock).unwrap();
        let saw_commit = Arc::new(Mutex::new(false));
        let saw_commit2 = saw_commit.clone();
        let server = tokio::spawn(async move {
            // Accept the Feedback connection but DELAY responding, so the cancel
            // (fired below) wins the select first.
            if let Ok((mut c1, _)) = listener.accept().await {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let _ = write_frame(&mut c1, &feedback_resp_with_ids(&["f1"])).await;
            }
            // A commit (if any) would arrive as a second connection. Wait briefly;
            // a timeout (no connection) is the expected, correct outcome.
            if let Ok(Ok((mut c2, _))) =
                tokio::time::timeout(Duration::from_millis(200), listener.accept()).await
            {
                if matches!(
                    read_frame::<_, BrokerMessage>(&mut c2).await,
                    Ok(BrokerMessage::CommitFeedback(_))
                ) {
                    *saw_commit2.lock().await = true;
                }
            }
        });

        let ctx = CompanionContext {
            parent_connection_id: "p".into(),
            socket_path: sock,
            token: "tok".into(),
            features: FEEDBACK_ONLY,
        };
        let inflight = Arc::new(InflightCalls::new());
        // tools/call → Spawn (registers the inflight entry).
        let call_line = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "check_user_feedback", "arguments": {} }
        })
        .to_string();
        let action = dispatch_line(&ctx, inflight.clone(), &call_line).await;
        let LineAction::Spawn(call) = action else {
            panic!("expected Spawn")
        };
        // Cancel for the same id BEFORE the (delayed) response arrives.
        let cancel_line =
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1 } })
                .to_string();
        assert!(matches!(
            dispatch_line(&ctx, inflight.clone(), &cancel_line).await,
            LineAction::Silent
        ));
        // Cancel won → response suppressed AND no post-relay commit exists.
        let result = call.future.await;
        assert!(
            result.response.is_none(),
            "cancel must suppress the response"
        );
        assert!(
            result.after_relay.is_none(),
            "a suppressed response carries no commit"
        );
        server.abort();
        // Crucially: no commit was sent for a cancelled (undelivered) check.
        assert!(!*saw_commit.lock().await, "a cancelled check must not commit");
    }
}
