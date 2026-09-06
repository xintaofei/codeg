//! Outbound calls to the user's translation endpoints.
//!
//! Deliberately narrow: one POST per text, rotated across the provider pool
//! ([`crate::translation::pool`]) and paced by each provider's adaptive
//! limiter ([`crate::translation::aimd`]). The endpoints belong to the user
//! and may be small self-hosted models, so the pool exists to spread load and
//! the limiter to keep codeg from being the reason any one of them falls
//! over.
//!
//! Four dialects (`ApiFormat`) ride this one path: OpenAI-compatible requests
//! for openai/gemini/ollama (their compat surfaces differ only in URL and
//! auth), and Anthropic's native `/v1/messages` for anthropic, whose host
//! publishes no OpenAI route.

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::time::sleep;

use crate::app_error::AppCommandError;
use crate::translation::metrics::{translation_metrics, ProviderEventKind};
use crate::translation::pool::{self, pick_provider, PickedProvider};
use crate::translation::prompt;
use crate::translation::settings::{ApiFormat, ProviderConfig, TranslationSettings};

/// Two lanes, each a plain concurrency cap (not a rate): the adaptive limiter
/// owns the rate, these only mask round-trip latency. The lanes exist because
/// the per-provider queue is FIFO: a settled history view can enqueue dozens
/// of thinking-block chunks at once, and under a single gate a newly settled
/// reply's body text waited behind all of them — the "the button never
/// appears" report. Visible prose and user-initiated translation ride the
/// priority lane; background thinking-block translation shares whatever
/// endpoint capacity is left.
const PRIORITY_MAX_CONCURRENT: usize = 4;
const BACKGROUND_MAX_CONCURRENT: usize = 3;
/// Transport-level and 5xx retries. A 429 is NOT retried in place any more:
/// the pool's rotation hands the next chunk to another provider, the limiter
/// throttles this one, and the frontend's bounded retry re-requests only what
/// is still missing.
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Generous on purpose: reasoning models (deepseek-r1 distills and friends)
/// spend tens of seconds *thinking* about even a short translation, so a flat
/// 60 s reads as "endpoint broken" when the endpoint is merely slow.
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Well under the ~30-60 s idle cutoff CDNs apply to keep-alive connections:
/// a pooled connection older than this is evicted instead of failing the next
/// request the instant it is reused.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// Per-request deadline scaling (plan B2). A slow endpoint needs real time to
/// translate a full 3000-char chunk; a flat 60 s would kill the tail of a long
/// batch the frontend has already budgeted 300 s for.
const SCALING_TIMEOUT_THRESHOLD_CHARS: usize = 2000;
const SCALING_TIMEOUT_BASE: Duration = Duration::from_secs(60);
const SCALING_TIMEOUT_PER_CHAR: Duration = Duration::from_millis(20);

/// The model-list probe answers before any generation happens, so it gets a
/// tight deadline: hanging here is a settings-page click, not a reading flow.
const MODELS_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on distinct model names one list response may contribute.
const MAX_MODEL_LIST: usize = 500;

/// Cap on what a translation endpoint may return, so a misbehaving or hostile
/// server cannot stream an unbounded body into memory. Generous next to the
/// 3000-char request cap the frontend enforces.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// The deadline one chat request may take, scaled to how much text it carries.
/// Chunks at or under the threshold keep the flat [`READ_TIMEOUT`]; larger ones
/// earn `30 s + 20 ms/char` (3000 chars → 90 s).
fn request_timeout(text_chars: usize) -> Duration {
    if text_chars <= SCALING_TIMEOUT_THRESHOLD_CHARS {
        return READ_TIMEOUT;
    }
    SCALING_TIMEOUT_BASE
        .saturating_add(SCALING_TIMEOUT_PER_CHAR.saturating_mul(text_chars as u32))
}

/// The version header Anthropic pins per protocol release; requests without it
/// are rejected outright.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Whether the request serves content the reader is waiting on (reply prose,
/// a hand-initiated translation) or background polish (thinking blocks). The
/// lane decides which concurrency gate the request queues on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Priority,
    Background,
}

impl Priority {
    /// The cap for this lane: a plain concurrency ceiling the adaptive rate
    /// lives under, not a rate itself.
    fn max_concurrent(self) -> usize {
        match self {
            Priority::Priority => PRIORITY_MAX_CONCURRENT,
            Priority::Background => BACKGROUND_MAX_CONCURRENT,
        }
    }
}

/// Dynamically sized gates. Grown once at first use, never shrunk: a briefly
/// oversubscribed lane is harmless, and the lane caps are compile-time
/// constants since the adaptive limiter took over pacing.
struct LaneGate {
    semaphore: Semaphore,
}

fn gate(priority: Priority) -> &'static Semaphore {
    static PRIORITY: OnceLock<LaneGate> = OnceLock::new();
    static BACKGROUND: OnceLock<LaneGate> = OnceLock::new();
    let lane = match priority {
        Priority::Priority => &PRIORITY,
        Priority::Background => &BACKGROUND,
    };
    let cap = priority.max_concurrent();
    &lane
        .get_or_init(|| LaneGate {
            semaphore: Semaphore::new(cap),
        })
        .semaphore
}

