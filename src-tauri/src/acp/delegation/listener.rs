//! Main-process side of the `codeg-mcp` round-trip: accept UDS / named-pipe
//! connections from companion processes, validate the per-launch token,
//! resolve the parent's current conversation, and hand off to the broker.
//!
//! The listener is intentionally tiny — most of the work (depth checking,
//! spawn lifecycle, timeout, cancellation) happens inside
//! [`DelegationBroker`]. The listener is the boundary between the wire and
//! the broker, plus the place where the per-launch token policy is enforced.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

use crate::acp::delegation::broker::DelegationBroker;
use crate::acp::delegation::transport::{
    read_frame, write_frame, BrokerCancelRequest, BrokerMessage, BrokerRequest, BrokerResponse,
};
use crate::acp::delegation::types::{DelegationOutcome, DelegationRequest};
use crate::models::AgentType;
use serde_json::Value;

/// Pluggable "what conversation is this parent currently in?" lookup. The
/// production impl wraps `ConnectionManager.get_state`; tests use an
/// in-memory map.
///
/// Kept as a trait so the listener can be unit-tested without spinning up a
/// real `ConnectionManager` or RwLock<SessionState>.
#[async_trait]
pub trait ParentSessionLookup: Send + Sync {
    async fn current_conversation_id(&self, parent_connection_id: &str) -> Option<i32>;
}

/// Per-launch token entry. Bound at MCP injection time and revoked on parent
/// connection teardown.
#[derive(Debug, Clone)]
pub struct TokenEntry {
    pub parent_connection_id: String,
    pub working_dir: PathBuf,
}

#[derive(Default)]
pub struct TokenRegistry {
    inner: RwLock<HashMap<String, TokenEntry>>,
}

impl TokenRegistry {
    pub async fn register(&self, token: String, entry: TokenEntry) {
        self.inner.write().await.insert(token, entry);
    }

    pub async fn revoke(&self, token: &str) {
        self.inner.write().await.remove(token);
    }

    pub async fn lookup(&self, token: &str) -> Option<TokenEntry> {
        self.inner.read().await.get(token).cloned()
    }

    /// Drop every token whose `parent_connection_id` matches. Used on parent
    /// connection teardown so a leaked token can't be reused.
    pub async fn revoke_by_parent(&self, parent_connection_id: &str) {
        let mut map = self.inner.write().await;
        map.retain(|_, entry| entry.parent_connection_id != parent_connection_id);
    }
}

pub struct DelegationListener {
    pub broker: Arc<DelegationBroker>,
    pub tokens: Arc<TokenRegistry>,
    pub parent_lookup: Arc<dyn ParentSessionLookup>,
}

impl DelegationListener {
    pub fn new(
        broker: Arc<DelegationBroker>,
        tokens: Arc<TokenRegistry>,
        parent_lookup: Arc<dyn ParentSessionLookup>,
    ) -> Arc<Self> {
        Arc::new(Self {
            broker,
            tokens,
            parent_lookup,
        })
    }

