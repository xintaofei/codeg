//! Persisted configuration for the content-translation middleware.
//!
//! Stored as one JSON blob in `app_metadata` under [`TRANSLATION_SETTINGS_KEY`]
//! rather than in `model_provider`: that table's `validate_agent_type` forces
//! `agent_type` to name a real coding agent, which a translation endpoint is
//! not. A KV row needs no migration and carries no such constraint.

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::db::service::app_metadata_service;

pub const TRANSLATION_SETTINGS_KEY: &str = "translation_settings";

/// What a saved `api_key` is replaced with on the way out to the frontend. The
/// settings page shows this to mean "a key is stored"; sending it back
/// unchanged on save keeps the stored key (see [`merge_secret`]).
pub const API_KEY_MASK: &str = "••••••••";

const MAX_BASE_URL_LEN: usize = 2048;
const MAX_API_KEY_LEN: usize = 4096;
const MAX_MODEL_LEN: usize = 256;
const MAX_TARGET_LANG_LEN: usize = 32;
const MAX_PROVIDER_NAME_LEN: usize = 64;
/// Bounds for a provider's explicit requests-per-minute ceiling. The floor
/// keeps a typo (`2` is slow but intentional) from reading as "unset", and a
/// zero would divide the pacing math by nothing.
pub const RPM_CAP_MIN: u32 = 2;
pub const RPM_CAP_MAX: u32 = 600;

/// The `api_format` value that asks the backend to read the dialect off the
/// host. Stored rows written before the field existed deserialize to `""`,
/// which [`resolve_format`] treats the same way — hence no migration.
pub const API_FORMAT_AUTO: &str = "auto";

/// Everything `api_format` may hold. Anything else is a typo or a hand-edited
/// row, and is rejected on save rather than silently guessed at.
pub const KNOWN_API_FORMATS: [&str; 5] =
    [API_FORMAT_AUTO, "openai", "anthropic", "gemini", "ollama"];

// ─── Error messages asserted by tests ────────────────────────────────────
//
// The settings page shows these verbatim, and "the URL is wrong" has to read
// differently from "the scheme is wrong" or the user has nothing to act on.

pub const ERR_BASE_URL_TOO_LONG: &str = "Translation base URL is too long";
pub const ERR_BASE_URL_SCHEME: &str =
    "Translation base URL scheme must be http:// or https://";
pub const ERR_BASE_URL_INVALID: &str = "Translation base URL is not a valid URL";
pub const ERR_BASE_URL_NO_HOST: &str = "Translation base URL must include a host";
pub const ERR_UNKNOWN_API_FORMAT: &str = "Unknown translation API format";

/// Path suffixes that name a *route* rather than a base. Users paste whatever
/// their provider's docs show, which is usually the full chat endpoint; peeling
/// these off on save is what lets one stored value derive both the chat route
/// and the model-list route below.
const STRIPPED_PATH_SUFFIXES: [&str; 5] = [
    "/chat/completions",
    "/v1/messages",
    "/v1beta/openai",
    "/api/chat",
    "/api/generate",
];

/// Where Gemini mounts its OpenAI-compatible surface. codeg speaks that dialect
/// rather than Gemini's native one, so only the path differs.
const GEMINI_COMPAT_PATH: &str = "/v1beta/openai";

/// The port Ollama serves on, used as a detection hint when the host itself
/// gives nothing away (`http://192.168.1.5:11434`).
const OLLAMA_PORT_SUFFIX: &str = ":11434";

/// Drop a trailing `/v1` a dialect is about to re-add. Users paste what the
/// vendor's docs show — `localhost:11434/v1`, `api.anthropic.com/v1` — and the
/// dialect-specific derivations below append their own `/v1/...`, so the
/// OpenAI guard alone would leave those pastes doubled.
fn strip_trailing_v1(base: &str) -> &str {
    base.strip_suffix("/v1").unwrap_or(base)
}

/// Which wire dialect an endpoint speaks.
///
/// Only [`ApiFormat::Anthropic`] needs its own serialization: `api.anthropic.com`
/// exposes no OpenAI-compatible route. Gemini and Ollama both publish one
/// (`/v1beta/openai` and `/v1`), so they reuse the OpenAI request path and
/// differ only in how the URL is derived and how the request is authorized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiFormat {
    Openai,
    Anthropic,
    Gemini,
    Ollama,
}

impl ApiFormat {
    /// The stable identifier that participates in cache keys. Not `Debug`,
    /// which would tie the on-disk cache to a derive.
    pub fn as_str(self) -> &'static str {
        match self {
            ApiFormat::Openai => "openai",
            ApiFormat::Anthropic => "anthropic",
            ApiFormat::Gemini => "gemini",
            ApiFormat::Ollama => "ollama",
        }
    }
}

/// One translation endpoint in the rotation pool.
///
/// Rows written before the pool existed stored a single endpoint in the flat
/// [`TranslationSettings`] fields; [`migrate_legacy`] synthesizes the list
/// from those on read, so every code path after `load` sees the pool shape.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Stable identity for pool-state keying (the adaptive limiter's memory)
    /// and the settings page's list rows. Empty on entries synthesized from
    /// legacy fields; `validate` fills one in on save.
    #[serde(default)]
    pub id: String,
    /// Optional label shown in the settings page ("主力中转", "backup").
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    /// One of [`KNOWN_API_FORMATS`]; empty means the same as `"auto"`. Per
    /// provider, so an OpenAI-compatible relay and a native Anthropic endpoint
    /// can share one pool.
    #[serde(default)]
    pub api_format: String,
    /// Pool membership. The global `enabled` is still the master switch; a
    /// disabled provider is skipped by the rotation without being deleted.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Requests-per-minute ceiling the adaptive limiter may climb to. `None`
    /// lets the limiter explore on its own (start at [`crate::translation::aimd::AUTO_START_RPM`],
    /// ceiling [`crate::translation::aimd::AUTO_MAX_RPM`]).
    #[serde(default)]
    pub rpm_cap: Option<u32>,
}

impl ProviderConfig {
    /// The dialect in force for this endpoint: the explicit choice when one
    /// was made, otherwise read off the host.
    pub fn resolve_format(&self) -> ApiFormat {
        resolve_format(&self.base_url, &self.api_format)
    }

    /// Identifies the endpoint for pool-state keying: the same equivalence
    /// class the cache key used before the pool existed. Insensitive to base
    /// spelling (`https://host` ≡ `https://host/v1`), sensitive to dialect.
    pub fn provider_id(&self) -> String {
        format!("{}|{}", self.chat_completions_url(), self.model)
    }

    /// The base as [`normalize_base_url`] would store it, falling back to the
    /// plain trimmed value for rows saved before normalization existed. That
    /// keeps endpoint derivation working for legacy rows without a rewrite.
    fn normalized_base(&self) -> String {
        normalize_base_url(&self.base_url).unwrap_or_else(|_| {
            self.base_url
                .trim()
                .trim_end_matches('/')
                .to_string()
        })
    }