/// The proxy env fingerprint a client was built under, paired with that client.
/// Same contract as `forge::http_client`: reqwest freezes proxy configuration
/// at build time, but codeg lets the user change it at runtime.
type ProxyKeyedClient = (Vec<(String, String)>, reqwest::Client);

static HTTP_CLIENT: RwLock<Option<ProxyKeyedClient>> = RwLock::new(None);

fn http_client() -> Result<reqwest::Client, AppCommandError> {
    let fingerprint = crate::network::proxy::current_proxy_env_vars();
    if let Ok(guard) = HTTP_CLIENT.read() {
        if let Some((cached, client)) = guard.as_ref() {
            if *cached == fingerprint {
                return Ok(client.clone());
            }
        }
    }

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(READ_TIMEOUT)
        // Relays behind CDNs (Cloudflare and friends) close idle connections
        // within seconds without telling us, while our pool would happily keep
        // them for 90 s. Reusing one of those corpses fails the request
        // instantly — hyper does not retry a POST — so evict idle connections
        // well before the CDN does and let TCP keepalive notice real breaks.
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE)
        .build()
        .map_err(|e| {
            AppCommandError::network("Failed to build the translation HTTP client")
                .with_detail(e.to_string())
        })?;

    if let Ok(mut guard) = HTTP_CLIENT.write() {
        *guard = Some((fingerprint, client.clone()));
    }
    Ok(client)
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    /// Deterministic output makes the cache worth having: the same text should
    /// not produce a different translation on a later miss.
    temperature: f32,
    /// Without it a relay picks its own (often tiny) completion ceiling and
    /// silently truncates the translation mid-paragraph; the half translation
    /// then lands in the cache and is served forever. Scaled the same way as
    /// the Anthropic path.
    max_tokens: usize,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// Anthropic's native `/v1/messages` body shape. Deliberately minimal: a
/// system string, one user turn, no tools. The request is built as JSON
/// directly (field order and optionality differ enough from the OpenAI
/// envelope that sharing structs would obscure both); this type exists for
/// the response-side docs and future request-side reuse.
#[derive(Serialize)]
#[allow(dead_code)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: usize,
    system: &'a str,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: Option<ChatResponseMessage>,
    /// `"length"` means the completion hit `max_tokens` mid-output — the same
    /// truncation-as-poison the Anthropic path refuses at `stop_reason`.
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(default)]
    #[allow(dead_code)]
    r#type: String,
    #[serde(default)]
    text: Option<String>,
}

/// Anthropic caps completion length by model; the clamp keeps a big chunk from
/// requesting past it and from under-requesting on a tiny one.
fn anthropic_max_tokens(text_chars: usize) -> usize {
    (text_chars.saturating_mul(2).saturating_add(1024)).clamp(4096, 32768)
}

/// The OpenAI-compatible ceiling. Relays reject a `max_tokens` past the
/// model's output limit, so the cap stays conservative (8k covers every
/// current model); the floor keeps a one-line reply from being asked for with
/// a ceiling a reasoning endpoint burns entirely on its own thinking.
fn openai_max_tokens(text_chars: usize) -> usize {
    (text_chars.saturating_mul(2).saturating_add(1024)).clamp(1024, 8192)
}

/// Whether a failure is worth retrying. A 4xx means the request itself is
/// wrong — retrying it just spends the user's quota to fail identically.
fn is_retryable(status: Option<reqwest::StatusCode>) -> bool {
    match status {
        // Rate limiting is the one 4xx that a wait can fix.
        Some(status) => status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS,
        // Transport-level failure (timeout, connection reset).
        None => true,
    }
}

/// One warn line per failed request, with the classified message and whatever
/// detail the endpoint's body carried. The renderer discards failed
/// translations silently (the message simply stays in its original language),
/// so this log is the only place the *why* is visible. The provider id rides
/// along because the settings page's health view attributes every failure.
fn log_failure(stage: &str, provider: &str, error: &AppCommandError) {
    tracing::warn!(
        "[translation] {} (provider {}) failed: {}{}",
        stage,
        provider,
        error.message,
        error
            .detail
            .as_deref()
            .map(|detail| format!(" — {detail}"))
            .unwrap_or_default()
    );
}

fn classify(status: reqwest::StatusCode, body: &str) -> AppCommandError {
    let detail = body.chars().take(500).collect::<String>();
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            AppCommandError::authentication_failed(
                "The translation service rejected the API key",
            )
            .with_detail(detail)
        }
        reqwest::StatusCode::NOT_FOUND => AppCommandError::configuration_invalid(
            "The translation endpoint was not found — check the base URL",
        )
        .with_detail(detail),
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            AppCommandError::network("The translation service is rate limiting requests")
                .with_detail(detail)
        }
        _ => AppCommandError::network(format!(
            "The translation service returned HTTP {}",
            status.as_u16()
        ))
        .with_detail(detail),
    }
}

