//! Wire format for `codeg-mcp` companion ↔ main process round-trip over UDS
//! (Unix) or named pipe (Windows).
//!
//! The frame is dead simple: a little-endian `u32` byte length followed by
//! that many bytes of UTF-8 JSON. One request, one response — the companion
//! reopens the socket per `tools/call`. This trades a few extra connects for
//! a wire that's trivial to test and that doesn't need multiplexing
//! (a parent makes at most one delegation call at a time from the LLM's
//! perspective — the broker handles concurrency at a higher level).
//!
//! Why length-prefix instead of newline-delimited JSON? The LLM-issued
//! `task` arguments can contain newlines, and we'd rather avoid escaping
//! them into a single line. JSON-RPC over stdio uses newlines because
//! Content-Length headers add complexity; for an internal UDS we can do
//! better.
//!
//! ### Message shapes
//!
//! Inbound traffic is a tagged [`BrokerMessage`] enum, one variant per MCP
//! tool plus the MCP cancel notification:
//!   * `call` — [`BrokerRequest`] for `delegate_to_agent`; returns a
//!     [`BrokerResponse`] wrapping a `DelegationTaskReport` (a `Running` ack, or
//!     a terminal report).
//!   * `status` — [`BrokerStatusRequest`] for `get_delegation_status`. Carries a
//!     `task_ids` list (one or many) and an optional `wait_ms` long-poll —
//!     omitted is an immediate snapshot, an explicit `0` blocks until a task is
//!     terminal, a positive value is a bounded wait. Returns a `{ "tasks": [..] }`
//!     envelope with one task report per requested id (in request order); a
//!     batch wait wakes as soon as ANY requested task reaches a terminal state.
//!   * `cancel_task` — [`BrokerCancelTaskRequest`] for `cancel_delegation`;
//!     returns a task report.
//!   * `cancel` — fire-and-forget [`BrokerCancelRequest`] from MCP
//!     `notifications/cancelled`, targeting an in-flight `delegate_to_agent`
//!     call by `external_handle`; gets a `Value::Null` ack.
//!
//! All arms are authenticated by the same per-launch `token`.
//!
//! ### Version coupling
//!
//! The companion (`codeg-mcp`) and the listener (inside the codeg main
//! process) ship in the SAME release artifact — the Tauri bundle, the
//! server Docker image, and the standalone binary tree all install both
//! binaries at the same path. The MCP config pointing the agent CLI at
//! `codeg-mcp` uses an absolute path that is replaced atomically by the
//! upgrade, so an old-version companion talking to a new-version listener
//! is not a supported configuration. As a consequence this protocol does
//! NOT carry a version field and the tagged-enum cutover from the older
//! plain-`BrokerRequest` frame is deliberately non-backward-compatible —
//! a stale companion would fail to decode and surface as a JSON-RPC
//! error to the LLM, which is preferable to silent misbehavior.

use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One delegation call's worth of input forwarded from the companion to the
/// main process. The main process re-validates `token` and maps
/// `parent_connection_id` to the live ACP connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerRequest {
    /// Shared secret minted by the main process when it spawned the agent CLI;
    /// the agent passes it through to the companion via `--token`. Rejects
    /// anything else.
    pub token: String,
    /// codeg-internal ACP connection UUID for the parent session.
    pub parent_connection_id: String,
    /// The MCP `tool_use_id` for the LLM-issued `delegate_to_agent` call.
    /// Used to bind the eventual child outcome back to the parent's
    /// tool_use_id in the UI / DB.
    pub parent_tool_use_id: String,
    /// Opaque companion-minted token (one per `tools/call`). The broker
    /// keys its `cancel_by_external_handle` lookup off this value so an
    /// MCP-side `notifications/cancelled` can target this specific call.
    /// Older companions / tests can omit it; missing handles disable the
    /// cancel path for that call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_handle: Option<String>,
    /// Raw `arguments` JSON from the MCP `tools/call` request, schema-shaped
    /// per [`super::tool_schema_json`]. The main process re-parses into
    /// [`super::types::DelegationRequest`].
    pub input: Value,
}

/// Cancel an in-flight delegation by its companion-minted
/// `external_handle`. Sent fire-and-forget — the listener acknowledges by
/// writing an empty [`BrokerResponse`] so the companion can detect a
/// broken socket, but the response body carries no information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerCancelRequest {
    pub token: String,
    pub external_handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Query the status (and, optionally, block briefly for the result) of one or
