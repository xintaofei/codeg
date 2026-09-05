//! A27 real-adapter smoke: does a strict session/resume actually PRESERVE
//! context across a full process release?
//!
//! Round 1: launch the real `claude-agent-acp` adapter over stdio, open a
//! session, have it memorize a random marker (explicitly told NOT to write
//! any file), record the external session id, and drop the process.
//! Round 2: a BRAND-NEW process (new connection), strict session/resume of
//! the SAME id, then ask for the marker — the prompt does NOT contain it, so
//! a correct answer can only come from restored agent-side context.
//!
//! This proves limited context continuation (v2 design's top risk), not full
//! semantic memory. It requires a logged-in Claude Code CLI on this machine,
//! so it is `#[ignore]`d: run explicitly with
//!
//! ```sh
//! cargo test --features test-utils --test real_resume_smoke -- --ignored --nocapture
//! ```

#![recursion_limit = "1024"]

use sacp::schema::{
    ContentBlock, InitializeRequest, NewSessionRequest, ProtocolVersion, PromptRequest,
    SessionId, SessionNotification, SessionUpdate, TextContent,
};
use sacp::{on_receive_notification, Agent, Client, UntypedMessage};
use sacp_tokio::AcpAgent;
use std::sync::Arc;
use tokio::sync::Mutex;

fn random_marker() -> String {
    format!("SZK-{}", uuid::Uuid::new_v4().simple())
}

fn text_prompt(session: &SessionId, text: &str) -> PromptRequest {
    PromptRequest::new(
        session.clone(),
        vec![ContentBlock::Text(TextContent::new(text.to_string()))],
    )
}

/// One ACP episode against a FRESH adapter process: initialize → (new |
/// strict resume) → one prompt → collect the streamed assistant text.
/// Strict resume is the ONLY recovery: a session/resume failure ends the
/// episode with an error — never session/new.
async fn run_episode(
    cwd: &std::path::Path,
    turn_prompt: &str,
    resume_session: Option<String>,
) -> Result<(String, String), String> {
    let agent =
        AcpAgent::from_args(["claude-agent-acp"]).map_err(|e| format!("adapter launch: {e}"))?;

    let assistant = Arc::new(Mutex::new(String::new()));
    let assistant_sink = Arc::clone(&assistant);

    let result = Client
        .builder()
        .name("codeg-strict-smoke")
        .on_receive_notification(
            async move |notif: SessionNotification, _cx| {
                if let SessionUpdate::AgentMessageChunk(chunk) = &notif.update {
                    if let ContentBlock::Text(t) = &chunk.content {
                        assistant_sink.lock().await.push_str(&t.text);
                    }
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_with(agent, async move |cx| -> Result<String, sacp::Error> {
            let _ = cx
                .send_request_to(Agent, InitializeRequest::new(ProtocolVersion::LATEST))
                .block_task()
                .await?;

            let session_id = match resume_session.as_deref() {
                None => {
                    let resp = cx
                        .send_request_to(
                            Agent,
                            NewSessionRequest::new(cwd.to_path_buf()),
                        )
                        .block_task()
                        .await?;
                    resp.session_id
                }
                Some(sid) => {
                    let req = serde_json::json!({
                        "sessionId": sid,
                        "cwd": cwd.to_string_lossy(),
                    });
                    let untyped = UntypedMessage::new("session/resume", req)?;
                    cx.send_request_to(Agent, untyped).block_task().await?;
                    SessionId::new(sid.to_string())
                }
            };

            let _ = cx
                .send_request_to(Agent, text_prompt(&session_id, turn_prompt))
                .block_task()
                .await?;
            Ok(session_id.0.to_string())
        })
        .await
        .map_err(|e| format!("episode failed: {e}"))?;

    let text = assistant.lock().await.clone();
    Ok((result, text))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a logged-in Claude Code CLI + claude-agent-acp adapter"]
async fn strict_resume_preserves_a_context_only_marker() {
    let marker = random_marker();
    let dir = std::env::temp_dir().join("codeg-strict-smoke");
    std::fs::create_dir_all(&dir).expect("mkdir");

    // ---- Episode 1: fresh session, memorize the marker (no files!) --------
    let memorize_prompt = format!(
        "Remember this random marker for later: {marker}. \
         Do NOT write it to any file or note. Just reply with the single word OK."
    );
    let (session_id, reply1) = run_episode(&dir, &memorize_prompt, None)
        .await
        .expect("episode 1");
    println!("[smoke] external session id = {session_id}");
    println!("[smoke] episode 1 reply = {reply1}");
    assert!(
        reply1.to_uppercase().contains("OK"),
        "episode 1 should acknowledge: {reply1}"
    );

    // ---- Episode 2: BRAND-NEW process, strict resume, ask for the marker --
    let ask_prompt =
        "What random marker did I ask you to remember earlier? Reply with ONLY the marker."
            .to_string();
    match run_episode(&dir, &ask_prompt, Some(session_id.clone())).await {
        Ok((resumed_id, reply2)) => {
            assert_eq!(resumed_id, session_id, "resume must keep the SAME external id");
            println!("[smoke] episode 2 reply = {reply2}");
            assert!(
                reply2.contains(&marker),
                "the resumed session must recall the marker {marker}; got: {reply2}"
            );
            println!("[smoke] PASS: strict resume preserved the context-only marker");
        }
        Err(e) => {
            // An honest failure is a VALID smoke outcome for an agent whose
            // resume is broken — report precisely, never fake success.
            panic!("strict resume smoke FAILED (a real negative result, not a skip): {e}");
        }
    }
}