/// Read the response body chunk by chunk, refusing past `cap`. `bytes()` would
/// buffer the whole body before any check — the opposite of what the cap
/// promises — so a hostile or misbehaving endpoint must trip the limit while
/// it is still streaming, not after its payload is already in memory.
async fn read_capped(
    mut response: reqwest::Response,
    cap: usize,
    context: &str,
) -> Result<Vec<u8>, AppCommandError> {
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        AppCommandError::network(format!("Failed to read the {context} response"))
            .with_detail(e.to_string())
    })? {
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(AppCommandError::network(format!(
                "The {context} response was too large"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The auth headers a request to `settings`' endpoint carries. Anthropic signs
/// with `x-api-key` plus a pinned protocol version; the OpenAI-compatible
/// dialects use a bearer token, and an empty key (local Ollama) sends none.
fn auth_headers(format: ApiFormat, api_key: &str) -> Vec<(&'static str, String)> {
    match format {
        ApiFormat::Anthropic => vec![
            ("x-api-key", api_key.to_string()),
            ("anthropic-version", ANTHROPIC_VERSION.to_string()),
        ],
        _ => {
            if api_key.trim().is_empty() {
                Vec::new()
            } else {
                vec![("Authorization", format!("Bearer {}", api_key))]
            }
        }
    }
}

/// One text in, one translation out, against one picked provider. Retries
/// transport and 5xx failures per [`RETRY_BACKOFF`]; returns the last error
/// when the retries are spent. Every outcome feeds the provider's adaptive
/// limiter (success climbs, a 429 halves and cools down, a 4xx twice in a
/// row retires the endpoint for the session) and the per-attempt metrics
/// window that the health score reads.
///
/// The request speaks the endpoint's dialect ([`ApiFormat`]): Anthropic gets
/// its native `/v1/messages` body and versioned key headers, everything else
/// the OpenAI chat shape with a bearer token.
async fn translate_one(
    text: &str,
    target_lang: &str,
    picked: &PickedProvider,
    priority: Priority,
) -> ChunkOutcome {
    let client = match http_client() {
        Ok(client) => client,
        Err(err) => {
            return ChunkOutcome {
                result: Err(err),
                provider_id: picked.id().to_string(),
                latency_ms: 0,
            }
        }
    };
    let provider = &picked.provider;
    let url = provider.chat_completions_url();
    let system = prompt::system_prompt(target_lang);
    let format = provider.resolve_format();
    let timeout = request_timeout(text.chars().count());

    let body = match format {
        ApiFormat::Anthropic => serde_json::json!({
            "model": provider.model,
            "max_tokens": anthropic_max_tokens(text.chars().count()),
            "system": system,
            "messages": [{ "role": "user", "content": text }],
        }),
        _ => serde_json::to_value(ChatRequest {
            model: &provider.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: &system,
                },
                ChatMessage {
                    role: "user",
                    content: text,
                },
            ],
            temperature: 0.0,
            max_tokens: openai_max_tokens(text.chars().count()),
        })
        .expect("the chat request serializes by construction"),
    };

    let mut attempt = 0;
    #[allow(unused_assignments)]
    let mut last_latency_ms = 0u64;
    loop {
        // The provider's adaptive pacing slot first, then the lane gate: the
        // wait for the next dispatch slot must not occupy a concurrency
        // permit. The permit is held for the whole attempt so the gate bounds
        // requests actually in flight, not just the rate they start.
        picked.wait_for_dispatch_slot().await;
        let _permit = match gate(priority).acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                return ChunkOutcome {
                    result: Err(AppCommandError::task_execution_failed(
                        "Translation gate closed",
                    )),
                    provider_id: picked.id().to_string(),
                    latency_ms: 0,
                }
            }
        };

        tracing::debug!(
            "[translation] sending {} chars to {} (lane {:?}, attempt {}): {}",
            text.chars().count(),
            picked.id(),
            priority,
            attempt + 1,
            text.chars().take(200).collect::<String>()
        );
        let started = std::time::Instant::now();
        let mut request = client.post(&url).timeout(timeout);
        for (name, value) in auth_headers(format, &provider.api_key) {
            request = request.header(name, value);
        }
        let outcome = request.json(&body).send().await;
        last_latency_ms = started.elapsed().as_millis() as u64;
        let latency = last_latency_ms;

        // Per-attempt recording: a retried chunk shows every attempt, which
        // is what pacing analysis and the health window need.
        let metrics = translation_metrics();
        metrics.record_dispatch(picked.id());

        let error = match outcome {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let bytes =
                        match read_capped(response, MAX_RESPONSE_BYTES, "translation").await {
                            Ok(bytes) => bytes,
                            Err(err) => {
                                log_failure("read the translation response", picked.id(), &err);
                                return ChunkOutcome {
                                    result: Err(err),
                                    provider_id: picked.id().to_string(),
                                    latency_ms: latency,
                                };
                            }
                        };
                    let parsed = match format {
                        ApiFormat::Anthropic => parse_anthropic_translation(&bytes),
                        _ => parse_translation(&bytes),
                    };
                    match &parsed {
                        Err(err) => log_failure("parse the translation response", picked.id(), err),
                        // DEBUG diagnostics for the "endpoint answers fine but
                        // nothing renders" class of report: the frontend
                        // discards a translation whose placeholders drifted,
                        // and this snippet is where the drift is visible.
                        Ok(translated) => tracing::debug!(
                            "[translation] response from {} in {latency}ms: {}",
                            picked.id(),
                            translated.chars().take(400).collect::<String>()
                        ),
                    }
                    // A parseable reply is transport success (feeds the
                    // health window and the AIMD climb); a parse failure is
                    // not — the endpoint answered, but not with a
                    // translation, and rewarding it would inflate the rate.
                    match &parsed {
                        Ok(_) => {
                            metrics.record_attempt(picked.id(), ProviderEventKind::Ok, latency);
                            picked.report_success();
                        }
                        Err(err) => {
                            if err.message.contains("cut off") {
                                metrics.record_truncated();
                            }
                            metrics.record_attempt(
                                picked.id(),
                                ProviderEventKind::ParseError,
                                latency,
                            );
                        }
                    }
                    return ChunkOutcome {
                        result: parsed,
                        provider_id: picked.id().to_string(),
                        latency_ms: latency,
                    };
                }
                let retry_after = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    // The upstream is saturated. Halve this provider's rate
                    // and park it for the window it names; every queued
                    // request to it picks up the new pacing, and the rotation
                    // sends the next chunks elsewhere meanwhile.
                    response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .and_then(parse_retry_after)
                } else {
                    None
                };
                let body = response.text().await.unwrap_or_default();
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    picked.report_rate_limited(retry_after);
                    let err = classify(status, &body);
                    log_failure("endpoint rate limited the request", picked.id(), &err);
                    metrics.record_attempt(picked.id(), ProviderEventKind::RateLimited, latency);
                    return ChunkOutcome {
                        result: Err(err),
                        provider_id: picked.id().to_string(),
                        latency_ms: latency,
                    };
                }
                let err = classify(status, &body);
                if status.is_client_error() {
                    picked.report_client_error(&err.message);
                }
                log_failure(&format!("endpoint answered HTTP {status}"), picked.id(), &err);
                metrics.record_attempt(picked.id(), ProviderEventKind::HttpError, latency);
                if !is_retryable(Some(status)) {
                    return ChunkOutcome {
                        result: Err(err),
                        provider_id: picked.id().to_string(),
                        latency_ms: latency,
                    };
                }
                err
            }
            Err(err) => {
                let mapped = AppCommandError::network("The translation request failed")
                    .with_detail(err.to_string());
                log_failure("request to the translation endpoint", picked.id(), &mapped);
                metrics.record_attempt(picked.id(), ProviderEventKind::NetworkError, latency);
                if !is_retryable(err.status()) {
                    return ChunkOutcome {
                        result: Err(mapped),
                        provider_id: picked.id().to_string(),
                        latency_ms: latency,
                    };
                }
                mapped
            }
        };

        // `_permit` drops here, so the backoff wait does not occupy the gate.
        drop(_permit);
        match RETRY_BACKOFF.get(attempt) {
            Some(delay) => {
                tracing::debug!(
                    "[translation] attempt {} failed, retrying in {:?}",
                    attempt + 1,
                    delay
                );
                sleep(*delay).await;
                attempt += 1;
            }
            None => {
                return ChunkOutcome {
                    result: Err(error),
                    provider_id: picked.id().to_string(),
                    latency_ms: last_latency_ms,
                }
            }
        }
    }
}