/// more previously-issued delegation tasks by their broker `task_id`s. Backs the
/// `get_delegation_status` MCP tool. Authenticated by the same per-launch
/// `token`; the listener scopes each lookup to the token's parent connection
/// so one parent can't read another's tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerStatusRequest {
    pub token: String,
    /// One or many task ids to resolve. The companion forwards the MCP
    /// `task_ids` array into this list (trimmed, de-duplicated, order-preserving).
    /// The listener returns one report per id, in this order.
    pub task_ids: Vec<String>,
    /// How long the listener may block waiting for a task to reach a terminal
    /// state before returning the current (possibly still-running) snapshot.
    /// `None` (omitted) returns an immediate snapshot; an explicit `0` blocks
    /// with no timeout until a task finishes (long-running children); any
    /// positive value is a long-poll the listener clamps to a hard ceiling so a
    /// single bounded call can't hang unbounded. For a batch the wait resolves as
    /// soon as ANY requested task reaches a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
}

/// Cancel a previously-issued delegation task by its broker `task_id`. Backs
/// the `cancel_delegation` MCP tool. Distinct from [`BrokerCancelRequest`],
/// which targets an in-flight `tools/call` by its companion-minted
/// `external_handle` for MCP `notifications/cancelled`; this targets a running
/// task the LLM is explicitly stopping by id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerCancelTaskRequest {
    pub token: String,
    pub task_id: String,
}

/// Pull the pending live-feedback notes for the parent session. Backs the
/// `check_user_feedback` MCP tool. Authenticated by the same per-launch
/// `token`; the listener resolves the parent connection from it and scopes the
/// drain to that connection so one parent can't read another's feedback.
/// Always returns an immediate snapshot — no blocking wait.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerFeedbackRequest {
    pub token: String,
}

/// Confirm delivery of feedback notes, marking them `Delivered`. Sent by the
/// companion AFTER its `check_user_feedback` round-trip wins (i.e. it is
/// returning the result to the agent), NOT by the listener at UDS-write time —
/// so a per-request cancel that suppresses the agent-facing response (the agent
/// staying alive) leaves the notes pending for the next check (at-least-once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerCommitFeedbackRequest {
    pub token: String,
    pub ids: Vec<String>,
}

/// Tagged top-level message dispatched by the listener. Adding new variants
/// is the wire-stable way to grow the broker protocol without touching the
/// frame layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrokerMessage {
    Call(BrokerRequest),
    Cancel(BrokerCancelRequest),
    Status(BrokerStatusRequest),
    CancelTask(BrokerCancelTaskRequest),
    Feedback(BrokerFeedbackRequest),
    CommitFeedback(BrokerCommitFeedbackRequest),
}

/// The wrapped outcome the main process returns over the same socket.
/// `outcome` is a serialized [`super::types::DelegationTaskReport`] for `Call`
/// / `CancelTask` messages, a `{ "tasks": [report, ...] }` envelope (one report
/// per requested id, in request order) for `Status`, and `Value::Null` for
/// `Cancel` acknowledgements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerResponse {
    pub outcome: Value,
}

/// Maximum allowed frame size, 16 MiB. Guards against a misbehaving peer
/// allocating gigabytes when reading the length prefix.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Write one length-prefixed JSON frame.
pub async fn write_frame<W, T>(stream: &mut W, value: &T) -> io::Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("encode: {e}")))?;
    let len: u32 = bytes
        .len()
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame > u32::MAX"))?;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

/// Read one length-prefixed JSON frame. Rejects frames larger than
/// [`MAX_FRAME_BYTES`].
pub async fn read_frame<R, T>(stream: &mut R) -> io::Result<T>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame {len} bytes exceeds cap {MAX_FRAME_BYTES}"),
        ));
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("decode: {e}")))
}

/// One-shot client round-trip: connect, write one [`BrokerMessage`], read the
/// response, drop the connection. The three public helpers below differ only
/// in which message they build, so the connect/write/read is shared here.
#[cfg(unix)]
async fn message_round_trip(socket_path: &str, msg: &BrokerMessage) -> io::Result<BrokerResponse> {
    use tokio::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path).await?;
    write_frame(&mut stream, msg).await?;
    read_frame(&mut stream).await
}

/// Windows path uses named pipes; the address format is `\\.\pipe\<name>`.
#[cfg(windows)]
async fn message_round_trip(socket_path: &str, msg: &BrokerMessage) -> io::Result<BrokerResponse> {
    let mut stream = open_named_pipe_with_retry(socket_path)
        .await
        .map_err(|e| io::Error::other(format!("open pipe: {e}")))?;
    write_frame(&mut stream, msg).await?;
    read_frame(&mut stream).await
}