    /// One endpoint family, two routes: the chat path and its model-list
    /// sibling always share a shape, so a base that routes one routes both.
    ///
    /// Users paste an OpenAI-compatible base (`https://host/v1`, or just
    /// `https://host`); both must end up at the same route, and a base that
    /// already names a route is left alone so a non-standard mount still works.
    fn endpoint_url(&self, suffix: &str) -> String {
        let mut base = self.normalized_base();
        // Legacy rows may still store a full route; peel it off rather than
        // double it. normalize_base_url strips these on save, so this only
        // fires for values already on disk.
        for known in STRIPPED_PATH_SUFFIXES {
            if base.ends_with(known) {
                base = base[..base.len() - known.len()].to_string();
            }
        }

        match self.resolve_format() {
            ApiFormat::Openai => {
                if base.ends_with("/v1") {
                    format!("{base}/{suffix}")
                } else {
                    format!("{base}/v1/{suffix}")
                }
            }
            ApiFormat::Anthropic => {
                let base = strip_trailing_v1(&base);
                if suffix == "chat/completions" {
                    format!("{base}/v1/messages")
                } else {
                    format!("{base}/v1/{suffix}")
                }
            }
            ApiFormat::Ollama => {
                format!("{}/v1/{suffix}", strip_trailing_v1(&base))
            }
            ApiFormat::Gemini => {
                // The compat surface is fixed; honour a custom mount if the
                // user pointed at something other than the API origin.
                if !base.ends_with(GEMINI_COMPAT_PATH) {
                    base = format!("{}{GEMINI_COMPAT_PATH}", strip_trailing_v1(&base));
                }
                format!("{base}/{suffix}")
            }
        }
    }

    /// The POST target for a translation request.
    pub fn chat_completions_url(&self) -> String {
        self.endpoint_url("chat/completions")
    }

    /// The GET target for the model list — same base shape as
    /// [`Self::chat_completions_url`] by construction, so one probe validates
    /// both routes.
    pub fn models_url(&self) -> String {
        self.endpoint_url("models")
    }