/// The final state of one chunk's dispatch: what came back, from whom, and
/// how long the deciding attempt took. The provider id is what lets the
/// caller attribute quality-gate rejections to the endpoint that produced
/// the refused reply.
pub struct ChunkOutcome {
    pub result: Result<String, AppCommandError>,
    pub provider_id: String,
    pub latency_ms: u64,
}

/// `Retry-After` in either documented shape: seconds ("12") or an HTTP date.
/// Dates are rare in the wild and parsing them is not worth the surface — a
/// missing fallback just means the AIMD halving throttles without parking.
fn parse_retry_after(raw: &str) -> Option<Duration> {
    raw.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Reply openings a relay's guard model produces when it refuses the request
/// or answers as itself instead of translating. Deliberately narrow — each
/// shape was served by a real relay — because a false positive discards a
/// genuine translation: a real translation into any target never opens with
/// a first-person AI self-identification or a capability statement.
fn refusal_shape(content: &str) -> bool {
    const REFUSAL_OPENINGS: [&str; 12] = [
        "i'm mistral",
        "i am mistral",
        "i'm a large language model",
        "i am a large language model",
        "i can only translate",
        "i can't provide",
        "i cannot provide",
        "i can't fulfill",
        "i cannot fulfill",
        "i'm unable to",
        "i am unable to",
        "i don't have the capability",
    ];
    let lowered = content.trim_start().to_lowercase();
    REFUSAL_OPENINGS
        .iter()
        .any(|opening| lowered.starts_with(opening))
}

fn parse_translation(bytes: &[u8]) -> Result<String, AppCommandError> {
    let parsed: ChatResponse = serde_json::from_slice(bytes).map_err(|e| {
        AppCommandError::network("The translation service returned malformed JSON")
            .with_detail(e.to_string())
    })?;

    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| AppCommandError::network("The translation service returned no translation"))?;

    // A `length` stop means the answer was cut mid-output; serving it would
    // cache a half translation forever (the same rule the Anthropic path
    // applies to `stop_reason: "max_tokens"`). Some reasoning relays also burn
    // the whole budget on `<think>` and stop at `length` with no answer behind
    // it — refusing is the only safe reading of that response.
    if choice.finish_reason.as_deref() == Some("length") {
        return Err(AppCommandError::network(
            "The translation was cut off before completion",
        ));
    }

    let content = choice
        .message
        .and_then(|message| message.content)
        .ok_or_else(|| {
            AppCommandError::network("The translation service returned no translation")
        })?;

    // Reasoning distills (DeepSeek-R1 & friends) inline their chain of thought
    // as `<think>…</think>` before the answer. That text is not the
    // translation: serving it would pour reasoning into the message, and its
    // rambling usually drags the placeholders out of shape. A truncated think
    // (no closing tag) means there is no answer behind it at all.
    let translated = strip_reasoning_block(&content);
    // A refusal caught here fails immediately with a precise message instead
    // of riding the quality gates one layer up — and, crucially, it is never
    // counted as transport success, so the AIMD rate and the health score
    // both see the endpoint's real behavior.
    if refusal_shape(&translated) {
        return Err(AppCommandError::network(
            "The endpoint refused the request instead of translating",
        )
        .with_detail(translated.chars().take(200).collect::<String>()));
    }
    if translated.is_empty() {
        return Err(AppCommandError::network(
            "The translation service returned only reasoning, no translation",
        ));
    }
    Ok(translated)
}

