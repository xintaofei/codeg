//! Outbound calls to the user's translation endpoint.
//!
//! Deliberately narrow: one POST per text, sent to the single endpoint
//! selected by [`crate::translation::endpoint`] and paced by per-lane
//! concurrency gates. The endpoint belongs to the user and may be a small
//! self-hosted model, so the gates exist to keep codeg from being the reason
//! it falls over.
//!
//! This module is the single ACCEPTANCE OWNER for a reply (PR1): a parseable
//! response becomes a translation only after the quality gates accept it.
//! The gates run here — before any ok/success report, before the caller can
//! write the cache — so a parseable but refused reply is reported as a gate
//! rejection and a failure, never as transport success.
//!
//! Dialects (`ApiFormat`) riding this one path: OpenAI-compatible requests
//! for openai/gemini/ollama (their compat surfaces differ only in URL and
//! auth), and Anthropic's native `/v1/messages` for anthropic, whose host
//! publishes no OpenAI route.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};
use tokio::time::sleep;

use crate::app_error::AppCommandError;
use crate::translation::endpoint;
use crate::translation::metrics::{translation_metrics, AttemptKind};
use crate::translation::prompt;
use crate::translation::settings::{ApiFormat, ProviderConfig, TranslationSettings};
use crate::translation::{
    display_language, is_numbered_request, normalize_protocol_marker_echo, quality_gate_error,
};

/// Two lanes, each a plain concurrency cap: visible prose and user-initiated
/// translation ride the priority lane; background thinking-block translation
/// shares whatever endpoint capacity is left, so a backlog of settled
/// thinking blocks can never delay the reply body a reader is waiting on.
/// Built-in lane sizes, in force while the user has not set an explicit cap
/// (`priority_max_concurrent` / `background_max_concurrent` in settings).
const DEFAULT_PRIORITY_MAX_CONCURRENT: usize = 4;
const DEFAULT_BACKGROUND_MAX_CONCURRENT: usize = 3;
/// Transport-level and 5xx retries, in place: PR1 has exactly one endpoint,
/// so a retry can only go back to it. A 429 is not retried at all — the
/// failure streak and the frontend's bounded retry own the recovery.
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Generous on purpose: reasoning models (deepseek-r1 distills and friends)
/// spend tens of seconds *thinking* about even a short translation, so a flat
/// 60 s reads as "endpoint broken" when the endpoint is merely slow.
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a request may stay in flight before it is reported as slow: the
/// reader is visibly waiting at this mark, long before the reply lands and
/// the true latency could be judged. Deliberately far below READ_TIMEOUT —
/// the hard timeout judges a broken endpoint, this judges a reader-visible
/// wait — and deliberately generous: reasoning relays routinely take 10-20 s
/// on a normal batch.
const SLOW_INFLIGHT: Duration = Duration::from_secs(30);
/// Well under the ~30-60 s idle cutoff CDNs apply to keep-alive connections:
/// a pooled connection older than this is evicted instead of failing the next
/// request the instant it is reused.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// Per-request deadline scaling. A slow endpoint needs real time to
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
/// 3000-char request cap the frontend enforces. Applies to error bodies too —
/// an endpoint that answers a bad key with a megabyte of HTML is the same
/// threat.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// The deadline one chat request may take, scaled to how much text it carries.
/// Chunks at or under the threshold keep the flat [`READ_TIMEOUT`]; larger ones
/// earn `60 s + 20 ms/char` (3000 chars → 120 s).
fn request_timeout(text_chars: usize) -> Duration {
    if text_chars <= SCALING_TIMEOUT_THRESHOLD_CHARS {
        return READ_TIMEOUT;
    }
    SCALING_TIMEOUT_BASE.saturating_add(SCALING_TIMEOUT_PER_CHAR.saturating_mul(text_chars as u32))
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
    /// The lane this priority rides, indexed into [`LANES`]. Kept adjacent to
    /// the lane table so a new variant cannot silently index out of bounds.
    fn lane_index(self) -> usize {
        match self {
            Priority::Priority => 0,
            Priority::Background => 1,
        }
    }
}