    /// Whether this endpoint can serve a request at all: a base and a model,
    /// plus a key unless the dialect serves locally without one. Incomplete
    /// entries are kept as settings-page drafts; the pool skips them.
    pub fn is_complete(&self) -> bool {
        !self.base_url.is_empty()
            && !self.model.is_empty()
            && (self.resolve_format() == ApiFormat::Ollama || !self.api_key.is_empty())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranslationSettings {
    /// Off until the user supplies an endpoint. Every read path short-circuits
    /// on this, so a fresh install behaves exactly as it did before the
    /// feature existed.
    #[serde(default)]
    pub enabled: bool,
    /// The endpoint rotation pool. Legacy rows stored one endpoint in the flat
    /// fields below; [`migrate_legacy`] synthesizes a single-entry pool from
    /// those on read, so this list is the source of truth everywhere else.
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    /// `None` follows the interface locale.
    #[serde(default)]
    pub target_lang: Option<String>,
    #[serde(default)]
    pub translate_thinking: bool,
    /// One of [`KNOWN_API_FORMATS`]. Empty means the same as `"auto"` so rows
    /// written before this field existed keep working untouched. Legacy: the
    /// single-endpoint dialect, mirrored from `providers[0]` on save.
    #[serde(default)]
    pub api_format: String,
    /// Offer 翻译 in the text-selection bubble. Rows written before this field
    /// existed read as `false`, which would silently disable the action — so
    /// the default here is `true` via the custom serde default below.
    #[serde(default = "default_true")]
    pub selection_translate: bool,
    /// Target language for selection translation. `None` follows
    /// [`Self::target_lang`], the main setting; an explicit value lets the
    /// user translate selections somewhere else without moving the whole
    /// feature off its configured language.
    #[serde(default)]
    pub selection_target_lang: Option<String>,
    /// Render translation toggle buttons without waiting for a hover.
    #[serde(default)]
    pub toggle_always_visible: bool,
    /// Character ceiling for one outbound request body when the frontend
    /// coalesces small adjacent segments into one numbered request. `None`
    /// keeps the built-in default. (The retired manual pacing knobs
    /// `max_concurrent`/`min_request_interval_ms`/`stream_batch_units` were
    /// removed from the protocol; dispatch pacing is now the adaptive
    /// limiter's job.)
    #[serde(default)]
    pub batch_max_chars: Option<u32>,
    /// Prepend the previous segment's source and translation as a
    /// reference-only block, so terminology stays consistent across the
    /// independent per-segment requests. Default on; one request carries
    /// at most 500 source + 500 translated chars of context, so the
    /// per-request cost is flat regardless of document length.
    #[serde(default = "default_true")]
    pub carry_context: bool,
}

fn default_true() -> bool {
    true
}

impl TranslationSettings {
    /// The stored keys replaced by [`API_KEY_MASK`], for any value that leaves
    /// the backend. The real keys never reach the renderer.
    pub fn masked(&self) -> Self {
        let mut masked = Self {
            api_key: if self.api_key.is_empty() {
                String::new()
            } else {
                API_KEY_MASK.to_string()
            },
            ..self.clone()
        };
        for provider in &mut masked.providers {
            if !provider.api_key.is_empty() {
                provider.api_key = API_KEY_MASK.to_string();
            }
        }
        masked
    }

    /// The providers that may receive requests: pool members the user has not
    /// individually disabled, and complete enough to be callable. The global
    /// `enabled` gate is applied by the callers, not here — this answers
    /// "who is in the pool" once the feature is on.
    pub fn active_providers(&self) -> Vec<ProviderConfig> {
        if self.providers.is_empty() {
            // A legacy row read through [`migrate_legacy`] always has the list
            // filled; an empty list here means a default-constructed value
            // (tests, a fresh install) whose flat fields are the only truth.
            let legacy = ProviderConfig {
                base_url: self.base_url.clone(),
                api_key: self.api_key.clone(),
                model: self.model.clone(),
                api_format: self.api_format.clone(),
                enabled: true,
                ..Default::default()
            };
            return if legacy.is_complete() { vec![legacy] } else { Vec::new() };
        }
        self.providers
            .iter()
            .filter(|provider| provider.enabled && provider.is_complete())
            .cloned()
            .collect()
    }

    /// The endpoint rotation pool in force. Identifies the *pool* a cached
    /// translation came from: any member may have produced the answer, and any
    /// member may serve a later render of the same text — that sharing is the
    /// point of the pool, so the cache key uses one constant rather than any
    /// member's identity. (Member identity still keys the adaptive limiter's
    /// runtime state, which is not persisted.)
    pub fn provider_id(&self) -> String {
        "pool".to_string()
    }

    /// The dialect of the pool's first active member, for callers that need a
    /// single answer (error classification, the settings page's format
    /// display). Legacy single-endpoint settings delegate to the flat fields.
    pub fn resolve_format(&self) -> ApiFormat {
        if self.providers.is_empty() {
            return resolve_format(&self.base_url, &self.api_format);
        }
        self.active_providers()
            .first()
            .or_else(|| self.providers.first())
            .map(|provider| provider.resolve_format())
            .unwrap_or(ApiFormat::Openai)
    }

    /// The base as [`normalize_base_url`] would store it, falling back to the
    /// plain trimmed value for rows saved before normalization existed. That
    /// keeps endpoint derivation working for legacy rows without a rewrite.
    fn normalized_base(&self) -> String {
        normalize_base_url(&self.base_url).unwrap_or_else(|_| {
            self.base_url
                .trim()
                .trim_end_matches('/')
                .to_string()
        })
    }

    /// One endpoint family, two routes: the chat path and its model-list
    /// sibling always share a shape, so a base that routes one routes both.
    ///
    /// Users paste an OpenAI-compatible base (`https://host/v1`, or just
    /// `https://host`); both must end up at the same route, and a base that
    /// already names a route is left alone so a non-standard mount still works.
    fn endpoint_url(&self, suffix: &str) -> String {
        let mut base = self.normalized_base();
        // Legacy rows may still store a full route; peel it off rather than
        // double it. normalize_base_url strips these on save, so this only
        // fires for values already on disk.
        for known in STRIPPED_PATH_SUFFIXES {
            if base.ends_with(known) {
                base = base[..base.len() - known.len()].to_string();
            }
        }

        match self.resolve_format() {
            ApiFormat::Openai => {
                if base.ends_with("/v1") {
                    format!("{base}/{suffix}")
                } else {
                    format!("{base}/v1/{suffix}")
                }
            }
            ApiFormat::Anthropic => {
                let base = strip_trailing_v1(&base);
                if suffix == "chat/completions" {
                    format!("{base}/v1/messages")
                } else {
                    format!("{base}/v1/{suffix}")
                }
            }
            ApiFormat::Ollama => {
                format!("{}/v1/{suffix}", strip_trailing_v1(&base))
            }
            ApiFormat::Gemini => {
                // The compat surface is fixed; honour a custom mount if the
                // user pointed at something other than the API origin.
                if !base.ends_with(GEMINI_COMPAT_PATH) {
                    base = format!("{}{GEMINI_COMPAT_PATH}", strip_trailing_v1(&base));
                }
                format!("{base}/{suffix}")
            }
        }
    }

    /// The POST target for a translation request.
    pub fn chat_completions_url(&self) -> String {
        self.endpoint_url("chat/completions")
    }

    /// The GET target for the model list — same base shape as
    /// [`Self::chat_completions_url`] by construction, so one probe validates
    /// both routes.
    pub fn models_url(&self) -> String {
        self.endpoint_url("models")
    }
}

/// Which dialect applies: an explicit pin wins, otherwise read the host.
///
/// The heuristic covers what users actually paste — the vendor's own origin
/// (`api.anthropic.com`, `generativelanguage.googleapis.com`, `localhost:11434`)
/// and nothing subtler. A reverse proxy that hides the provider behind a
/// private domain is exactly what the explicit dropdown exists for; guessing
/// `Openai` there is the correct default because the OpenAI dialect is the
/// lingua franca of compat endpoints.
pub fn resolve_format(base_url: &str, api_format: &str) -> ApiFormat {
    match api_format.trim() {
        "" | API_FORMAT_AUTO => {}
        "openai" => return ApiFormat::Openai,
        "anthropic" => return ApiFormat::Anthropic,
        "gemini" => return ApiFormat::Gemini,
        "ollama" => return ApiFormat::Ollama,
        // Unreachable through `validate`, but a stored row may predate a
        // rename; falling back to detection beats panicking on read paths.
        _ => {}
    }

    let trimmed = base_url.trim();
    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let host_and_port = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let host = host_and_port
        .rsplit_once(':')
        // An IPv6 literal brackets its port; a bare `::1` has no port to split.
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map_or(host_and_port.as_str(), |(host, _)| host);
    let bare_host = host.trim_start_matches('[').trim_end_matches(']');

    if bare_host == "api.anthropic.com" {
        return ApiFormat::Anthropic;
    }
    if bare_host.contains("googleapis.com") || bare_host.contains("gemini") {
        return ApiFormat::Gemini;
    }
    if bare_host.contains("ollama") || host_and_port.ends_with(OLLAMA_PORT_SUFFIX) {
        return ApiFormat::Ollama;
    }
    ApiFormat::Openai
}

/// Keep the stored secret when the frontend echoes back the mask, and only
/// then. A user clearing the field really does mean "forget the key", which
/// an unconditional "empty means keep" would make impossible.
fn merge_secret(incoming: &str, stored: &str) -> String {
    if incoming == API_KEY_MASK {
        stored.to_string()
    } else {
        incoming.to_string()
    }
}
/// Whether a schemeless host is reached over plain http. Local and private
/// network endpoints (Ollama, llama.cpp, a LAN proxy) rarely serve TLS, while
/// anything routable from outside almost certainly does.
fn is_private_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host == "::1"
        || host.ends_with(".local")
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || is_private_172(&host)
}

/// The `172.16.0.0/12` block: `172.16.*` through `172.31.*`. The `/12` is easy
/// to miss — `172.32.*` is public and must not default to http.
fn is_private_172(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("172.") else {
        return false;
    };
    let Some((second, _)) = rest.split_once('.') else {
        return false;
    };
    second
        .parse::<u8>()
        .map(|octet| (16..=31).contains(&octet))
        .unwrap_or(false)
}

/// Turn whatever the user pasted into the canonical base the rest of the
/// module routes from — or `""`, which callers treat as "no endpoint yet".
///
/// Order matters: length first (a hostile input should not reach the parser),
/// then the scheme default, then real parsing, then cosmetic cleanup. The
/// output is what gets *stored*, so `provider_id` and both endpoint routes
/// stay stable across equally-valid spellings of the same endpoint.
pub fn normalize_base_url(raw: &str) -> Result<String, AppCommandError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.chars().count() > MAX_BASE_URL_LEN {
        return Err(AppCommandError::invalid_input(ERR_BASE_URL_TOO_LONG));
    }