/// Cut one `<think>…</think>` block (or an unclosed one running to the end),
/// keeping whatever prose precedes it. Byte offsets are safe: the markers are
/// ASCII in a `String` that is always valid UTF-8.
fn strip_reasoning_block(content: &str) -> String {
    let Some(start) = content.find("<think>") else {
        return content.trim().to_string();
    };
    let stripped = match content[start..].find("</think>") {
        Some(end) => {
            let close_end = start + end + "</think>".len();
            let mut out = String::with_capacity(content.len());
            out.push_str(&content[..start]);
            out.push_str(&content[close_end..]);
            out
        }
        None => content[..start].to_string(),
    };
    stripped.trim().to_string()
}

fn parse_anthropic_translation(bytes: &[u8]) -> Result<String, AppCommandError> {
    let parsed: AnthropicResponse = serde_json::from_slice(bytes).map_err(|e| {
        AppCommandError::network("The translation service returned malformed JSON")
            .with_detail(e.to_string())
    })?;

    // `max_tokens` truncation would cache a half translation and serve it
    // forever; refuse it and let the caller fall back to the original.
    if parsed.stop_reason.as_deref() == Some("max_tokens") {
        return Err(AppCommandError::network(
            "The translation was cut off before completion",
        ));
    }

    let text = parsed
        .content
        .iter()
        .filter_map(|block| block.text.as_deref())
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        return Err(AppCommandError::network(
            "The translation service returned no translation",
        ));
    }
    Ok(text)
}

/// `GET {base}/models` for the settings page's picker, aimed at ONE provider
/// (the row being edited, by id — same rule as [`test_connection`]). Searched
/// across every configured row, not only the complete ones: the model field
/// is what this call fills in. Runs against the form's (possibly unsaved)
/// settings; the caller resolves the masked key first.
pub async fn list_models(
    settings: &TranslationSettings,
    provider_id: Option<&str>,
) -> Result<Vec<String>, AppCommandError> {
    let candidate = |provider: &ProviderConfig| {
        !provider.base_url.trim().is_empty()
            && (provider.resolve_format() == ApiFormat::Ollama || !provider.api_key.is_empty())
    };
    let provider = provider_id
        .and_then(|id| settings.providers.iter().find(|p| p.id == id && candidate(p)))
        .or_else(|| settings.providers.iter().find(|p| candidate(p)))
        .cloned()
        // A legacy row keeps its endpoint in the flat fields.
        .or_else(|| {
            let legacy = ProviderConfig {
                base_url: settings.base_url.clone(),
                api_key: settings.api_key.clone(),
                model: settings.model.clone(),
                api_format: settings.api_format.clone(),
                ..Default::default()
            };
            candidate(&legacy).then_some(legacy)
        })
        .ok_or_else(|| {
            AppCommandError::configuration_missing(
                "Fill in the provider's base URL and key before fetching models",
            )
        })?;

    let client = http_client()?;
    let url = provider.models_url();
    let format = provider.resolve_format();

    let mut request = client.get(&url).timeout(MODELS_TIMEOUT);
    for (name, value) in auth_headers(format, &provider.api_key) {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(|err| {
        AppCommandError::network("The model list request failed").with_detail(err.to_string())
    })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(if status == reqwest::StatusCode::NOT_FOUND {
            AppCommandError::configuration_invalid(
                "This endpoint does not expose a model list — enter the model name manually",
            )
            .with_detail(body.chars().take(500).collect::<String>())
        } else {
            classify(status, &body)
        });
    }

    let bytes = read_capped(response, MAX_RESPONSE_BYTES, "model list").await?;
    parse_models(&bytes)
}

/// OpenAI (`{data:[{id}]}`), Ollama, and Gemini's compat surface all answer
/// this shape; Anthropic's native list is `{data:[{id,…}]}` too. The fallbacks
/// cover the minor drift between them.
fn parse_models(bytes: &[u8]) -> Result<Vec<String>, AppCommandError> {
    let parsed: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| {
        AppCommandError::network("The model list returned malformed JSON")
            .with_detail(e.to_string())
    })?;

    let entries = match parsed {
        serde_json::Value::Array(items) => Some(items),
        serde_json::Value::Object(map) => map
            .get("data")
            .or_else(|| map.get("models"))
            .and_then(|value| value.as_array())
            .cloned(),
        _ => None,
    }
    .ok_or_else(|| {
        AppCommandError::network("The model list response was not recognised")
    })?;

    let mut names = Vec::new();
    for entry in entries {
        let name = match &entry {
            serde_json::Value::String(raw) => Some(raw.clone()),
            serde_json::Value::Object(map) => map
                .get("id")
                .or_else(|| map.get("name"))
                .and_then(|value| value.as_str())
                .map(str::to_string),
            _ => None,
        };
        if let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    names.truncate(MAX_MODEL_LIST);
    Ok(names)
}