/// Dispatch a `delegate_to_agent` call and read back the broker's
/// [`super::types::DelegationTaskReport`] (a `Running` ack, or a terminal
/// report when the child finished during setup / setup failed).
pub async fn client_round_trip(
    socket_path: &str,
    req: &BrokerRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(socket_path, &BrokerMessage::Call(req.clone())).await
}

/// Dispatch a `get_delegation_status` query and read back the
/// `{ "tasks": [report, ...] }` envelope (one report per requested id, in
/// request order).
pub async fn client_status_round_trip(
    socket_path: &str,
    req: &BrokerStatusRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(socket_path, &BrokerMessage::Status(req.clone())).await
}

/// Dispatch a `cancel_delegation` request and read back the task report.
pub async fn client_cancel_task_round_trip(
    socket_path: &str,
    req: &BrokerCancelTaskRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(socket_path, &BrokerMessage::CancelTask(req.clone())).await
}

/// Dispatch a `check_user_feedback` query and read back the
/// `{ "feedback": [..], "count": N }` envelope (the pending notes drained for
/// the parent session, possibly empty).
pub async fn client_feedback_round_trip(
    socket_path: &str,
    req: &BrokerFeedbackRequest,
) -> io::Result<BrokerResponse> {
    message_round_trip(socket_path, &BrokerMessage::Feedback(req.clone())).await
}

/// Confirm delivery of feedback notes (fire-and-forget). Reads the empty ack so
/// the listener can flush before the socket drops; the body carries nothing.
pub async fn client_commit_feedback(
    socket_path: &str,
    req: &BrokerCommitFeedbackRequest,
) -> io::Result<()> {
    let _ = message_round_trip(socket_path, &BrokerMessage::CommitFeedback(req.clone())).await?;
    Ok(())
}

/// Total budget for `open()` retries on Windows named pipes. Has to be
/// short enough that it nests comfortably inside the companion's
/// `BROKER_CANCEL_BUDGET` (500 ms) — leaving ≥ 300 ms for the actual
/// write/read after the open lands.
#[cfg(windows)]
const PIPE_OPEN_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_millis(200);

#[cfg(windows)]
const PIPE_OPEN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(10);

/// Windows-only: `ClientOptions::open()` can fail with
/// `ERROR_PIPE_BUSY` (231) or `NotFound` during the brief window between
/// the listener accepting one connection and binding the next instance
/// (see `DelegationListener::run` on Windows). The companion has already
/// removed the inflight entry by the time it dispatches a cancel, so
/// dropping the cancel on a transient open failure would silently lose
/// it. Retry with small backoff inside a tight budget. Non-busy errors
/// (e.g. listener not running at all) propagate immediately.
#[cfg(windows)]
async fn open_named_pipe_with_retry(
    socket_path: &str,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    let attempt = async {
        loop {
            match ClientOptions::new().open(socket_path) {
                Ok(client) => return Ok::<_, io::Error>(client),
                Err(e) => {
                    let busy = e.raw_os_error() == Some(231);
                    let not_found = e.kind() == io::ErrorKind::NotFound;
                    if !(busy || not_found) {
                        return Err(e);
                    }
                    tokio::time::sleep(PIPE_OPEN_RETRY_DELAY).await;
                }
            }
        }
    };
    match tokio::time::timeout(PIPE_OPEN_RETRY_BUDGET, attempt).await {
        Ok(inner) => inner,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "named pipe open: retry budget exhausted",
        )),
    }
}

/// Fire-and-forget cancel: open a fresh socket, write a
/// `BrokerMessage::Cancel`, read the (always-empty) ack so the listener gets
/// a chance to flush its side before we drop, then close. Errors are
/// returned but generally treated as "best effort" by callers — a cancel
/// race that loses to a completed response is fine, the companion will
/// suppress the response per MCP spec either way.
#[cfg(unix)]
pub async fn client_cancel(socket_path: &str, req: &BrokerCancelRequest) -> io::Result<()> {
    use tokio::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path).await?;
    let msg = BrokerMessage::Cancel(req.clone());
    write_frame(&mut stream, &msg).await?;
    // The listener writes an empty BrokerResponse so we can detect a broken
    // pipe; we don't care what's inside.
    let _: io::Result<BrokerResponse> = read_frame(&mut stream).await;
    Ok(())
}