    // No `://` means the scheme was never typed. Default it rather than
    // rejecting: `localhost:11434` is the single most common paste for local
    // model servers, and making the user type `http://` there is pure friction.
    let candidate = if trimmed.contains("://") {
        let (scheme, rest) = trimmed
            .split_once("://")
            .expect("checked for the separator above");
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(AppCommandError::configuration_invalid(ERR_BASE_URL_SCHEME)
                .with_detail(scheme));
        }
        format!("{scheme}://{rest}")
    } else {
        // Take the host as everything before the first path, query, or port
        // separator *after* any userinfo, so the private-host check below sees
        // the host it should. An unbracketed IPv6 literal (`::1`) gets brackets
        // added: without them the parser reads the colons as a port.
        let authority = trimmed.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let bracketed = host.starts_with('[');
        let host_for_guess = if bracketed {
            host.trim_start_matches('[')
                .split_once(']')
                .map_or(host, |(inner, _)| inner)
        } else if host.matches(':').count() > 1 {
            // More than one colon can only be an IPv6 address; a `host:port`
            // has exactly one.
            host
        } else {
            host.rsplit_once(':')
                .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
                .map_or(host, |(h, _)| h)
        };
        let scheme = if is_private_host(host_for_guess) {
            "http"
        } else {
            "https"
        };
        if !bracketed && host == authority && host.contains(':') && host_for_guess == host {
            let rest = &trimmed[authority.len()..];
            format!("{scheme}://[{authority}]{rest}")
        } else {
            format!("{scheme}://{trimmed}")
        }
    };

    let mut url = reqwest::Url::parse(&candidate)
        .map_err(|e| AppCommandError::invalid_input(ERR_BASE_URL_INVALID).with_detail(e.to_string()))?;

    if url.host_str().is_none_or(str::is_empty) {
        return Err(AppCommandError::configuration_invalid(ERR_BASE_URL_NO_HOST));
    }

    // A base is not a query target; whatever the docs page had in the address
    // bar does not belong in the stored value.
    url.set_query(None);
    url.set_fragment(None);

    // Peel a known route suffix (case-insensitively) so the stored value is a
    // true base and both endpoint derivations start from the same place. The
    // lowercased copy is only for matching: `to_ascii_lowercase` is
    // byte-length preserving, so the index it yields is valid in `path`.
    let mut path = url.path().trim_end_matches('/').to_string();
    let lower = path.to_ascii_lowercase();
    if let Some(known) = STRIPPED_PATH_SUFFIXES
        .iter()
        .find(|suffix| lower.ends_with(**suffix))
    {
        path.truncate(path.len() - known.len());
    }
    while path.ends_with('/') {
        path.pop();
    }
    url.set_path(&path);

    // `to_string()` re-renders the parsed form; trailing slashes here come from
    // an empty path (`https://host/`), not from the route stripping above.
    let mut out = url.to_string();
    while out.ends_with('/') {
        out.pop();
    }
    Ok(out)
}

/// Fill in an empty provider list from the legacy flat fields, so every code
/// path after `load` sees the pool shape regardless of what is on disk.
///
/// Runs on *read*, not on a stored-row rewrite: the flat fields stay the
/// source of truth for a row that has never been saved through the pool-aware
/// settings page, and `save` mirrors `providers[0]` back into them, so an old
/// build reading a new row (or vice versa) keeps working either way.
fn migrate_legacy(mut settings: TranslationSettings) -> TranslationSettings {
    if !settings.providers.is_empty() {
        return settings;
    }
    if settings.base_url.trim().is_empty() {
        return settings;
    }
    settings.providers.push(ProviderConfig {
        // Deterministic, not random: every load re-runs this migration until
        // the user saves, and a stable id is what lets the settings page's
        // masked key refill match the stored entry across reads (and what
        // keeps the pool's runtime state keyed consistently).
        id: "legacy".to_string(),
        name: None,
        base_url: settings.base_url.clone(),
        api_key: settings.api_key.clone(),
        model: settings.model.clone(),
        api_format: settings.api_format.clone(),
        enabled: true,
        rpm_cap: None,
    });
    settings
}

/// Trim, bound, and check coherence. Length caps exist because these strings
/// are echoed into a `app_metadata.value` row and an outbound HTTP request;
/// the `enabled` coupling is what keeps a turned-on feature from firing at an
/// endpoint it has no way to reach.
pub fn validate(settings: TranslationSettings) -> Result<TranslationSettings, AppCommandError> {
    let target_lang = settings
        .target_lang
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(lang) = target_lang.as_deref() {
        if lang.chars().count() > MAX_TARGET_LANG_LEN {
            return Err(AppCommandError::invalid_input(
                "Translation target language is too long",
            ));
        }
    }

    if settings.enabled && settings.active_providers().is_empty() {
        return Err(AppCommandError::configuration_missing(
            "Translation needs at least one enabled provider with a base URL, an API key, and a model",
        ));
    }

    // The batch ceiling clamps rather than refuses: a value outside the range
    // is a slip of the keyboard on a settings form, not a hostile payload, and
    // refusing the whole save over it would strand the endpoint config too.
    let batch_max_chars = settings
        .batch_max_chars
        .map(|value| value.clamp(500, 20_000));

    let mut providers = Vec::with_capacity(settings.providers.len());
    for provider in settings.providers {
        providers.push(validate_provider(provider)?);
    }

    // Mirror the pool head into the legacy flat fields, so a row carries both
    // shapes: old builds read the flat fields, pool-aware paths read the
    // list, and neither sees an endpoint the other cannot. An empty pool is
    // the legacy/default path — there the flat fields ARE the truth.
    if let Some(head) = providers.first() {
        let api_format = head.api_format.trim().to_string();
        if !api_format.is_empty() && !KNOWN_API_FORMATS.contains(&api_format.as_str()) {
            return Err(AppCommandError::configuration_invalid(ERR_UNKNOWN_API_FORMAT));
        }
        return Ok(TranslationSettings {
            enabled: settings.enabled,
            base_url: normalize_base_url(head.base_url.trim())?,
            api_key: head.api_key.trim().to_string(),
            model: head.model.trim().to_string(),
            target_lang,
            translate_thinking: settings.translate_thinking,
            api_format,
            selection_translate: settings.selection_translate,
            selection_target_lang: settings.selection_target_lang,
            toggle_always_visible: settings.toggle_always_visible,
            batch_max_chars,
            carry_context: settings.carry_context,
            providers,
        });
    }

    let api_format = settings.api_format.trim().to_string();
    if !api_format.is_empty() && !KNOWN_API_FORMATS.contains(&api_format.as_str()) {
        return Err(AppCommandError::configuration_invalid(ERR_UNKNOWN_API_FORMAT));
    }
    let api_key = settings.api_key.trim().to_string();
    let model = settings.model.trim().to_string();
    if api_key.chars().count() > MAX_API_KEY_LEN {
        return Err(AppCommandError::invalid_input(
            "Translation API key is too long",
        ));
    }
    if model.chars().count() > MAX_MODEL_LEN {
        return Err(AppCommandError::invalid_input(
            "Translation model name is too long",
        ));
    }
    Ok(TranslationSettings {
        enabled: settings.enabled,
        providers,
        base_url: normalize_base_url(settings.base_url.trim())?,
        api_key,
        model,
        target_lang,
        translate_thinking: settings.translate_thinking,
        api_format,
        selection_translate: settings.selection_translate,
        selection_target_lang: settings.selection_target_lang,
        toggle_always_visible: settings.toggle_always_visible,
        batch_max_chars,
        carry_context: settings.carry_context,
    })
}