/// Translate every text, preserving order. One request per text; each text
/// picks its own provider from the rotation, so a batch of N chunks spreads
/// across the pool instead of stacking on one endpoint.
///
/// The texts are issued concurrently — `join_all` (not `try_join_all`) so one
/// failed chunk does not cancel the others' already-spent work. Per-chunk
/// results, not all-or-nothing: a rate-limited endpoint (whose failed
/// attempts count against the limit, so a burst fails *some* chunks) would
/// otherwise throw away every chunk that succeeded. The caller caches the
/// successes and only re-requests the failures, and the rotation gives those
/// retries a different provider to land on.
pub async fn translate_batch(
    texts: &[String],
    target_lang: &str,
    settings: &TranslationSettings,
    priority: Priority,
) -> Vec<ChunkOutcome> {
    join_all(texts.iter().map(|text| async move {
        // Re-pick per chunk: the rotation spreads the batch, and a provider
        // that just drew a 429 is already cooling down for the next pick.
        let picked = match pick_provider(settings).await {
            Ok(picked) => picked,
            Err(err) => {
                // No provider was picked, so there is nothing to attribute.
                return ChunkOutcome {
                    result: Err(err),
                    provider_id: String::new(),
                    latency_ms: 0,
                };
            }
        };
        translate_one(text, target_lang, &picked, priority).await
    }))
    .await
}