/// One lane's live concurrency gate. The semaphore sits behind an `Arc` so the
/// cap can change at runtime while in-flight requests keep the old permits
/// alive: a permit is acquired from and released back to the *same* `Arc`, so
/// an old, smaller semaphore simply drains and dies once the last request
/// holding it finishes.
struct LaneState {
    cap: usize,
    semaphore: Arc<Semaphore>,
}

/// Both lanes, indexed by [`Priority::lane_index`]. Caps are user settings, so
/// they can change between requests; the write lock swaps in a fresh
/// semaphore only when the desired cap differs from the live one.
static LANES: OnceLock<RwLock<[LaneState; 2]>> = OnceLock::new();

/// The semaphore enforcing `desired_cap` for this lane right now. Read lock
/// hits are a plain clone; a cap change takes the write lock and replaces the
/// semaphore whole. A briefly oversubscribed lane (permits from the old
/// semaphore still in flight while the new one is already open) is harmless —
/// the lane exists to mask round-trip latency, not to enforce a hard atom
/// across a settings change.
fn lane_semaphore(priority: Priority, desired_cap: usize) -> Arc<Semaphore> {
    let lanes = LANES.get_or_init(|| {
        RwLock::new([
            LaneState {
                cap: DEFAULT_PRIORITY_MAX_CONCURRENT,
                semaphore: Arc::new(Semaphore::new(DEFAULT_PRIORITY_MAX_CONCURRENT)),
            },
            LaneState {
                cap: DEFAULT_BACKGROUND_MAX_CONCURRENT,
                semaphore: Arc::new(Semaphore::new(DEFAULT_BACKGROUND_MAX_CONCURRENT)),
            },
        ])
    });
    let mut guard = match lanes.write() {
        Ok(guard) => guard,
        // A poisoned table still holds valid lane state; the panic that
        // poisoned it happened elsewhere and must not take translation down.
        Err(poisoned) => poisoned.into_inner(),
    };
    let lane = &mut guard[priority.lane_index()];
    if lane.cap != desired_cap {
        lane.cap = desired_cap;
        lane.semaphore = Arc::new(Semaphore::new(desired_cap));
    }
    lane.semaphore.clone()
}

/// The cap in force for this lane: an explicit user setting wins, `None`
/// follows the built-in ceiling.
fn lane_cap(priority: Priority, settings: &TranslationSettings) -> usize {
    match priority {
        Priority::Priority => settings
            .priority_max_concurrent
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_PRIORITY_MAX_CONCURRENT),
        Priority::Background => settings
            .background_max_concurrent
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_BACKGROUND_MAX_CONCURRENT),
    }
}

/// Acquire one lane permit. The permit is *owned* (it carries its semaphore's
/// `Arc` with it), so releasing it returns it to the exact semaphore it came
/// from even if the user changed the cap and the lane table swapped in a
/// fresh one while the request was in flight.
async fn lane_acquire(
    priority: Priority,
    settings: &TranslationSettings,
) -> Result<OwnedSemaphorePermit, AcquireError> {
    lane_semaphore(priority, lane_cap(priority, settings))
        .acquire_owned()
        .await
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
        // within seconds without telling us. Reusing one of those corpses
        // fails the request instantly — hyper does not retry a POST — so
        // evict idle connections well before the CDN does and let TCP
        // keepalive notice real breaks.
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
        // Rate limiting is the one 4xx that a wait can fix — but a 429 also
        // means "back off NOW", so it is retried via the caller's bounded
        // retry on a fresh request, never immediately in place.
        Some(status) => status.is_server_error(),
        // Transport-level failure (timeout, connection reset).
        None => true,
    }
}