/// Validate one pool member: trim, bound, normalize, keep its secret semantics
/// (the mask merge happens in `save`, on the flat mirror only — per-provider
/// keys use the same mask and the same merge rule there), and assign its
/// stable id when missing.
fn validate_provider(provider: ProviderConfig) -> Result<ProviderConfig, AppCommandError> {
    let base_url = normalize_base_url(provider.base_url.trim())?;
    let api_key = provider.api_key.trim().to_string();
    let model = provider.model.trim().to_string();
    let api_format = provider.api_format.trim().to_string();
    if !api_format.is_empty() && !KNOWN_API_FORMATS.contains(&api_format.as_str()) {
        return Err(AppCommandError::configuration_invalid(ERR_UNKNOWN_API_FORMAT));
    }
    if api_key.chars().count() > MAX_API_KEY_LEN {
        return Err(AppCommandError::invalid_input(
            "Translation API key is too long",
        ));
    }
    if model.chars().count() > MAX_MODEL_LEN {
        return Err(AppCommandError::invalid_input(
            "Translation model name is too long",
        ));
    }
    if let Some(name) = provider.name.as_deref() {
        if name.chars().count() > MAX_PROVIDER_NAME_LEN {
            return Err(AppCommandError::invalid_input(
                "Translation provider name is too long",
            ));
        }
    }
    let rpm_cap = provider.rpm_cap.map(|value| value.clamp(RPM_CAP_MIN, RPM_CAP_MAX));

    Ok(ProviderConfig {
        // A provider without an id gets one at validation time, so pool-state
        // keying survives every later save (the id, not the list position, is
        // what the adaptive limiter remembers).
        id: if provider.id.trim().is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            provider.id
        },
        name: provider
            .name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty()),
        base_url,
        api_key,
        model,
        api_format,
        enabled: provider.enabled,
        rpm_cap,
    })
}

/// The stored settings, with the real `api_key`. Callers that send this
/// anywhere near the frontend must go through [`TranslationSettings::masked`].
///
/// A malformed row reads as "not configured" rather than an error: the
/// translation path is an enhancement, and failing it hard would take the
/// message list down with it.
pub async fn load(conn: &DatabaseConnection) -> TranslationSettings {
    let raw = match app_metadata_service::get_value(conn, TRANSLATION_SETTINGS_KEY).await {
        Ok(Some(raw)) => raw,
        Ok(None) => return TranslationSettings::default(),
        Err(err) => {
            tracing::warn!("[translation] failed to read settings: {err}");
            return TranslationSettings::default();
        }
    };

    match serde_json::from_str::<TranslationSettings>(&raw) {
        Ok(settings) => migrate_legacy(settings),
        Err(err) => {
            tracing::warn!("[translation] stored settings are unreadable: {err}");
            TranslationSettings::default()
        }
    }
}