/// The settings page's connection test, aimed at ONE provider (the row being
/// edited, identified by its stable id) so a multi-member pool tests the
/// endpoint the user is looking at. Returns what the endpoint made of
/// [`prompt::TEST_PHRASE`], or a classified error the page can show verbatim.
pub async fn test_connection(
    settings: &TranslationSettings,
    target_lang: &str,
    provider_id: Option<&str>,
) -> Result<String, AppCommandError> {
    let picked = match provider_id
        .and_then(|id| settings.providers.iter().find(|p| p.id == id))
        .filter(|p| p.is_complete())
        .cloned()
    {
        Some(provider) => pool::standalone(provider),
        None => pick_provider(settings).await?,
    };
    translate_one(
        prompt::TEST_PHRASE,
        target_lang,
        &picked,
        Priority::Priority,
    )
    .await
    .result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_response_yields_its_content() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"你好"}}]}"#;
        assert_eq!(parse_translation(body.as_bytes()).expect("parses"), "你好");
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_translation(b"{not json").is_err());
    }

    /// A syntactically valid envelope with nothing in it must not surface as
    /// an empty translation — that would cache "" and blank the message.
    #[test]
    fn an_empty_envelope_is_an_error() {
        for body in [
            &br#"{"choices":[]}"#[..],
            &br#"{"choices":[{}]}"#[..],
            &br#"{"choices":[{"message":{}}]}"#[..],
            &br#"{}"#[..],
        ] {
            assert!(
                parse_translation(body).is_err(),
                "an envelope with no content must be an error"
            );
        }
    }

    /// Reasoning distills inline their chain of thought; serving it would pour
    /// `<think>` rambling (and mangled placeholders) into the message.
    #[test]
    fn reasoning_blocks_are_stripped_from_the_translation() {
        let body = r#"{"choices":[{"message":{"content":"<think>\nThe user wants Chinese. Okay.\n</think>\n你好世界"}}]}"#;
        assert_eq!(parse_translation(body.as_bytes()).expect("parses"), "你好世界");
    }

    #[test]
    fn prose_before_the_reasoning_block_survives() {
        let body = r#"{"choices":[{"message":{"content":"注 <think>hm</think> 译"}}]}"#;
        // Strip keeps the prose around the block, then trims the ends; the
        // double space the splice leaves in the middle stays as-is.
        assert_eq!(
            parse_translation(body.as_bytes()).expect("parses"),
            "注  译"
        );
    }
    #[test]
    fn a_truncated_reasoning_block_is_an_error_not_a_leak() {
        // max_tokens cut the model off mid-think: there is no answer behind it.
        let body = r#"{"choices":[{"message":{"content":"<think>the translation should be"}}]}"#;
        let err = parse_translation(body.as_bytes()).expect_err("must not serve bare reasoning");
        assert!(err.message.contains("only reasoning"));
    }

    #[test]
    fn plain_content_is_untouched() {
        let body = r#"{"choices":[{"message":{"content":"你好"}}]}"#;
        assert_eq!(parse_translation(body.as_bytes()).expect("parses"), "你好");
    }

    /// The observed relay behavior: the guard model answers as itself
    /// ("I'm Mistral, …") or states a translation policy instead of
    /// translating. These fail at parse time with a precise message, never
    /// as a "successful" English translation the gates must catch later.
    #[test]
    fn a_refusal_reply_is_a_parse_failure_not_a_translation() {
        for content in [
            "I'm Mistral, a Large Language Model created by Mistral AI. I can't provide a detailed explanation.",
            "I can only translate text into Simplified Chinese. Please provide the text you'd like me to translate.",
            "I don't have the capability to provide that.",
        ] {
            let body = format!(r#"{{"choices":[{{"message":{{"content":"{content}"}}}}]}}"#);
            let err = parse_translation(body.as_bytes()).expect_err("refusal must fail");
            assert!(
                err.message.contains("refused"),
                "message was: {}",
                err.message
            );
        }
    }

    /// The gate keeps its head: prose that merely starts with "I" — including
    /// a translation that legitimately opens with a first-person sentence —
    /// is not a refusal.
    #[test]
    fn first_person_prose_is_not_a_refusal() {
        assert!(!refusal_shape("I'm going to explain how merges work."));
        assert!(!refusal_shape("I am a merge commit with two parents."));
        assert!(!refusal_shape("你好，这是一条测试。"));
    }

    #[test]
    fn only_transport_and_server_failures_are_retried() {
        assert!(is_retryable(None), "a transport failure is worth a retry");
        assert!(is_retryable(Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR)));
        assert!(is_retryable(Some(reqwest::StatusCode::BAD_GATEWAY)));
        assert!(is_retryable(Some(reqwest::StatusCode::TOO_MANY_REQUESTS)));

        assert!(!is_retryable(Some(reqwest::StatusCode::UNAUTHORIZED)));
        assert!(!is_retryable(Some(reqwest::StatusCode::NOT_FOUND)));
        assert!(!is_retryable(Some(reqwest::StatusCode::BAD_REQUEST)));
    }

    /// The settings page shows these verbatim, so a wrong key and a wrong URL
    /// must not read the same.
    #[test]
    fn failures_are_classified_by_what_the_user_has_to_fix() {
        let auth = classify(reqwest::StatusCode::UNAUTHORIZED, "bad key");
        assert!(auth.message.contains("API key"));

        let missing = classify(reqwest::StatusCode::NOT_FOUND, "nope");
        assert!(missing.message.contains("base URL"));

        let server = classify(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "boom");
        assert!(server.message.contains("500"));
    }

    /// Error bodies reach the settings page; an endpoint that answers with a
    /// megabyte of HTML must not put all of it on screen.
    #[test]
    fn error_detail_is_bounded() {
        let err = classify(reqwest::StatusCode::INTERNAL_SERVER_ERROR, &"x".repeat(10_000));
        assert!(err.detail.unwrap_or_default().chars().count() <= 500);
    }

    #[test]
    fn an_empty_batch_makes_no_requests() {
        let settings = TranslationSettings::default();
        let out = tokio_test_block(translate_batch(
            &[],
            "zh-CN",
            &settings,
            Priority::Background,
        ));
        assert!(out.is_empty());
    }

    /// The lane gate is structural with the concurrent `translate_batch`: ten
    /// waiters each hold their permit across a yield, so the runtime genuinely
    /// overlaps them and the sampled peak pins the ≤lane-cap-in-flight
    /// contract (plan P-8 / G-1). Pacing itself lives in the pool's per-
    /// provider slots now; the gate only bounds requests in flight.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn the_gate_never_holds_more_than_lane_cap_permits() {
        let cap = Priority::Background.max_concurrent();
        let mut max_held = 0usize;
        let mut waiters = Vec::new();
        for _ in 0..10 {
            waiters.push(async {
                let _permit = gate(Priority::Background)
                    .acquire()
                    .await
                    .expect("gate open");
                // Park behind a yield so other waiters can claim the rest of
                // the pool before this one samples; without it each future
                // acquires, samples, and drops within a single poll.
                tokio::task::yield_now().await;
                cap - gate(Priority::Background).available_permits()
            });
        }

        let results = join_all(waiters).await;
        for held in results {
            max_held = max_held.max(held);
        }
        assert!(
            max_held <= cap,
            "{max_held} permits were held at once; the gate leaked"
        );
        assert_eq!(
            max_held, cap,
            "ten concurrent waiters must actually saturate the gate"
        );
    }

    /// A numeric `Retry-After` parses; a date-shaped one deliberately does
    /// not (the AIMD halving throttles without parking — see
    /// [`parse_retry_after`]).
    #[test]
    fn retry_after_parses_seconds_only() {
        assert_eq!(parse_retry_after("12"), Some(Duration::from_secs(12)));
        assert_eq!(parse_retry_after(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("soon"), None);
    }

    #[test]
    fn small_texts_keep_the_fast_client_timeout() {
        assert_eq!(request_timeout(0), READ_TIMEOUT);
        assert_eq!(request_timeout(2000), READ_TIMEOUT);
    }

    #[test]
    fn large_texts_scale_the_deadline_with_input_size() {
        assert_eq!(
            request_timeout(2001),
            SCALING_TIMEOUT_BASE + SCALING_TIMEOUT_PER_CHAR * 2001
        );
        // The largest chunk the frontend can send: 3000 chars → 120 s.
        assert_eq!(request_timeout(3000), Duration::from_secs(60 + 60));
    }

    #[test]
    fn anthropic_max_tokens_spans_the_documented_clamp() {
        assert_eq!(anthropic_max_tokens(0), 4096);
        assert_eq!(anthropic_max_tokens(100), 4096, "small texts hit the floor");
        assert_eq!(anthropic_max_tokens(5000), 11_024);
        assert_eq!(anthropic_max_tokens(1 << 20), 32768, "huge texts hit the cap");
    }

    #[test]
    fn openai_max_tokens_spans_the_documented_clamp() {
        assert_eq!(openai_max_tokens(0), 1024);
        assert_eq!(openai_max_tokens(100), 1224);
        assert_eq!(openai_max_tokens(3000), 7024);
        assert_eq!(openai_max_tokens(1 << 20), 8192, "huge texts hit the cap");
    }

    /// A `finish_reason: "length"` stop is a truncated translation; serving it
    /// would cache the half answer forever.
    #[test]
    fn a_length_stopped_translation_is_an_error() {
        let body =
            br#"{"choices":[{"message":{"content":"partial"},"finish_reason":"length"}]}"#;
        let err = parse_translation(body).expect_err("truncation must fail");
        assert!(err.message.contains("cut off"));
    }

    #[test]
    fn a_stop_finished_translation_is_accepted() {
        let body =
            br#"{"choices":[{"message":{"content":"full"},"finish_reason":"stop"}]}"#;
        assert_eq!(parse_translation(body).expect("parses"), "full");
    }

    #[test]
    fn anthropic_text_blocks_are_joined() {
        let good = br#"{"content":[{"type":"text","text":"A"},{"type":"text","text":"B"}],"stop_reason":"end_turn"}"#;
        assert_eq!(
            parse_anthropic_translation(good).expect("parses"),
            "AB",
            "adjacent text blocks concatenate with no separator"
        );
    }

    /// A `max_tokens` stop would cache a half translation and serve it
    /// forever; it must read as an error instead.
    #[test]
    fn a_truncated_anthropic_output_is_an_error() {
        let body =
            br#"{"content":[{"type":"text","text":"partial"}],"stop_reason":"max_tokens"}"#;
        let err = parse_anthropic_translation(body).expect_err("truncation must fail");
        assert!(err.message.contains("cut off"));
    }

    #[test]
    fn an_anthropic_envelope_with_no_text_is_an_error() {
        for body in [
            br#"{"content":[],"stop_reason":"end_turn"}"#.as_slice(),
            br#"{"content":[{"type":"tool_use"}],"stop_reason":"end_turn"}"#.as_slice(),
            br#"{}"#.as_slice(),
        ] {
            assert!(
                parse_anthropic_translation(body).is_err(),
                "an envelope with no text must be an error"
            );
        }
    }

    #[test]
    fn anthropic_requests_carry_the_versioned_key_headers() {
        let headers = auth_headers(ApiFormat::Anthropic, "sk-ant");
        assert!(headers.contains(&("x-api-key", "sk-ant".to_string())));
        assert!(headers.contains(&("anthropic-version", ANTHROPIC_VERSION.to_string())));
    }

    #[test]
    fn an_empty_key_sends_no_bearer_header() {
        assert!(auth_headers(ApiFormat::Ollama, "").is_empty());
        assert!(auth_headers(ApiFormat::Openai, "   ").is_empty());
        assert_eq!(
            auth_headers(ApiFormat::Openai, "sk-openai"),
            vec![("Authorization", "Bearer sk-openai".to_string())]
        );
    }

    #[test]
    fn models_parse_from_the_openai_data_shape() {
        let body = br#"{"object":"list","data":[{"id":"gpt-4o-mini"},{"id":"gpt-4o"}]}"#;
        assert_eq!(
            parse_models(body).expect("parses"),
            vec!["gpt-4o", "gpt-4o-mini"]
        );
    }

    #[test]
    fn models_parse_from_the_models_shape() {
        let body = br#"{"models":[{"name":"qwen2.5:14b"},{"name":"llama3"}]}"#;
        assert_eq!(
            parse_models(body).expect("parses"),
            vec!["llama3", "qwen2.5:14b"]
        );
    }

    #[test]
    fn models_parse_from_a_bare_array() {
        let body = br#"["m1", "m2"]"#;
        assert_eq!(parse_models(body).expect("parses"), vec!["m1", "m2"]);
    }

    #[test]
    fn an_empty_model_list_is_ok_not_an_error() {
        assert!(parse_models(br#"{"data":[]}"#).expect("empty ok").is_empty());
    }

    #[test]
    fn models_skip_blank_and_non_string_ids() {
        // `"junk"` is a bare string element, which counts as a name; objects
        // without an id/name and non-string `id` values are skipped.
        let body = br#"{"data":[{"id":"  "},{"id":"ok"},{},"junk",{"id":42},{"name":"by-name"}]}"#;
        assert_eq!(
            parse_models(body).expect("parses"),
            vec!["by-name", "junk", "ok"]
        );
    }

    #[test]
    fn models_deduplicate_and_cap_at_500() {
        let body = br#"{"data":[{"id":"a"},{"id":"a"},{"id":"b"}]}"#;
        assert_eq!(parse_models(body).expect("parses"), vec!["a", "b"]);

        let many: Vec<_> = (0..600).map(|i| format!(r#"{{"id":"m{i}"}}"#)).collect();
        let body = format!(r#"{{"data":[{}]}}"#, many.join(","));
        assert_eq!(parse_models(body.as_bytes()).expect("parses").len(), 500);
    }

    #[test]
    fn malformed_model_json_is_an_error() {
        assert!(parse_models(b"{not json").is_err());
        assert!(parse_models(br#"{"nope":1}"#).is_err());
    }

    fn tokio_test_block<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(future)
    }
}