    /// Run the accept loop until the socket is unbound. Errors on accept are
    /// logged and the loop continues — a single bad connection can't bring
    /// down the listener.
    #[cfg(unix)]
    pub async fn run(self: Arc<Self>, socket_path: PathBuf) -> std::io::Result<()> {
        let _ = tokio::fs::remove_file(&socket_path).await;
        if let Some(parent) = socket_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let listener = tokio::net::UnixListener::bind(&socket_path)?;
        eprintln!("[delegation] listening on UDS {}", socket_path.display());
        loop {
            match listener.accept().await {
                Ok((mut conn, _)) => {
                    let me = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(e) = me.serve_one(&mut conn).await {
                            eprintln!("[delegation] connection failed: {e}");
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[delegation] accept failed: {e}");
                    // Brief backoff so a persistent accept error doesn't pin a core.
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Windows variant: bind a named pipe and follow Tokio's recommended
    /// accept pattern — wait for a connect, immediately create the *next*
    /// server instance, then hand the connected instance off to a worker.
    /// This keeps a pipe instance available at all times, so clients calling
    /// `ClientOptions::open()` between connections don't see `NotFound`.
    #[cfg(windows)]
    pub async fn run(self: Arc<Self>, socket_path: PathBuf) -> std::io::Result<()> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let path_str = socket_path.to_string_lossy().to_string();
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&path_str)?;
        eprintln!("[delegation] listening on named pipe {path_str}");
        loop {
            if let Err(e) = server.connect().await {
                eprintln!("[delegation] connect failed: {e}");
                // Re-create the instance so the next iteration has a fresh
                // listener; a failed connect leaves the current one unusable.
                server = ServerOptions::new().create(&path_str)?;
                continue;
            }
            let connected = server;
            // Re-bind BEFORE serving the current client, so a client that
            // opens during this turn finds a server instance to connect to.
            server = ServerOptions::new().create(&path_str)?;
            let me = Arc::clone(&self);
            tokio::spawn(async move {
                let mut conn = connected;
                if let Err(e) = me.serve_one(&mut conn).await {
                    eprintln!("[delegation] connection failed: {e}");
                }
            });
        }
    }

    /// Stream-generic per-connection handler. Exposed so unit tests can drive
    /// it over `tokio::io::duplex` instead of a real socket.
    pub async fn serve_one<C>(&self, conn: &mut C) -> std::io::Result<()>
    where
        C: AsyncReadExt + AsyncWriteExt + Unpin,
    {
        let msg: BrokerMessage = read_frame(conn).await?;
        let resp = match msg {
            BrokerMessage::Call(req) => {
                let outcome = self.process(req).await;
                BrokerResponse {
                    outcome: serde_json::to_value(&outcome).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("encode: {e}"),
                        )
                    })?,
                }
            }
            BrokerMessage::Cancel(cancel) => {
                self.process_cancel(cancel).await;
                // Empty ack — the companion only uses this to detect the
                // listener has at least seen the cancel before dropping.
                BrokerResponse {
                    outcome: Value::Null,
                }
            }
        };
        write_frame(conn, &resp).await?;
        Ok(())
    }

    /// Validate token + dispatch cancel to the broker. Unknown tokens and
    /// parent-mismatched cancels are silently dropped — there's no LLM on
    /// the receiving end of this method to react to errors.
    async fn process_cancel(&self, cancel: BrokerCancelRequest) {
        let Some(_entry) = self.tokens.lookup(&cancel.token).await else {
            return;
        };
        let reason = cancel.reason.unwrap_or_else(|| "mcp client canceled".into());
        self.broker
            .cancel_by_external_handle(&cancel.external_handle, reason)
            .await;
    }

    async fn process(&self, req: BrokerRequest) -> DelegationOutcome {
        // 1. Token + parent_connection_id consistency check. Treat both as
        //    "canceled" since the LLM can't usefully react to either —
        //    the parent has either been torn down or is impersonating.
        let entry = match self.tokens.lookup(&req.token).await {
            Some(e) => e,
            None => return cancel("invalid token"),
        };
        if entry.parent_connection_id != req.parent_connection_id {
            return cancel("token does not match parent connection");
        }

        // 2. Resolve the parent's current conversation. Without one the
        //    broker can't link the child row to the parent.
        let parent_conversation_id = match self
            .parent_lookup
            .current_conversation_id(&req.parent_connection_id)
            .await
        {
            Some(id) => id,
            None => return cancel("parent has no active conversation"),
        };

        // 3. Parse the delegate_to_agent arguments. Schema validation lives
        //    on the LLM side; we only enforce what the broker can't.
        let agent_type = match req.input.get("agent_type").and_then(|v| v.as_str()) {
            Some(raw) => match parse_agent_type(raw) {
                Some(t) => t,
                None => return invalid_agent_type(raw),
            },
            None => return invalid_agent_type(""),
        };
        let task = match req.input.get("task").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => {
                return DelegationOutcome::Err {
                    code: "invalid_working_dir".into(),
                    message: "missing or empty task".into(),
                    child_conversation_id: None,
                }
            }
        };
        let working_dir = req
            .input
            .get("working_dir")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some(entry.working_dir.to_string_lossy().to_string()));

        let delegation_req = DelegationRequest {
            parent_connection_id: req.parent_connection_id,
            parent_conversation_id,
            parent_tool_use_id: req.parent_tool_use_id,
            agent_type,
            task,
            working_dir,
            external_handle: req.external_handle,
        };
        self.broker.handle_request(delegation_req).await
    }
}

