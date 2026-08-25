use crate::acp::delegation::broker::{
    DelegationBroker, ConversationDepthLookup, DelegationConfig,
};
use crate::acp::delegation::spawner::ConnectionSpawner;
use crate::acp::delegation::types::{DelegationOutcome, DelegationRequest};
use crate::models::agent::AgentType;
use crate::semantic::envelope::{AcceptState, IntentEnvelope, Op};
use serde::Deserialize;
use std::sync::Arc;

/// A semantic-mode request: an intent, the reason it matters, the set of
/// concrete operations the operator sub-agent should run, and where to run
/// them.
#[derive(Debug, Clone, Deserialize)]
pub struct SemanticRequest {
    pub intent: String,
    pub why: String,
    pub ops: Vec<Op>,
    pub working_dir: Option<String>,
    pub agent_type: AgentType,
}

fn render_ops(ops: &[Op]) -> String {
    ops.iter()
        .map(|o| {
            format!(
                "[{}] {}",
                o.tool,
                serde_json::to_string(&o.params).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drive an operator sub-agent (via the existing `codeg-mcp` delegation
/// broker) to execute the requested ops and produce raw output, then run a
/// summarizer sub-agent to sanitize that raw output into the user-facing
/// `result`. Returns a fully-populated `IntentEnvelope`.
///
/// If the operator delegation fails, the envelope is returned in the `Denied`
/// state carrying the broker's error message. If only the summarizer fails,
/// the envelope is still `Accepted` and falls back to a truncated copy of the
/// raw output so the main chat is never left empty.
pub async fn run_semantic_core(
    spawner: Arc<dyn ConnectionSpawner>,
    depth: Arc<dyn ConversationDepthLookup>,
    req: SemanticRequest,
) -> IntentEnvelope {
    let broker = DelegationBroker::new(spawner, depth);
    semantic_core_inner(broker, req).await
}

/// Core glue shared by the public entry point and the tests: builds the two
/// delegation hops (operator → summarizer) on the supplied broker. Taking the
/// broker by value (and being `pub(crate)`) lets the tests drive the same
/// broker instance to completion under `MockSpawner` — `DelegationBroker` is
/// `Clone` and shares its pending map via `Arc`, so a clone parked inside the
/// driver task resolves against the broker the test owns.
pub(crate) async fn semantic_core_inner(
    broker: DelegationBroker,
    req: SemanticRequest,
) -> IntentEnvelope {
    broker
        .set_config(DelegationConfig {
            enabled: true,
            ..Default::default()
        })
        .await;

    let operator_task = format!(
        "INTENT: {}\nWHY: {}\nRun these operations and return their raw output:\n{}",
        req.intent,
        req.why,
        render_ops(&req.ops)
    );
    let op_req = DelegationRequest {
        parent_connection_id: "semantic".into(),
        parent_conversation_id: 0,
        parent_tool_use_id: "semantic-op".into(),
        agent_type: req.agent_type,
        task: operator_task,
        working_dir: req.working_dir.clone(),
        requested_working_dir: req.working_dir.clone(),
        external_handle: None,
    };
    let raw = match broker.handle_request(op_req).await {
        DelegationOutcome::Ok(s) => s.text,
        DelegationOutcome::Err { message, .. } => return IntentEnvelope::denied(&message),
    };

    let summarize_task = format!(
        "You are a result sanitizer. Given an intent, its why, and the raw tool output, \
         write a concise (<=6 lines) answer that directly addresses the intent. \
         Do NOT include raw logs.\n\nINTENT: {}\nWHY: {}\n\nRAW OUTPUT:\n{}",
        req.intent, req.why, raw
    );
    let sum_req = DelegationRequest {
        parent_connection_id: "semantic".into(),
        parent_conversation_id: 0,
        parent_tool_use_id: "semantic-sum".into(),
        agent_type: req.agent_type,
        task: summarize_task,
        working_dir: req.working_dir,
        requested_working_dir: None,
        external_handle: None,
    };
    let result = match broker.handle_request(sum_req).await {
        DelegationOutcome::Ok(s) => s.text,
        DelegationOutcome::Err { message, .. } => {
            let trunc: String = raw.chars().take(500).collect();
            format!("[sanitizer failed: {message}] {trunc}")
        }
    };

    IntentEnvelope {
        intent: req.intent,
        why: req.why,
        ops: req.ops,
        accept: AcceptState::Accepted,
        result: Some(result),
        raw: Some(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::delegation::spawner::mock::MockSpawner;
    use crate::acp::delegation::spawner::SpawnerError;
    use crate::acp::delegation::types::{DelegationError, DelegationSuccess};
    use std::time::Duration;

    struct RootDepth;
    #[async_trait::async_trait]
    impl ConversationDepthLookup for RootDepth {
        async fn parent_of(&self, _id: i32) -> Result<Option<i32>, DelegationError> {
            Ok(None)
        }
    }

    fn ok_outcome(text: &str, conv: i32) -> DelegationOutcome {
        DelegationOutcome::Ok(DelegationSuccess {
            text: text.into(),
            child_conversation_id: conv,
            child_agent_type: AgentType::ClaudeCode,
            turn_count: 1,
            duration_ms: 1,
            token_usage: None,
        })
    }

    /// Drives BOTH delegation hops (operator + summarizer) on a single broker
    /// the test owns — mirroring `happy_path_returns_ok_after_complete_call`
    /// but for two sequential `handle_request` calls. `DelegationBroker` clones
    /// share the pending map via `Arc`, so resolving calls on the test-owned
    /// broker unblocks the driver task running `semantic_core_inner`.
    #[tokio::test]
    async fn operator_raw_is_sanitized_to_result() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Ok("op-conn".into())).await;
        mock.queue_send(Ok(1)).await;
        mock.queue_spawn(Ok("sum-conn".into())).await;
        mock.queue_send(Ok(2)).await;

        let broker = DelegationBroker::new(
            mock as Arc<dyn ConnectionSpawner>,
            Arc::new(RootDepth) as Arc<dyn ConversationDepthLookup>,
        );

        let req = SemanticRequest {
            intent: "list files".into(),
            why: "see layout".into(),
            ops: vec![Op {
                tool: "shell".into(),
                params: serde_json::json!({"cmd":"ls"}),
            }],
            working_dir: Some("/tmp".into()),
            agent_type: AgentType::ClaudeCode,
        };

        let driver = {
            let broker = broker.clone();
            tokio::spawn(async move { semantic_core_inner(broker, req).await })
        };

        // Drive the operator delegation to completion.
        let op_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker.complete_call(&op_id, ok_outcome("RAW_OPERATOR", 1)).await;

        // Drive the summarizer delegation to completion (distinct call id).
        let sum_id = loop {
            if let Some(id) = broker.peek_first_pending_call_id().await {
                if id != op_id {
                    break id;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        broker
            .complete_call(&sum_id, ok_outcome("RESULT_SUMMARY", 2))
            .await;

        let out = driver.await.unwrap();
        assert_eq!(out.accept, AcceptState::Accepted);
        assert_eq!(out.raw.as_deref(), Some("RAW_OPERATOR"));
        assert_eq!(out.result.as_deref(), Some("RESULT_SUMMARY"));
        assert_eq!(out.ops.len(), 1);
    }

    #[tokio::test]
    async fn operator_failure_returns_denied() {
        let mock = Arc::new(MockSpawner::new());
        mock.queue_spawn(Err(SpawnerError::Spawn("boom".into()))).await;

        let broker = DelegationBroker::new(
            mock as Arc<dyn ConnectionSpawner>,
            Arc::new(RootDepth) as Arc<dyn ConversationDepthLookup>,
        );
        let req = SemanticRequest {
            intent: "list files".into(),
            why: "see layout".into(),
            ops: vec![],
            working_dir: None,
            agent_type: AgentType::ClaudeCode,
        };

        let out = semantic_core_inner(broker, req).await;
        assert_eq!(out.accept, AcceptState::Denied);
        assert!(out.result.as_ref().unwrap().contains("boom"));
        assert!(out.raw.is_none());
    }
}