/// Validate, preserve the secrets the frontend masked out, and persist.
/// Returns the saved settings **masked**, ready to hand back to the caller.
///
/// Key merging runs per provider, matched by id: a row echoing the mask keeps
/// its stored key, a new or edited row carries its new key in the clear. The
/// flat legacy mirror is merged separately (it is the pool head's shadow) and
/// rebuilt from the validated providers afterwards.
pub async fn save(
    conn: &DatabaseConnection,
    incoming: TranslationSettings,
) -> Result<TranslationSettings, AppCommandError> {
    let stored = load(conn).await;

    let mut merged = incoming;
    for provider in &mut merged.providers {
        if provider.api_key == API_KEY_MASK {
            if let Some(existing) = stored
                .providers
                .iter()
                .find(|existing| existing.id == provider.id && !existing.id.is_empty())
            {
                provider.api_key = existing.api_key.clone();
            } else {
                // A mask with no stored original (an unsaved new row, or a
                // legacy row that never had a per-provider key) merges to
                // empty — same semantics as clearing it.
                provider.api_key = String::new();
            }
        }
    }
    if merged.providers.is_empty() {
        merged.api_key = merge_secret(merged.api_key.trim(), &stored.api_key);
    } else {
        // The flat mirror never carries an independent secret anymore: it is
        // rebuilt from the pool head in `validate`.
        merged.api_key = String::new();
    }
    let validated = validate(merged)?;

    let serialized = serde_json::to_string(&validated).map_err(|e| {
        AppCommandError::invalid_input("Failed to serialize translation settings")
            .with_detail(e.to_string())
    })?;
    app_metadata_service::upsert_value(conn, TRANSLATION_SETTINGS_KEY, &serialized)
        .await
        .map_err(AppCommandError::from)?;

    Ok(validated.masked())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    fn complete() -> TranslationSettings {
        TranslationSettings {
            enabled: true,
            providers: Vec::new(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-secret".to_string(),
            model: "gpt-4o-mini".to_string(),
            target_lang: Some("zh-CN".to_string()),
            translate_thinking: false,
            api_format: String::new(),
            selection_translate: true,
            selection_target_lang: None,
            toggle_always_visible: false,
            batch_max_chars: None,
            carry_context: true,
        }
    }

    /// A row saved by the pool-aware settings page: the endpoint lives in the
    /// list, not the flat fields.
    fn pooled() -> TranslationSettings {
        TranslationSettings {
            providers: vec![ProviderConfig {
                id: "p1".to_string(),
                name: Some("main".to_string()),
                base_url: "https://api.example.com".to_string(),
                api_key: "sk-secret".to_string(),
                model: "gpt-4o-mini".to_string(),
                api_format: String::new(),
                enabled: true,
                rpm_cap: Some(120),
            }],
            ..complete()
        }
    }

    /// B1 step 1-8 across the shapes users actually paste. One table so a
    /// regression in any single step shows as exactly one failed row.
    #[test]
    fn base_urls_normalize_to_a_canonical_form() {
        for (raw, expected) in [
            ("https://api.example.com", "https://api.example.com"),
            ("https://api.example.com/", "https://api.example.com"),
            ("  https://api.example.com  ", "https://api.example.com"),
            ("https://api.example.com/v1", "https://api.example.com/v1"),
            ("https://api.example.com/v1/", "https://api.example.com/v1"),
            (
                "https://api.example.com/v1/chat/completions",
                "https://api.example.com/v1",
            ),
            (
                "https://api.example.com/v1/chat/completions/",
                "https://api.example.com/v1",
            ),
            ("HTTPS://API.EXAMPLE.COM/v1", "https://api.example.com/v1"),
            (
                "https://api.example.com/v1/?key=abc#frag",
                "https://api.example.com/v1",
            ),
            ("HTTP://Api.Example.Com:8080/v1", "http://api.example.com:8080/v1"),
        ] {
            assert_eq!(
                normalize_base_url(raw).expect("normalizes"),
                expected,
                "input {raw:?}"
            );
        }
    }

    /// A public host defaults to TLS; a private or local one to plain http,
    /// because that is how Ollama/llama.cpp actually serve.
    #[test]
    fn a_missing_scheme_defaults_to_https_for_public_hosts() {
        assert_eq!(
            normalize_base_url("api.openai.com/v1").expect("normalizes"),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn a_private_host_defaults_to_http() {
        for host in [
            "localhost",
            "localhost:11434",
            "127.0.0.1:8080",
            "::1",
            "[::1]:11434",
            "nas.local",
            "10.0.0.5",
            "192.168.1.5:11434",
            "172.16.0.1",
            "172.31.255.255",
        ] {
            let normalized = normalize_base_url(host).expect("normalizes");
            assert!(
                normalized.starts_with("http://"),
                "{host} must default to http, got {normalized}"
            );
        }
        // The /12 upper bound: 172.32.x is public and must get https.
        assert_eq!(
            normalize_base_url("172.32.0.1").expect("normalizes"),
            "https://172.32.0.1"
        );
    }

    /// The guess is only a default; a user who spelled out https:// on a
    /// private host (a TLS-terminating LAN proxy) must not be overridden.
    #[test]
    fn an_explicit_scheme_wins_over_the_private_host_guess() {
        assert_eq!(
            normalize_base_url("https://localhost:8080").expect("normalizes"),
            "https://localhost:8080"
        );
    }

    #[test]
    fn unsupported_schemes_are_rejected_distinctly() {
        for raw in ["ftp://files.example.com", "file:///etc/passwd", "socks5://host"] {
            let err = normalize_base_url(raw).expect_err("must be rejected");
            assert_eq!(err.message, ERR_BASE_URL_SCHEME, "input {raw:?}");
            assert!(
                matches!(err.code, crate::app_error::AppErrorCode::ConfigurationInvalid),
                "a wrong scheme is a configuration problem, not bad input"
            );
        }
    }

    /// A schemeless URL naming no host is junk, but the *empty* paste is the
    /// draft path and must stay `Ok("")` — the settings page keeps half-filled
    /// forms while the user is still typing.
    #[test]
    fn a_schemeless_url_without_a_host_is_rejected() {
        for raw in ["://no-host", "http://"] {
            assert!(
                normalize_base_url(raw).is_err(),
                "{raw:?} must be rejected"
            );
        }
    }

    #[test]
    fn an_empty_base_url_normalizes_to_empty() {
        assert_eq!(normalize_base_url("").expect("empty ok"), "");
        assert_eq!(normalize_base_url("   ").expect("blank ok"), "");
    }

    /// `validate` must route through the normalizer, so the *stored* value is
    /// the canonical one — not just the one the derivation happens to survive.
    #[test]
    fn normalize_runs_inside_validate_and_persists() {
        let saved = validate(TranslationSettings {
            base_url: "api.example.com/v1/chat/completions".to_string(),
            ..complete()
        })
        .expect("scheme defaulted and route stripped");
        assert_eq!(saved.base_url, "https://api.example.com/v1");
    }

    #[tokio::test]
    async fn saving_after_normalization_keeps_the_mask_roundtrip() {
        let db = fresh_in_memory_db().await;
        save(&db.conn, complete()).await.expect("initial save");

        let returned = save(
            &db.conn,
            TranslationSettings {
                base_url: "api.example.com/v1".to_string(),
                api_key: API_KEY_MASK.to_string(),
                ..complete()
            },
        )
        .await
        .expect("second save");

        assert_eq!(returned.base_url, "https://api.example.com/v1");
        assert_eq!(returned.api_key, API_KEY_MASK);
        let stored = load(&db.conn).await;
        assert_eq!(stored.api_key, "sk-secret");
    }

    #[test]
    fn oversized_base_urls_are_rejected_after_trim() {
        let raw = format!("  https://{}  ", "a".repeat(MAX_BASE_URL_LEN));
        let err = normalize_base_url(&raw).expect_err("must be rejected");
        assert_eq!(err.message, ERR_BASE_URL_TOO_LONG);
    }

    /// models_url and chat_completions_url must share one base derivation, so
    /// a probe of the list route proves the chat route too.
    #[test]
    fn models_url_matches_the_chat_completions_shape() {
        let urls = |base: &str, format: &str| {
            let settings = TranslationSettings {
                base_url: base.to_string(),
                api_format: format.to_string(),
                ..complete()
            };
            (settings.chat_completions_url(), settings.models_url())
        };

        // OpenAI: base without /v1 gains it; both routes agree.
        let (chat, models) = urls("https://api.openai.com", "openai");
        assert_eq!(chat, "https://api.openai.com/v1/chat/completions");
        assert_eq!(models, "https://api.openai.com/v1/models");
        let (chat, models) = urls("https://api.openai.com/v1", "openai");
        assert_eq!(chat, "https://api.openai.com/v1/chat/completions");
        assert_eq!(models, "https://api.openai.com/v1/models");
    }

    /// The cached key must not change when only the *spelling* of the base
    /// changed, and must change when the dialect did.
    ///
    /// `provider_id` partitions on the resolved chat URL, so the two spellings
    /// share one id without anyone normalizing first, and the dialect pin
    /// (same host, different route) still splits.
    #[test]
    fn host_and_v1_forms_share_one_provider_id() {
        let plain = TranslationSettings {
            base_url: "https://api.example.com".to_string(),
            ..complete()
        };
        let with_v1 = TranslationSettings {
            base_url: "https://api.example.com/v1".to_string(),
            ..complete()
        };
        assert_eq!(plain.provider_id(), with_v1.provider_id());

        let anthropic = TranslationSettings {
            base_url: "https://api.anthropic.com".to_string(),
            api_format: "anthropic".to_string(),
            ..complete()
        };
        let auto_detected = TranslationSettings {
            base_url: "https://api.anthropic.com".to_string(),
            ..complete()
        };
        assert_eq!(anthropic.provider_id(), auto_detected.provider_id());
    }

    #[test]
    fn formats_are_detected_from_the_host() {
        for (base, expected) in [
            ("https://api.anthropic.com", ApiFormat::Anthropic),
            (
                "https://generativelanguage.googleapis.com/v1beta/openai",
                ApiFormat::Gemini,
            ),
            ("http://localhost:11434", ApiFormat::Ollama),
            ("http://192.168.1.5:11434", ApiFormat::Ollama),
            ("https://api.openai.com", ApiFormat::Openai),
            ("https://my-proxy.example.com/v1", ApiFormat::Openai),
        ] {
            assert_eq!(
                resolve_format(base, "auto"),
                expected,
                "base {base} must detect {expected:?}"
            );
        }
    }

    /// A reverse proxy that hides the provider behind a private domain is
    /// exactly what the explicit dropdown exists for.
    #[test]
    fn an_explicit_format_wins_over_detection() {
        for (format, expected) in [
            ("openai", ApiFormat::Openai),
            ("anthropic", ApiFormat::Anthropic),
            ("gemini", ApiFormat::Gemini),
            ("ollama", ApiFormat::Ollama),
        ] {
            assert_eq!(
                resolve_format("https://my-proxy.example.com", format),
                expected,
                "explicit {format} must pin the dialect"
            );
        }
    }

    #[test]
    fn an_unknown_format_is_rejected() {
        assert!(validate(TranslationSettings {
            api_format: "claude-code".to_string(),
            ..complete()
        })
        .is_err());
    }

    #[test]
    fn provider_id_changes_with_the_format() {
        // The equivalence class lives on the member now: the pool's cache
        // partition is one constant, but pool-state keying (the adaptive
        // limiter's memory) must still split on a dialect change — same host,
        // different route = different endpoint.
        let base = ProviderConfig {
            base_url: "https://my-proxy.example.com".to_string(),
            api_key: "sk".to_string(),
            model: "m".to_string(),
            ..Default::default()
        };
        let mut other = base.clone();
        other.api_format = "anthropic".to_string();
        assert_ne!(
            base.provider_id(),
            other.provider_id(),
            "same base, different dialect = different endpoint = different runtime state"
        );
    }

    #[test]
    fn ollama_may_be_enabled_without_a_key() {
        let settings = TranslationSettings {
            base_url: "http://localhost:11434/v1".to_string(),
            api_key: String::new(),
            api_format: String::new(),
            ..complete()
        };
        assert!(validate(settings).is_ok());
    }

    /// Every format's documented pair of routes, derived from the same base
    /// spellings a user would paste.
    #[test]
    fn the_four_formats_derive_their_documented_endpoints() {
        let urls = |base: &str, format: &str| {
            let settings = TranslationSettings {
                base_url: normalize_base_url(base).expect("valid base"),
                api_format: format.to_string(),
                ..complete()
            };
            (settings.chat_completions_url(), settings.models_url())
        };

        // OpenAI-compatible.
        let (chat, models) = urls("https://api.openai.com", "openai");
        assert_eq!(chat, "https://api.openai.com/v1/chat/completions");
        assert_eq!(models, "https://api.openai.com/v1/models");

        // Anthropic native.
        let (chat, models) = urls("https://api.anthropic.com", "anthropic");
        assert_eq!(chat, "https://api.anthropic.com/v1/messages");
        assert_eq!(models, "https://api.anthropic.com/v1/models");

        // Gemini OpenAI-compat surface.
        let (chat, models) =
            urls("https://generativelanguage.googleapis.com", "gemini");
        assert_eq!(
            chat,
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
        assert_eq!(
            models,
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );

        // Ollama's official OpenAI-compatible mount.
        let (chat, models) = urls("http://localhost:11434", "ollama");
        assert_eq!(chat, "http://localhost:11434/v1/chat/completions");
        assert_eq!(models, "http://localhost:11434/v1/models");
    }

    /// The canonical pastes that already end in `/v1` must not double it —
    /// auto-detection makes these the *common* shapes for Ollama and Anthropic.
    #[test]
    fn a_v1_suffixed_base_does_not_double_the_route() {
        let urls = |base: &str, format: &str| {
            let settings = TranslationSettings {
                base_url: normalize_base_url(base).expect("valid base"),
                api_format: format.to_string(),
                ..complete()
            };
            (settings.chat_completions_url(), settings.models_url())
        };

        let (chat, models) = urls("http://localhost:11434/v1", "auto");
        assert_eq!(chat, "http://localhost:11434/v1/chat/completions");
        assert_eq!(models, "http://localhost:11434/v1/models");

        let (chat, models) = urls("https://api.anthropic.com/v1", "auto");
        assert_eq!(chat, "https://api.anthropic.com/v1/messages");
        assert_eq!(models, "https://api.anthropic.com/v1/models");
    }

    #[test]
    fn known_api_suffixes_are_stripped_on_save() {
        for (raw, expected_base) in [
            ("https://host/v1/chat/completions", "https://host/v1"),
            ("https://host/v1/messages", "https://host"),
            (
                "https://host/v1beta/openai",
                "https://host",
            ),
            ("http://localhost:11434/api/chat", "http://localhost:11434"),
            (
                "http://localhost:11434/api/generate",
                "http://localhost:11434",
            ),
            ("https://host/CHAT/COMPLETIONS", "https://host"),
        ] {
            assert_eq!(
                normalize_base_url(raw).expect("normalizes"),
                expected_base,
                "input {raw:?}"
            );
        }
    }

    #[test]
    fn a_disabled_draft_may_be_incomplete() {
        let draft = TranslationSettings {
            enabled: false,
            base_url: "https://api.example.com".to_string(),
            ..Default::default()
        };
        assert!(validate(draft).is_ok());
    }

    #[test]
    fn enabling_requires_url_key_and_model() {
        for missing in ["base_url", "api_key", "model"] {
            let mut settings = complete();
            match missing {
                "base_url" => settings.base_url = String::new(),
                "api_key" => settings.api_key = String::new(),
                _ => settings.model = String::new(),
            }
            assert!(
                validate(settings).is_err(),
                "enabled settings without {missing} must be rejected"
            );
        }
    }

    #[test]
    fn a_base_url_without_a_scheme_is_normalized_not_rejected() {
        let settings = TranslationSettings {
            base_url: "api.example.com".to_string(),
            ..complete()
        };
        let validated = validate(settings).expect("scheme defaulted");
        assert_eq!(validated.base_url, "https://api.example.com");
    }

    #[test]
    fn oversized_fields_are_rejected() {
        let cases = [
            TranslationSettings {
                base_url: format!("https://{}", "a".repeat(MAX_BASE_URL_LEN)),
                ..complete()
            },
            TranslationSettings {
                api_key: "k".repeat(MAX_API_KEY_LEN + 1),
                ..complete()
            },
            TranslationSettings {
                model: "m".repeat(MAX_MODEL_LEN + 1),
                ..complete()
            },
            TranslationSettings {
                target_lang: Some("l".repeat(MAX_TARGET_LANG_LEN + 1)),
                ..complete()
            },
        ];
        for settings in cases {
            assert!(validate(settings).is_err());
        }
    }

    /// The boundary itself must pass — an off-by-one here would reject a key
    /// that is exactly as long as the documented cap.
    #[test]
    fn fields_at_exactly_the_cap_are_accepted() {
        let settings = TranslationSettings {
            api_key: "k".repeat(MAX_API_KEY_LEN),
            model: "m".repeat(MAX_MODEL_LEN),
            ..complete()
        };
        assert!(validate(settings).is_ok());
    }

    #[test]
    fn the_endpoint_is_derived_without_doubling_the_route() {
        let url_for = |base: &str| {
            TranslationSettings {
                base_url: base.to_string(),
                ..complete()
            }
            .chat_completions_url()
        };
        let expected = "https://api.example.com/v1/chat/completions";
        assert_eq!(url_for("https://api.example.com"), expected);
        assert_eq!(url_for("https://api.example.com/"), expected);
        assert_eq!(url_for("https://api.example.com/v1"), expected);
        assert_eq!(url_for("https://api.example.com/v1/"), expected);
        assert_eq!(url_for(expected), expected);
    }

    #[test]
    fn masking_hides_the_key_but_keeps_the_rest() {
        let masked = complete().masked();
        assert_eq!(masked.api_key, API_KEY_MASK);
        assert_eq!(masked.model, complete().model);
    }

    /// The batch ceiling and a provider's RPM cap clamp instead of refusing:
    /// a slip of the keyboard on a form must not strand the whole endpoint
    /// config behind one field.
    #[test]
    fn numeric_knobs_clamp_to_their_documented_ranges() {
        let clamped = validate(TranslationSettings {
            batch_max_chars: Some(1),
            providers: vec![ProviderConfig {
                rpm_cap: Some(1),
                ..pooled().providers.remove(0)
            }],
            ..pooled()
        })
        .expect("clamps, not errors");
        assert_eq!(clamped.batch_max_chars, Some(500));
        assert_eq!(clamped.providers[0].rpm_cap, Some(RPM_CAP_MIN));

        let clamped = validate(TranslationSettings {
            batch_max_chars: Some(99_999),
            ..complete()
        })
        .expect("clamps, not errors");
        assert_eq!(clamped.batch_max_chars, Some(20_000));
    }

    /// A legacy row (flat fields, no list) migrates to a one-member pool on
    /// read, so every downstream path sees one shape. The synthesized id is
    /// deterministic: the settings page matches its masked key refill against
    /// the stored entry BY ID, and every load re-runs the migration until the
    /// user saves.
    #[test]
    fn a_legacy_row_migrates_to_a_single_member_pool() {
        let legacy = complete();
        let migrated = migrate_legacy(legacy.clone());
        assert_eq!(migrated.providers.len(), 1);
        assert_eq!(migrated.providers[0].base_url, legacy.base_url);
        assert_eq!(migrated.providers[0].api_key, legacy.api_key);
        assert_eq!(migrated.providers[0].model, legacy.model);
        assert!(migrated.providers[0].enabled);
        assert_eq!(migrated.providers[0].id, "legacy");
        assert_eq!(
            migrate_legacy(legacy).providers[0].id,
            "legacy",
            "the id must be stable across loads, not regenerated"
        );
        // A blank flat row (fresh install) stays listless.
        assert!(migrate_legacy(TranslationSettings::default())
            .providers
            .is_empty());
    }

    /// Saving a pooled row mirrors the head back into the flat fields, so an
    /// old build reading the same row still finds its endpoint.
    #[tokio::test]
    async fn saving_a_pool_mirrors_the_head_into_the_legacy_fields() {
        let db = fresh_in_memory_db().await;
        save(&db.conn, pooled()).await.expect("save pooled");

        let stored = load(&db.conn).await;
        assert_eq!(stored.base_url, "https://api.example.com");
        assert_eq!(stored.model, "gpt-4o-mini");
        assert_eq!(stored.api_key, "sk-secret");
        assert_eq!(stored.providers.len(), 1);
        assert!(
            !stored.providers[0].id.is_empty(),
            "validate assigns the stable id the pool state keys on"
        );
    }

    /// The pool cache key is one constant: any member's answer serves any
    /// later render, which is the point of rotating.
    #[test]
    fn the_pool_shares_one_cache_partition() {
        assert_eq!(complete().provider_id(), pooled().provider_id());
        assert_eq!(pooled().provider_id(), "pool");
    }

    /// A per-provider mask round-trips through save by id, not by position.
    #[tokio::test]
    async fn a_provider_mask_preserves_its_stored_key() {
        let db = fresh_in_memory_db().await;
        save(&db.conn, pooled()).await.expect("initial save");

        let stored = load(&db.conn).await;
        let id = stored.providers[0].id.clone();
        let mut edited = stored;
        edited.providers[0].api_key = API_KEY_MASK.to_string();
        edited.providers[0].model = "gpt-4o".to_string();
        save(&db.conn, edited).await.expect("second save");

        let stored = load(&db.conn).await;
        assert_eq!(stored.providers[0].api_key, "sk-secret");
        assert_eq!(stored.providers[0].model, "gpt-4o");
        assert_eq!(stored.providers[0].id, id, "the id survives the save");
    }

    /// An enabled feature with no callable provider is a configuration hole;
    /// a disabled feature may keep half-filled drafts.
    #[test]
    fn enabling_requires_at_least_one_callable_provider() {
        let mut empty_pool = pooled();
        empty_pool.providers[0].enabled = false;
        assert!(validate(empty_pool).is_err());

        let mut incomplete = pooled();
        incomplete.providers[0].model = String::new();
        assert!(validate(incomplete).is_err(), "no member is complete");
    }

    #[test]
    fn an_unset_key_masks_to_empty_not_to_dots() {
        let masked = TranslationSettings {
            api_key: String::new(),
            enabled: false,
            ..complete()
        }
        .masked();
        assert!(
            masked.api_key.is_empty(),
            "an absent key must not look like a stored one"
        );
    }

    /// The settings page never holds the real key, so saving an unchanged form
    /// sends the mask back. Treating that as the new key would destroy the
    /// stored credential on every unrelated edit.
    #[tokio::test]
    async fn saving_the_mask_back_preserves_the_stored_key() {
        let db = fresh_in_memory_db().await;
        save(&db.conn, complete()).await.expect("initial save");

        save(
            &db.conn,
            TranslationSettings {
                api_key: API_KEY_MASK.to_string(),
                model: "gpt-4o".to_string(),
                ..complete()
            },
        )
        .await
        .expect("second save");

        let stored = load(&db.conn).await;
        assert_eq!(stored.api_key, "sk-secret");
        assert_eq!(stored.model, "gpt-4o");
    }

    /// Clearing the field is a real intent and must not be confused with the
    /// mask round-trip above.
    #[tokio::test]
    async fn clearing_the_key_forgets_it() {
        let db = fresh_in_memory_db().await;
        save(&db.conn, complete()).await.expect("initial save");

        save(
            &db.conn,
            TranslationSettings {
                enabled: false,
                api_key: String::new(),
                ..complete()
            },
        )
        .await
        .expect("clear the key");

        assert!(load(&db.conn).await.api_key.is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_row_reads_as_unconfigured() {
        let db = fresh_in_memory_db().await;
        app_metadata_service::upsert_value(&db.conn, TRANSLATION_SETTINGS_KEY, "{not json")
            .await
            .expect("seed a corrupt row");

        assert_eq!(load(&db.conn).await, TranslationSettings::default());
    }

    #[tokio::test]
    async fn save_returns_the_masked_form() {
        let db = fresh_in_memory_db().await;
        let returned = save(&db.conn, complete()).await.expect("save");
        assert_eq!(returned.api_key, API_KEY_MASK);
    }
}