/// One warn line per failed request, with the classified message and whatever
/// detail the endpoint's body carried. The renderer discards failed
/// translations silently (the message simply stays in its original language),
/// so this log is the only place the *why* is visible.
fn log_failure(stage: &str, error: &AppCommandError) {
    tracing::warn!(
        "[translation] {} failed: {}{}",
        stage,
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
            AppCommandError::authentication_failed("The translation service rejected the API key")
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

/// The auth headers a request to `provider`'s endpoint carries. Anthropic
/// signs with `x-api-key` plus a pinned protocol version; the
/// OpenAI-compatible dialects use a bearer token, and an empty key (local
/// Ollama) sends none.
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

/// The final state of one chunk's dispatch. `result` is `Ok` ONLY for a
/// reply the quality gate accepted (already marker-normalized — the text the
/// cache may store); every other outcome (transport failure, parse failure,
/// gate rejection) is an `Err` whose message the caller surfaces verbatim.
/// The latency is the round trip of the deciding attempt.
pub struct ChunkOutcome {
    pub result: Result<String, AppCommandError>,
    pub latency_ms: u64,
}

/// One text in, one translation out, all attempts against the one endpoint.
/// Transport and 5xx failures are retried per [`RETRY_BACKOFF`].
///
/// ACCEPTANCE (single owner, PR1): a parseable reply is normalized
/// (`normalize_protocol_marker_echo`), then judged by the quality gates
/// BEFORE anything is reported or handed back. Accepted → `record_attempt`
/// Ok + a success report that resets the failure streak. Refused →
/// `record_gate_rejection` (never Ok) + a failure report that counts toward
/// the automatic cooldown. The request speaks the endpoint's dialect
/// ([`ApiFormat`]).
///
/// `text` is the FULL outbound (context reference and `<translate>` envelope
/// included — that is what the endpoint must see); `body` is the stripped
/// source the gates and the cache key judge.
async fn translate_one(
    text: &str,
    body: &str,
    target_lang: &str,
    provider: &ProviderConfig,
    priority: Priority,
    settings: &TranslationSettings,
    trace: Option<&str>,
) -> ChunkOutcome {
    let client = match http_client() {
        Ok(client) => client,
        Err(err) => {
            return ChunkOutcome {
                result: Err(err),
                latency_ms: 0,
            }
        }
    };
    let system = prompt::system_prompt(display_language(target_lang));
    let timeout = request_timeout(text.chars().count());

    let mut attempt = 0;
    loop {
        let _permit = match lane_acquire(priority, settings).await {
            Ok(permit) => permit,
            Err(_) => {
                return ChunkOutcome {
                    result: Err(AppCommandError::task_execution_failed(
                        "Translation gate closed",
                    )),
                    latency_ms: 0,
                }
            }
        };

        // The trace id (the calling UI block) tags every log line a request
        // produces, so one block's traffic can be picked out of a mixed log —
        // the difference between "an endpoint failed" and "YOUR paragraph
        // failed, three times, on this endpoint".
        let tag = trace
            .filter(|trace| !trace.is_empty())
            .map(|trace| format!("[{trace}] "))
            .unwrap_or_default();
        tracing::debug!(
            "[translation] {tag}sending {} chars to {} (lane {:?}, attempt {}): {}",
            text.chars().count(),
            provider.provider_id(),
            priority,
            attempt + 1,
            text.chars().take(200).collect::<String>()
        );
        let url = provider.chat_completions_url();
        let format = provider.resolve_format();
        let request_body = match format {
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
        let started = std::time::Instant::now();
        let mut request = client.post(&url).timeout(timeout);
        for (name, value) in auth_headers(format, &provider.api_key) {
            request = request.header(name, value);
        }
        // The soft in-flight deadline: the request keeps waiting for its full
        // budget, but at the mark the metrics learn the endpoint is slow — an
        // endpoint answering in 90 s held a Background-lane slot for the whole
        // round trip while every queued chunk waited on it, and the reader
        // has been staring at untranslated text the entire time.
        let pending = request.json(&request_body).send();
        tokio::pin!(pending);
        let slow_mark = tokio::time::Instant::now() + SLOW_INFLIGHT;
        let mut slow_reported = false;
        let outcome = loop {
            tokio::select! {
                biased;
                response = &mut pending => break response,
                _ = tokio::time::sleep_until(slow_mark), if !slow_reported => {
                    slow_reported = true;
                    let elapsed = started.elapsed().as_millis() as u64;
                    tracing::warn!(
                        "[translation] request to {tag}{} still in flight after {}ms",
                        provider.provider_id(),
                        elapsed
                    );
                    translation_metrics().record_attempt(
                        AttemptKind::SlowInflight,
                        elapsed,
                    );
                }
            }
        };
        let latency = started.elapsed().as_millis() as u64;

        // Per-attempt recording: a retried chunk shows every attempt, which
        // is what pacing analysis needs.
        translation_metrics().record_dispatch();

        let error = match outcome {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let bytes =
                        match read_capped(response, MAX_RESPONSE_BYTES, "translation").await {
                            Ok(bytes) => bytes,
                            Err(err) => {
                                endpoint::report_failure(settings);
                                log_failure("read the translation response", &err);
                                return ChunkOutcome {
                                    result: Err(err),
                                    latency_ms: latency,
                                };
                            }
                        };
                    let parsed = match format {
                        ApiFormat::Anthropic => parse_anthropic_translation(&bytes),
                        _ => parse_translation(&bytes),
                    };
                    match &parsed {
                        Err(err) => log_failure(
                            &format!("{tag}parse the translation response"),
                            err,
                        ),
                        // DEBUG diagnostics for the "endpoint answers fine but
                        // nothing renders" class of report: the frontend
                        // discards a translation whose placeholders drifted,
                        // and this snippet is where the drift is visible.
                        Ok(translated) => tracing::debug!(
                            "[translation] {tag}response from the endpoint in {latency}ms: {}",
                            translated.chars().take(400).collect::<String>()
                        ),
                    }
                    return match parsed {
                        // ACCEPTANCE OWNER: normalize, gate, and only then
                        // report. The ok report and the success streak reset
                        // live behind the gate; a refused reply is recorded
                        // as a gate rejection and a failure instead.
                        Ok(translation) => {
                            let translation = if is_numbered_request(body) {
                                translation
                            } else {
                                normalize_protocol_marker_echo(&translation, body)
                            };
                            match quality_gate_error(body, &translation, target_lang) {
                                Some((rejection, message)) => {
                                    translation_metrics()
                                        .record_gate_rejection(rejection, latency);
                                    endpoint::report_failure(settings);
                                    let err =
                                        AppCommandError::task_execution_failed(message.clone());
                                    log_failure(&format!("{tag}quality gate refused the reply"),
                                        &err);
                                    ChunkOutcome {
                                        result: Err(err.with_detail(message)),
                                        latency_ms: latency,
                                    }
                                }
                                None => {
                                    translation_metrics()
                                        .record_attempt(AttemptKind::Ok, latency);
                                    endpoint::report_success();
                                    ChunkOutcome {
                                        result: Ok(translation),
                                        latency_ms: latency,
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            // A parse failure is a terminal failure like any
                            // other: the endpoint answered, but not with a
                            // translation.
                            endpoint::report_failure(settings);
                            if err.message.contains("cut off") {
                                translation_metrics().record_truncated();
                            }
                            translation_metrics().record_attempt(
                                AttemptKind::ParseError,
                                latency,
                            );
                            ChunkOutcome {
                                result: Err(err),
                                latency_ms: latency,
                            }
                        }
                    };
                }
                let error_body = match read_capped(response, MAX_RESPONSE_BYTES, "error").await {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    // The status is still known: classify on it alone rather
                    // than losing the verdict to a body that would not read.
                    Err(_) => String::new(),
                };
                let err = classify(status, &error_body);
                // Any non-success HTTP verdict is this endpoint's failure — a
                // bad key and a 500 both mean "this endpoint cannot serve
                // right now"; the failure streak decides what that earns.
                endpoint::report_failure(settings);
                log_failure(
                    &format!("{tag}endpoint answered HTTP {status}"),
                    &err,
                );
                let kind = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    AttemptKind::RateLimited
                } else {
                    AttemptKind::HttpError
                };
                translation_metrics().record_attempt(kind, latency);
                if !is_retryable(Some(status)) {
                    return ChunkOutcome {
                        result: Err(err),
                        latency_ms: latency,
                    };
                }
                err
            }
            Err(err) => {
                let mapped = AppCommandError::network("The translation request failed")
                    .with_detail(err.to_string());
                endpoint::report_failure(settings);
                log_failure("request to the translation endpoint", &mapped);
                translation_metrics().record_attempt(AttemptKind::NetworkError, latency);
                if !is_retryable(err.status()) {
                    return ChunkOutcome {
                        result: Err(mapped),
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
            // The retry budget is spent: this attempt's own error IS the last
            // error, and the actionable verdict to surface.
            None => {
                return ChunkOutcome {
                    result: Err(error),
                    latency_ms: latency,
                };
            }
        }
    }
}

/// Translate every text against the one endpoint, preserving order. One
/// request per text, issued concurrently — `join_all` (not `try_join_all`)
/// so one failed chunk does not cancel the others' already-spent work.
/// Per-chunk results, not all-or-nothing: a rate-limited endpoint (whose
/// failed attempts count against the limit, so a burst fails *some* chunks)
/// would otherwise throw away every chunk that succeeded. The caller caches
/// the accepted replies and only re-requests the failures.
///
/// `texts` are the FULL outbound bodies; `bodies` are their stripped sources,
/// positionally aligned — the gates judge `bodies[i]` while the endpoint sees
/// `texts[i]`.
pub async fn translate_batch(
    texts: &[String],
    bodies: &[String],
    target_lang: &str,
    provider: &ProviderConfig,
    settings: &TranslationSettings,
    priority: Priority,
    trace: Option<&str>,
) -> Vec<ChunkOutcome> {
    debug_assert_eq!(
        texts.len(),
        bodies.len(),
        "the stripped body list must align with the outbound texts"
    );
    join_all(texts.iter().zip(bodies.iter()).map(|(text, body)| async move {
        translate_one(
            text,
            body,
            target_lang,
            provider,
            priority,
            settings,
            trace,
        )
        .await
    }))
    .await
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

    let choice = parsed.choices.into_iter().next().ok_or_else(|| {
        AppCommandError::network("The translation service returned no translation")
    })?;

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
    // counted as transport success, so the ok counter and the failure streak
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
        .and_then(|id| {
            settings
                .providers
                .iter()
                .find(|p| p.id == id && candidate(p))
        })
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
    // Both the list body and an error body go through the streaming cap: an
    // endpoint that answers a bad key with a megabyte of HTML is the same
    // memory threat as one that streams an unbounded list.
    let bytes = match read_capped(response, MAX_RESPONSE_BYTES, "model list").await {
        Ok(bytes) => bytes,
        Err(err) if status.is_success() => return Err(err),
        // An error status whose body itself failed to read: classify on the
        // status alone rather than losing the verdict.
        Err(_) => Vec::new(),
    };
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).into_owned();
        return Err(if status == reqwest::StatusCode::NOT_FOUND {
            AppCommandError::configuration_invalid(
                "This endpoint does not expose a model list — enter the model name manually",
            )
            .with_detail(body.chars().take(500).collect::<String>())
        } else {
            classify(status, &body)
        });
    }

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
    .ok_or_else(|| AppCommandError::network("The model list response was not recognised"))?;

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

/// The settings page's connection test, aimed at ONE provider (the row being
/// edited, identified by its stable id). An incomplete or unknown row is an
/// error — the test must judge exactly the row it tests, never a success
/// borrowed from some other configured entry. Returns what the endpoint made
/// of [`prompt::TEST_PHRASE`], or a classified error the page can show
/// verbatim. Not subject to the failure cooldown: the whole point is to judge
/// the endpoint as it is right now.
pub async fn test_connection(
    settings: &TranslationSettings,
    target_lang: &str,
    provider_id: Option<&str>,
) -> Result<String, AppCommandError> {
    let provider = match provider_id {
        Some(id) => settings
            .providers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| {
                AppCommandError::configuration_missing(
                    "The provider being tested is no longer in the saved settings",
                )
            })?
            .clone(),
        // No row named: test what endpoint selection would actually use.
        None => endpoint::select_endpoint(settings)?,
    };
    if !provider.is_complete() {
        return Err(AppCommandError::configuration_missing(
            "Fill in the provider's base URL, API key, and model before testing",
        ));
    }
    translate_one(
        prompt::TEST_PHRASE,
        prompt::TEST_PHRASE,
        target_lang,
        &provider,
        Priority::Priority,
        settings,
        None,
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
        assert_eq!(
            parse_translation(body.as_bytes()).expect("parses"),
            "你好世界"
        );
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
    fn only_transport_and_server_failures_are_retried_in_place() {
        assert!(is_retryable(None), "a transport failure is worth a retry");
        assert!(is_retryable(Some(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        )));
        assert!(is_retryable(Some(reqwest::StatusCode::BAD_GATEWAY)));

        assert!(!is_retryable(Some(reqwest::StatusCode::UNAUTHORIZED)));
        assert!(!is_retryable(Some(reqwest::StatusCode::NOT_FOUND)));
        assert!(!is_retryable(Some(reqwest::StatusCode::BAD_REQUEST)));
        // A 429 means "back off NOW": the immediate retry would only feed the
        // limit, so it is left to the caller's bounded retry instead.
        assert!(!is_retryable(Some(reqwest::StatusCode::TOO_MANY_REQUESTS)));
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
        let err = classify(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            &"x".repeat(10_000),
        );
        assert!(err.detail.unwrap_or_default().chars().count() <= 500);
    }

    #[test]
    fn an_empty_batch_makes_no_requests() {
        let settings = TranslationSettings::default();
        let out = tokio_test_block(async {
            translate_batch(
                &[],
                &[],
                "zh-CN",
                &ProviderConfig::default(),
                &settings,
                Priority::Background,
                None,
            )
            .await
        });
        assert!(out.is_empty());
    }

    /// The lane gate is structural with the concurrent `translate_batch`: ten
    /// waiters each hold their permit across a yield, so the runtime genuinely
    /// overlaps them and the sampled peak pins the ≤cap-in-flight contract.
    /// The cap comes from settings now; a custom value must be honored
    /// exactly, not just the built-in default.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn the_gate_never_holds_more_than_lane_cap_permits() {
        let cap = 2usize;
        let semaphore = lane_semaphore(Priority::Background, cap);
        let mut max_held = 0usize;
        let mut waiters = Vec::new();
        for _ in 0..10 {
            let semaphore = semaphore.clone();
            waiters.push(async move {
                // Borrowed acquire: the permit lives on this waiter's own Arc
                // clone, so the sample below reads the same semaphore the
                // permit came from even if the lane table swaps meanwhile.
                let _permit = semaphore.acquire().await.expect("gate open");
                // Park behind a yield so other waiters can claim the rest of
                // the gate before this one samples; without it each future
                // acquires, samples, and drops within a single poll.
                tokio::task::yield_now().await;
                cap - semaphore.available_permits()
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

    /// A settings change must take effect on the *next* request: a smaller
    /// semaphore with a permit still held out must be swapped whole for a
    /// fresh one at the new cap, not silently ignored because the old state
    /// was initialized first. Uses the priority lane so it cannot race the
    /// background-lane saturation test over the shared lane table (tests run
    /// in parallel on separate runtimes).
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_cap_change_swaps_in_a_fresh_semaphore() {
        let old = lane_semaphore(Priority::Priority, 2);
        let held = old.clone().acquire_owned().await.expect("gate open");

        let new = lane_semaphore(Priority::Priority, 4);
        assert!(
            !Arc::ptr_eq(&old, &new),
            "a cap change must replace the semaphore, not reuse the old one"
        );
        assert_eq!(
            new.available_permits(),
            4,
            "the replacement opens at the new cap, not the old one's remaining permits"
        );

        // The old semaphore stays alive only through the held permit: when it
        // drops, the old gate drains and dies without leaking a permit into
        // the new one (which still shows its full 4).
        drop(held);
        assert_eq!(old.available_permits(), 2);
        assert_eq!(new.available_permits(), 4);
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
        assert_eq!(
            anthropic_max_tokens(1 << 20),
            32768,
            "huge texts hit the cap"
        );
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
        let body = br#"{"choices":[{"message":{"content":"partial"},"finish_reason":"length"}]}"#;
        let err = parse_translation(body).expect_err("truncation must fail");
        assert!(err.message.contains("cut off"));
    }

    #[test]
    fn a_stop_finished_translation_is_accepted() {
        let body = br#"{"choices":[{"message":{"content":"full"},"finish_reason":"stop"}]}"#;
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
        let body = br#"{"content":[{"type":"text","text":"partial"}],"stop_reason":"max_tokens"}"#;
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
        assert!(parse_models(br#"{"data":[]}"#)
            .expect("empty ok")
            .is_empty());
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

    // ─── Wire-level coverage ───────────────────────────────────────────────
    //
    // Same pattern as the stub-endpoint tests in `mod.rs`: raw loopback
    // listeners answering hand-rolled HTTP, so the retry and acceptance paths
    // run against real sockets.

    /// A loopback endpoint that answers 500 to every request until
    /// `set_ok(true)`, after which it answers a well-formed OpenAI chat
    /// reply, counting every request it saw.
    struct StubEndpoint {
        base_url: String,
        hits: Arc<std::sync::atomic::AtomicUsize>,
        ok: Arc<std::sync::atomic::AtomicBool>,
    }

    impl StubEndpoint {
        fn start() -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub endpoint");
            let port = listener.local_addr().expect("local addr").port();
            let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let ok = Arc::new(std::sync::atomic::AtomicBool::new(false));
            std::thread::spawn({
                let hits = Arc::clone(&hits);
                let ok = Arc::clone(&ok);
                move || {
                    use std::io::{Read, Write};
                    while let Ok((mut stream, _)) = listener.accept() {
                        let mut buf: Vec<u8> = Vec::new();
                        let mut chunk = [0u8; 8192];
                        loop {
                            match stream.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    buf.extend_from_slice(&chunk[..n]);
                                    // The request is complete once the headers
                                    // end and Content-Length payload bytes
                                    // have followed.
                                    let complete = buf
                                        .windows(4)
                                        .position(|w| w == b"\r\n\r\n")
                                        .map(|header_end| {
                                            let headers =
                                                String::from_utf8_lossy(&buf[..header_end])
                                                    .to_lowercase();
                                            let length = headers
                                                .lines()
                                                .find_map(|line| {
                                                    line.strip_prefix("content-length:")?
                                                        .trim()
                                                        .parse::<usize>()
                                                        .ok()
                                                })
                                                .unwrap_or(0);
                                            buf.len() >= header_end + 4 + length
                                        })
                                        .unwrap_or(false);
                                    if complete {
                                        break;
                                    }
                                }
                            }
                        }
                        hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let (status_line, reply) = if ok.load(std::sync::atomic::Ordering::SeqCst) {
                            (
                                "HTTP/1.1 200 OK",
                                r#"{"choices":[{"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}]}"#,
                            )
                        } else {
                            ("HTTP/1.1 500 Internal Server Error", "boom")
                        };
                        let response = format!(
                            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                            reply.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                }
            });
            StubEndpoint {
                base_url: format!("http://127.0.0.1:{port}"),
                hits,
                ok,
            }
        }

        fn set_ok(&self, ok: bool) {
            self.ok.store(ok, std::sync::atomic::Ordering::SeqCst);
        }

        fn hits(&self) -> usize {
            self.hits
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn endpoint_settings(base_url: &str) -> TranslationSettings {
        TranslationSettings {
            enabled: true,
            providers: vec![ProviderConfig {
                id: "only".to_string(),
                name: None,
                base_url: format!("{base_url}/v1"),
                api_key: "sk-test".to_string(),
                model: "stub".to_string(),
                api_format: "openai".to_string(),
                enabled: true,
                rpm_cap: None,
            }],
            ..Default::default()
        }
    }

    /// A 5xx is retried in place (PR1 has nowhere else to go) and the last
    /// error surfaces when the retry budget is spent.
    #[tokio::test]
    async fn a_5xx_is_retried_in_place_and_the_last_error_surfaces() {
        endpoint::reset_session();
        let x = StubEndpoint::start();
        let settings = endpoint_settings(&x.base_url);
        let provider = settings.providers[0].clone();

        let outcome = translate_one(
            "hello world",
            "hello world",
            "zh-CN",
            &provider,
            Priority::Background,
            &settings,
            None,
        )
        .await;

        let err = outcome.result.expect_err("the endpoint never recovers");
        assert!(err.message.contains("500"), "{err:?}");
        // One initial attempt plus both backoff retries, all on the endpoint.
        assert_eq!(x.hits(), 3);
        endpoint::reset_session();
    }

    /// A 5xx is retried in place; once the endpoint recovers inside the
    /// retry budget the reply is accepted like any other — the stub flips to
    /// 200 after the first refusal, so the second attempt must land.
    #[tokio::test]
    async fn a_5xx_followed_by_recovery_yields_the_accepted_reply() {
        endpoint::reset_session();
        let stub = StubEndpoint::start();
        let hits = Arc::clone(&stub.hits);
        let hits_for_assert = Arc::clone(&stub.hits);
        let settings = endpoint_settings(&stub.base_url);
        let provider = settings.providers[0].clone();

        let flipper = std::thread::spawn(move || {
            while hits.load(std::sync::atomic::Ordering::SeqCst) < 1 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            stub.set_ok(true);
        });

        let outcome = translate_one(
            "hello world",
            "hello world",
            "zh-CN",
            &provider,
            Priority::Background,
            &settings,
            None,
        )
        .await;

        flipper.join().expect("flipper joins");
        let translated = outcome.result.expect("the endpoint recovers mid-budget");
        assert_eq!(translated, "你好");
        assert_eq!(hits_for_assert.load(std::sync::atomic::Ordering::SeqCst), 2);
        endpoint::reset_session();
    }

    /// The acceptance owner, end to end: an endpoint that echoes the source
    /// back (parseable! transport-successful by shape!) must come back as a
    /// gate rejection — never as an ok, and never as text the caller could
    /// cache.
    #[tokio::test]
    async fn an_echo_reply_is_refused_by_the_acceptance_gate_not_served() {
        endpoint::reset_session();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub endpoint");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 8192];
                // Read exactly one request: headers end, then Content-Length
                // payload bytes. Waiting for EOF would deadlock — the client
                // is waiting for this very reply.
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                    if let Some(header_end) =
                        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
                    {
                        let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")?
                                    .trim()
                                    .parse::<usize>()
                                    .ok()
                            })
                            .unwrap_or(0);
                        if buf.len() >= header_end + length {
                            break;
                        }
                    }
                }
                // Echo whatever user content arrived, untouched. The user
                // message is the LAST `content` field in the request body —
                // the system prompt also carries one and must not be the
                // thing echoed back, or the length gate fires before the
                // echo gate ever sees the reply.
                let raw = String::from_utf8_lossy(&buf);
                let content = raw
                    .rsplit_once("\"content\":\"")
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .map(|(content, _)| content)
                    .unwrap_or("");
                let reply = format!(
                    r#"{{"choices":[{{"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}]}}"#
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let settings = endpoint_settings(&format!("http://127.0.0.1:{port}"));
        let provider = settings.providers[0].clone();
        let source = "The user asks an informational question about Git merge \
mechanics — this is a meta/educational query, exempt from the review gate.";

        let outcome = translate_one(
            source,
            source,
            "zh-CN",
            &provider,
            Priority::Background,
            &settings,
            None,
        )
        .await;

        let err = outcome
            .result
            .expect_err("an echo is not a translation");
        assert!(
            err.message.contains("echoed") || err.message.contains("target-language script"),
            "the gate's own verdict surfaces: {err:?}"
        );
        endpoint::reset_session();
    }

    fn tokio_test_block<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(future)
    }
}