fn cancel(message: &str) -> DelegationOutcome {
    DelegationOutcome::Err {
        code: "canceled".into(),
        message: message.into(),
        child_conversation_id: None,
    }
}

fn invalid_agent_type(raw: &str) -> DelegationOutcome {
    DelegationOutcome::Err {
        code: "invalid_agent_type".into(),
        message: if raw.is_empty() {
            "missing agent_type".into()
        } else {
            format!("invalid agent_type: {raw}")
        },
        child_conversation_id: None,
    }
}

fn parse_agent_type(raw: &str) -> Option<AgentType> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

/// Default socket path for the running process, scoped to PID so multiple
/// codeg instances on the same machine don't collide.
///
/// Unix: a `.sock` file inside `temp_dir`.
/// Windows: a named pipe address `\\.\pipe\codeg-delegation-<pid>`. Windows
/// named pipes live in their own kernel namespace and ignore `temp_dir`; the
/// argument is kept for signature parity across platforms.
#[cfg(unix)]
pub fn default_socket_path(temp_dir: &Path) -> PathBuf {
    temp_dir.join(format!("codeg-delegation-{}.sock", std::process::id()))
}

#[cfg(windows)]
pub fn default_socket_path(_temp_dir: &Path) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\codeg-delegation-{}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::broker::{ConversationDepthLookup, DelegationConfig};
    use crate::acp::delegation::spawner::{mock::MockSpawner, ConnectionSpawner, SpawnerError};
    use crate::acp::delegation::types::{DelegationError, DelegationSuccess};
    use serde_json::json;
    use std::time::Duration;
    use tokio::io::duplex;

    struct AlwaysRootLookup;
    #[async_trait]
    impl ConversationDepthLookup for AlwaysRootLookup {
        async fn parent_of(&self, _id: i32) -> Result<Option<i32>, DelegationError> {
            Ok(None)
        }
    }

    struct StaticParentLookup(Option<i32>);
    #[async_trait]
    impl ParentSessionLookup for StaticParentLookup {
        async fn current_conversation_id(&self, _parent_connection_id: &str) -> Option<i32> {
            self.0
        }
    }

    fn make_broker(mock: Arc<MockSpawner>) -> Arc<DelegationBroker> {
        Arc::new(DelegationBroker::new(
            mock as Arc<dyn ConnectionSpawner>,
            Arc::new(AlwaysRootLookup) as Arc<dyn ConversationDepthLookup>,
        ))
    }

    fn make_listener(
        broker: Arc<DelegationBroker>,
        tokens: Arc<TokenRegistry>,
        parent_conversation: Option<i32>,
    ) -> Arc<DelegationListener> {
        DelegationListener::new(
            broker,
            tokens,
            Arc::new(StaticParentLookup(parent_conversation)),
        )
    }

    async fn make_request(input: serde_json::Value) -> BrokerRequest {
        BrokerRequest {
            token: "tok".into(),
            parent_connection_id: "parent-conn".into(),
            parent_tool_use_id: "pt-1".into(),
            external_handle: None,
            input,
        }
    }

    #[tokio::test]
    async fn invalid_token_rejected() {
        let listener = make_listener(
            make_broker(Arc::new(MockSpawner::new())),
            Arc::new(TokenRegistry::default()),
            Some(1),
        );
        let outcome = listener
            .process(make_request(json!({"agent_type": "codex", "task": "x"})).await)
            .await;
        match outcome {
            DelegationOutcome::Err { code, message, .. } => {
                assert_eq!(code, "canceled");
                assert!(message.contains("invalid token"));
            }
            _ => panic!("expected canceled"),
        }
    }

    #[tokio::test]
    async fn token_parent_mismatch_rejected() {
        let tokens = Arc::new(TokenRegistry::default());
        tokens
            .register(
                "tok".into(),
                TokenEntry {
                    parent_connection_id: "other-parent".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        let listener = make_listener(make_broker(Arc::new(MockSpawner::new())), tokens, Some(1));
        let outcome = listener
            .process(make_request(json!({"agent_type": "codex", "task": "x"})).await)
            .await;
        match outcome {
            DelegationOutcome::Err { code, message, .. } => {
                assert_eq!(code, "canceled");
                assert!(message.contains("does not match"));
            }
            _ => panic!("expected canceled"),
        }
    }

    #[tokio::test]
    async fn missing_parent_conversation_rejected() {
        let tokens = Arc::new(TokenRegistry::default());
        tokens
            .register(
                "tok".into(),
                TokenEntry {
                    parent_connection_id: "parent-conn".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        // parent_conversation = None: parent has no live conversation.
        let listener = make_listener(make_broker(Arc::new(MockSpawner::new())), tokens, None);
        let outcome = listener
            .process(make_request(json!({"agent_type": "codex", "task": "x"})).await)
            .await;
        match outcome {
            DelegationOutcome::Err { code, message, .. } => {
                assert_eq!(code, "canceled");
                assert!(message.contains("no active conversation"));
            }
            _ => panic!("expected canceled"),
        }
    }

    #[tokio::test]
    async fn invalid_agent_type_rejected() {
        let tokens = Arc::new(TokenRegistry::default());
        tokens
            .register(
                "tok".into(),
                TokenEntry {
                    parent_connection_id: "parent-conn".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        let listener = make_listener(make_broker(Arc::new(MockSpawner::new())), tokens, Some(1));
        let outcome = listener
            .process(make_request(json!({"agent_type": "garbage", "task": "x"})).await)
            .await;
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "invalid_agent_type"),
            _ => panic!("expected invalid_agent_type"),
        }
    }

    #[tokio::test]
    async fn happy_path_via_duplex_stream() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("child-conn".into())).await;
        mock.queue_send(Ok(42)).await;
        let broker = make_broker(mock.clone());
        let tokens = Arc::new(TokenRegistry::default());
        tokens
            .register(
                "tok".into(),
                TokenEntry {
                    parent_connection_id: "parent-conn".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        let listener = make_listener(broker.clone(), tokens, Some(1));

        // Make broker resolve from another task once the call lands.
        let completer = {
            let broker = broker.clone();
            tokio::spawn(async move {
                loop {
                    if let Some(id) = broker.peek_first_pending_call_id().await {
                        broker
                            .complete_call(
                                &id,
                                DelegationOutcome::Ok(DelegationSuccess {
                                    text: "result-text".into(),
                                    child_conversation_id: 42,
                                    child_agent_type: AgentType::Codex,
                                    turn_count: 1,
                                    duration_ms: 5,
                                    token_usage: None,
                                }),
                            )
                            .await;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };

        // Drive the listener over a duplex pair.
        let (mut client, mut server) = duplex(16 * 1024);
        let server_task = tokio::spawn(async move {
            listener.serve_one(&mut server).await.unwrap();
        });

        let msg = BrokerMessage::Call(BrokerRequest {
            token: "tok".into(),
            parent_connection_id: "parent-conn".into(),
            parent_tool_use_id: "pt-1".into(),
            external_handle: None,
            input: json!({"agent_type": "codex", "task": "do x"}),
        });
        write_frame(&mut client, &msg).await.unwrap();
        let resp: BrokerResponse = read_frame(&mut client).await.unwrap();
        completer.await.unwrap();
        server_task.await.unwrap();

        assert_eq!(resp.outcome["kind"], "ok");
        assert_eq!(resp.outcome["text"], "result-text");
        assert_eq!(resp.outcome["child_conversation_id"], 42);
    }

    #[tokio::test]
    async fn cancel_message_routed_to_broker() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("c-cancel".into())).await;
        mock.queue_send(Ok(99)).await;
        let broker = make_broker(mock.clone());
        let tokens = Arc::new(TokenRegistry::default());
        tokens
            .register(
                "tok".into(),
                TokenEntry {
                    parent_connection_id: "parent-conn".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        let listener = make_listener(broker.clone(), tokens, Some(1));

        // Park a delegation call with a known external_handle.
        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move {
                let req = DelegationRequest {
                    parent_connection_id: "parent-conn".into(),
                    parent_conversation_id: 1,
                    parent_tool_use_id: "pt-cancel".into(),
                    agent_type: AgentType::Codex,
                    task: "do x".into(),
                    working_dir: None,
                    external_handle: Some("h-1".into()),
                };
                broker.handle_request(req).await
            })
        };
        while broker.pending_count().await == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Drive a cancel through the listener — listener should ack with
        // an empty BrokerResponse and the broker should drain the pending.
        let (mut client, mut server) = duplex(8 * 1024);
        let server_task = tokio::spawn(async move {
            listener.serve_one(&mut server).await.unwrap();
        });

        let cancel_msg = BrokerMessage::Cancel(BrokerCancelRequest {
            token: "tok".into(),
            external_handle: "h-1".into(),
            reason: Some("from test".into()),
        });
        write_frame(&mut client, &cancel_msg).await.unwrap();
        let resp: BrokerResponse = read_frame(&mut client).await.unwrap();
        assert!(resp.outcome.is_null(), "cancel ack must be null");
        server_task.await.unwrap();

        let outcome = driver.await.unwrap();
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "canceled"),
            other => panic!("expected canceled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn token_registry_revoke_and_revoke_by_parent() {
        let registry = TokenRegistry::default();
        registry
            .register(
                "t1".into(),
                TokenEntry {
                    parent_connection_id: "p1".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        registry
            .register(
                "t2".into(),
                TokenEntry {
                    parent_connection_id: "p1".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        registry
            .register(
                "t3".into(),
                TokenEntry {
                    parent_connection_id: "p2".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;

        registry.revoke("t1").await;
        assert!(registry.lookup("t1").await.is_none());
        assert!(registry.lookup("t2").await.is_some());

        registry.revoke_by_parent("p1").await;
        assert!(registry.lookup("t2").await.is_none());
        assert!(registry.lookup("t3").await.is_some());
    }

    // Sanity: spawn failure surfaces as spawn_failed when the listener path
    // is exercised. Exercises the full process() → broker.handle_request chain.
    #[tokio::test]
    async fn spawn_failure_surfaces_through_listener() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Err(SpawnerError::Spawn("agent missing".into())))
            .await;
        let broker = make_broker(mock);
        broker
            .set_config(DelegationConfig {
                enabled: true,
                depth_limit: 8,
                ..DelegationConfig::default()
            })
            .await;
        let tokens = Arc::new(TokenRegistry::default());
        tokens
            .register(
                "tok".into(),
                TokenEntry {
                    parent_connection_id: "parent-conn".into(),
                    working_dir: PathBuf::from("/tmp"),
                },
            )
            .await;
        let listener = make_listener(broker, tokens, Some(1));

        let outcome = listener
            .process(make_request(json!({"agent_type": "codex", "task": "x"})).await)
            .await;
        match outcome {
            DelegationOutcome::Err { code, .. } => assert_eq!(code, "spawn_failed"),
            _ => panic!("expected spawn_failed"),
        }
    }
}