#[cfg(windows)]
pub async fn client_cancel(socket_path: &str, req: &BrokerCancelRequest) -> io::Result<()> {
    let mut stream = open_named_pipe_with_retry(socket_path)
        .await
        .map_err(|e| io::Error::other(format!("open pipe: {e}")))?;
    let msg = BrokerMessage::Cancel(req.clone());
    write_frame(&mut stream, &msg).await?;
    let _: io::Result<BrokerResponse> = read_frame(&mut stream).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::duplex;

    #[tokio::test]
    async fn frame_round_trip_in_memory() {
        let (mut a, mut b) = duplex(8 * 1024);
        let msg = BrokerMessage::Call(BrokerRequest {
            token: "tok".into(),
            parent_connection_id: "p1".into(),
            parent_tool_use_id: "pt1".into(),
            external_handle: Some("h1".into()),
            input: json!({"agent_type": "codex", "task": "hi"}),
        });
        write_frame(&mut a, &msg).await.unwrap();
        let got: BrokerMessage = read_frame(&mut b).await.unwrap();
        match got {
            BrokerMessage::Call(req) => {
                assert_eq!(req.token, "tok");
                assert_eq!(req.input["agent_type"], "codex");
                assert_eq!(req.external_handle.as_deref(), Some("h1"));
            }
            other => panic!("expected Call variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_message_round_trip_in_memory() {
        let (mut a, mut b) = duplex(8 * 1024);
        let msg = BrokerMessage::Cancel(BrokerCancelRequest {
            token: "tok".into(),
            external_handle: "h1".into(),
            reason: Some("user requested".into()),
        });
        write_frame(&mut a, &msg).await.unwrap();
        let got: BrokerMessage = read_frame(&mut b).await.unwrap();
        match got {
            BrokerMessage::Cancel(req) => {
                assert_eq!(req.token, "tok");
                assert_eq!(req.external_handle, "h1");
                assert_eq!(req.reason.as_deref(), Some("user requested"));
            }
            other => panic!("expected Cancel variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_oversized_frame() {
        let (mut a, mut b) = duplex(8);
        // Write a length prefix larger than the cap, no body.
        let bad_len: u32 = (MAX_FRAME_BYTES as u32) + 1;
        a.write_all(&bad_len.to_le_bytes()).await.unwrap();
        a.flush().await.unwrap();
        let result: io::Result<BrokerMessage> = read_frame(&mut b).await;
        let err = result.expect_err("expected oversized frame to be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn named_pipe_round_trip() {
        use tokio::net::windows::named_pipe::ServerOptions;

        // PID + nanosecond suffix keeps the pipe name unique across parallel
        // tests and avoids collisions with a live listener on the same box.
        let pipe_name = format!(
            r"\\.\pipe\codeg-mcp-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .unwrap();

        let server_pipe = pipe_name.clone();
        let server_task = tokio::spawn(async move {
            let mut conn = server;
            conn.connect().await.unwrap();
            let msg: BrokerMessage = read_frame(&mut conn).await.unwrap();
            match msg {
                BrokerMessage::Call(req) => assert_eq!(req.token, "tok"),
                other => panic!("expected Call, got {other:?}"),
            }
            let resp = BrokerResponse {
                outcome: json!({"kind": "ok", "text": "hello"}),
            };
            write_frame(&mut conn, &resp).await.unwrap();
            // Silence "unused" — server name is captured for clarity.
            let _ = server_pipe;
        });

        let req = BrokerRequest {
            token: "tok".into(),
            parent_connection_id: "p1".into(),
            parent_tool_use_id: "pt1".into(),
            external_handle: None,
            input: json!({"agent_type": "codex", "task": "do x"}),
        };
        let resp = client_round_trip(&pipe_name, &req).await.unwrap();
        assert_eq!(resp.outcome["kind"], "ok");
        assert_eq!(resp.outcome["text"], "hello");
        server_task.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uds_round_trip() {
        use tokio::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codeg-mcp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server_path = path.to_string_lossy().to_string();

        let server = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let msg: BrokerMessage = read_frame(&mut conn).await.unwrap();
            match msg {
                BrokerMessage::Call(req) => assert_eq!(req.token, "tok"),
                other => panic!("expected Call, got {other:?}"),
            }
            let resp = BrokerResponse {
                outcome: json!({"kind": "ok", "text": "hello"}),
            };
            write_frame(&mut conn, &resp).await.unwrap();
        });

        let req = BrokerRequest {
            token: "tok".into(),
            parent_connection_id: "p1".into(),
            parent_tool_use_id: "pt1".into(),
            external_handle: None,
            input: json!({"agent_type": "codex", "task": "do x"}),
        };
        let resp = client_round_trip(&server_path, &req).await.unwrap();
        assert_eq!(resp.outcome["kind"], "ok");
        assert_eq!(resp.outcome["text"], "hello");
        server.await.unwrap();
    }
}
