//! Workspace-background marketplace backed by [wallhaven.cc](https://wallhaven.cc/).
//!
//! Three operations, all proxied host-side so the webview never talks to the
//! CDN directly (it is unreachable from some networks — same reason
//! `crate::pets::marketplace` proxies):
//! - `search(...)` — public `GET /api/v1/search` with `purity=100` (SFW)
//!   hard-coded; the app structurally cannot request NSFW results.
//! - `fetch_asset(...)` — one allowlisted thumbnail, returned as a
//!   `BackgroundAsset` for the frontend to mint a blob URL from.
//! - `download(...)` — full image through the *same* validation and atomic
//!   write as a manual background pick, so a market download and a local
//!   file share one security path (byte sniff, 16 MiB / 40 Mpx caps).
//!
//! All traffic uses a process-wide `reqwest::Client` with a stable
//! user-agent, mirroring `crate::pets::marketplace`.

use std::sync::LazyLock;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::backgrounds::{validate_background, write_background_atomic};
use crate::models::background::BackgroundAsset;

const WALLHAVEN_SEARCH_URL: &str = "https://wallhaven.cc/api/v1/search";
const WALLHAVEN_USER_AGENT: &str = "codeg-wallpaper-market/1.0";
/// SFW-only, permanently. Appended verbatim — never taken from params.
const WALLHAVEN_PURITY: &str = "100";
/// Search JSON cap. Real pages are ~50 KiB; 4 MiB matches the pet listing cap.
const MAX_SEARCH_JSON_BYTES: u64 = 4 * 1024 * 1024;
/// Thumbnail cap. Real `th.wallhaven.cc/small` files are tens of KiB.
const MAX_ASSET_BYTES: u64 = 4 * 1024 * 1024;
/// Full-image cap. Deliberately equals `backgrounds::MAX_BG_BYTES` so the
/// transport cap and the byte-level validator agree on one ceiling.
const MAX_DOWNLOAD_BYTES: u64 = 16 * 1024 * 1024;
/// Longest accepted search query; wallhaven itself truncates far earlier.
const MAX_QUERY_CHARS: usize = 128;

static MARKET_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .user_agent(WALLHAVEN_USER_AGENT)
        .build()
        .map_err(|e| format!("failed to initialize wallpaper market HTTP client: {e}"))
});

fn client() -> Result<&'static reqwest::Client, AppCommandError> {
    MARKET_HTTP_CLIENT
        .as_ref()
        .map_err(|err| AppCommandError::network(err.clone()))
}

// ─── URL allowlist ───────────────────────────────────────────────────────

pub(crate) fn is_allowed_wallhaven_host(host: &str) -> bool {
    host == "wallhaven.cc" || host.ends_with(".wallhaven.cc")
}

/// Accept only `https` URLs on wallhaven.cc or a subdomain, with no embedded
/// userinfo. Everything the market fetches funnels through this check.
pub(crate) fn parse_wallhaven_https_url(raw: &str) -> Result<reqwest::Url, AppCommandError> {
    let url = reqwest::Url::parse(raw).map_err(|_| {
        AppCommandError::invalid_input("Marketplace URL must be a valid https wallhaven.cc URL.")
    })?;
    if url.scheme() != "https" {
        return Err(AppCommandError::invalid_input(
            "Marketplace URL must use https.",
        ));
    }
    // A URL carrying userinfo is not a shape wallhaven ever produces; refuse
    // it rather than wonder what it was impersonating.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppCommandError::invalid_input(
            "Marketplace URL must not embed credentials.",
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        AppCommandError::invalid_input("Marketplace URL must name a host.")
    })?;
    if !is_allowed_wallhaven_host(host) {
        return Err(AppCommandError::invalid_input(
            "Marketplace URL host must be wallhaven.cc or a subdomain.",
        ));
    }
    Ok(url)
}

/// Canonical page URL for an id — derived, never trusted from the listing,
/// because `download` requires exactly this shape for `source_url`.
pub(crate) fn wallhaven_source_url(id: &str) -> String {
    format!("https://wallhaven.cc/w/{}", id.trim())
}

// ─── Wire types ──────────────────────────────────────────────────────────

/// Query parameters for `search`. `category` accepts exactly
/// all/general/anime/people; anything else is an error (a typo silently
/// becoming "all" would look like broken filtering).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSearchParams {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
}

/// One listing entry re-serialized as a stable contract (a subset of the
/// upstream record, like `pets::marketplace::MarketplacePetSummary`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketWallpaperSummary {
    pub id: String,
    pub thumb_url: String,
    pub full_url: String,
    pub source_url: String,
    pub resolution: String,
    pub category: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSearchPage {
    pub items: Vec<MarketWallpaperSummary>,
    pub page: u32,
    pub last_page: u32,
}

/// wallhaven category bitmask: general=100 / anime=010 / people=001.
pub(crate) fn wallhaven_categories(category: Option<&str>) -> Result<&'static str, AppCommandError> {
    match category {
        None | Some("all") => Ok("111"),
        Some("general") => Ok("100"),
        Some("anime") => Ok("010"),
        Some("people") => Ok("001"),
        Some(other) => Err(AppCommandError::invalid_input(format!(
            "Unknown wallpaper market category: {other}"
        ))),
    }
}

// ─── Search ──────────────────────────────────────────────────────────────

pub async fn search(params: MarketSearchParams) -> Result<MarketSearchPage, AppCommandError> {
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    if let Some(q) = query {
        if q.chars().count() > MAX_QUERY_CHARS {
            return Err(AppCommandError::invalid_input(format!(
                "Search query exceeds {MAX_QUERY_CHARS} characters."
            )));
        }
    }
    let page = params.page.unwrap_or(1).max(1);
    let categories = wallhaven_categories(params.category.as_deref())?;

    let mut url = reqwest::Url::parse(WALLHAVEN_SEARCH_URL)
        .map_err(|e| AppCommandError::network(format!("invalid search URL: {e}")))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("categories", categories);
        pairs.append_pair("purity", WALLHAVEN_PURITY);
        pairs.append_pair("page", &page.to_string());
        match query {
            Some(q) => {
                pairs.append_pair("q", q);
                pairs.append_pair("sorting", "relevance");
            }
            // Browse mode: the last month's top list is a sensible default grid.
            None => {
                pairs.append_pair("sorting", "toplist");
                pairs.append_pair("topRange", "1M");
            }
        }
    }

    let resp = client()?
        .get(url)
        .send()
        .await
        .map_err(|e| AppCommandError::network(format!("wallhaven search failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppCommandError::network(format!(
            "wallhaven search returned HTTP {}",
            resp.status()
        )));
    }
    let body = read_capped(resp, MAX_SEARCH_JSON_BYTES, "wallhaven search payload").await?;
    let text = String::from_utf8_lossy(&body).into_owned();
    parse_search_payload(&text)
}

/// Pure parser so the listing contract is unit-testable without network.
pub(crate) fn parse_search_payload(body: &str) -> Result<MarketSearchPage, AppCommandError> {
    #[derive(Deserialize)]
    struct ApiThumbs {
        #[serde(default)]
        small: Option<String>,
    }
    #[derive(Deserialize)]
    struct ApiItem {
        id: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        thumbs: Option<ApiThumbs>,
        #[serde(default)]
        dimension_x: Option<u32>,
        #[serde(default)]
        dimension_y: Option<u32>,
        #[serde(default)]
        category: Option<String>,
    }
    #[derive(Default, Deserialize)]
    struct ApiMeta {
        #[serde(default)]
        current_page: Option<u32>,
        #[serde(default)]
        last_page: Option<u32>,
    }
    #[derive(Deserialize)]
    struct ApiPayload {
        #[serde(default)]
        data: Vec<ApiItem>,
        #[serde(default)]
        meta: Option<ApiMeta>,
    }

    let payload: ApiPayload = serde_json::from_str(body)
        .map_err(|e| AppCommandError::network(format!("wallhaven returned malformed JSON: {e}")))?;

    let mut items = Vec::with_capacity(payload.data.len());
    for item in payload.data {
        let (Some(full_url), Some(thumb_url)) = (item.path, item.thumbs.and_then(|t| t.small))
        else {
            continue;
        };
        // A listing entry pointing off wallhaven is dropped, not trusted —
        // the frontend will only ever hand us URLs we vouched for here.
        if parse_wallhaven_https_url(&full_url).is_err()
            || parse_wallhaven_https_url(&thumb_url).is_err()
        {
            continue;
        }
        let resolution = match (item.dimension_x, item.dimension_y) {
            (Some(w), Some(h)) => format!("{w}×{h}"),
            _ => String::new(),
        };
        // Derived from the id, not copied from the listing. Computed before
        // `item.id` is moved into the summary below.
        let source_url = wallhaven_source_url(&item.id);
        items.push(MarketWallpaperSummary {
            id: item.id,
            thumb_url,
            full_url,
            source_url,
            resolution,
            category: item.category.unwrap_or_default(),
        });
    }
    let meta = payload.meta.unwrap_or_default();
    Ok(MarketSearchPage {
        items,
        page: meta.current_page.unwrap_or(1),
        last_page: meta.last_page.unwrap_or(1).max(1),
    })
}

// ─── Asset proxy (thumbnails) ────────────────────────────────────────────

pub async fn fetch_asset(url: &str) -> Result<BackgroundAsset, AppCommandError> {
    let url = parse_wallhaven_https_url(url)?;
    let (mime, bytes) = fetch_image_capped(&url, MAX_ASSET_BYTES, "wallhaven thumbnail").await?;
    Ok(BackgroundAsset {
        mime,
        data_base64: BASE64.encode(&bytes),
    })
}

// ─── Download & apply ────────────────────────────────────────────────────

pub async fn download(url: &str, source_url: &str) -> Result<(), AppCommandError> {
    let full_url = parse_wallhaven_https_url(url)?;
    // `source_url` is metadata we display/compare; require it to be the real
    // page URL shape so a download can't be attributed to a bogus source.
    let source = parse_wallhaven_https_url(source_url)?;
    if source.host_str() != Some("wallhaven.cc") || !source.path().starts_with("/w/") {
        return Err(AppCommandError::invalid_input(
            "sourceUrl must be a https://wallhaven.cc/w/<id> page URL.",
        ));
    }

    let (_mime, bytes) =
        fetch_image_capped(&full_url, MAX_DOWNLOAD_BYTES, "wallpaper download").await?;
    // Same gate as a manual pick: byte sniff, 16 MiB and 40 Mpx caps.
    validate_background(&bytes)?;
    write_background_atomic(&bytes)
}

// ─── Shared fetch helper ─────────────────────────────────────────────────

async fn fetch_image_capped(
    url: &reqwest::Url,
    cap: u64,
    what: &str,
) -> Result<(String, Vec<u8>), AppCommandError> {
    let resp = client()?
        .get(url.clone())
        .send()
        .await
        .map_err(|e| AppCommandError::network(format!("{what} failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppCommandError::network(format!(
            "{what} returned HTTP {}",
            resp.status()
        )));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        });
    // wallhaven serves jpeg/png/webp for both thumbs and full images. The
    // byte-level sniff in `validate_background` remains the final authority
    // for downloads; this is the early, cheap rejection.
    if !matches!(
        content_type.as_deref(),
        Some("image/jpeg") | Some("image/png") | Some("image/webp")
    ) {
        return Err(AppCommandError::network(format!(
            "{what} returned unsupported content-type {content_type:?}"
        )));
    }
    let bytes = read_capped(resp, cap, what).await?;
    Ok((content_type.expect("checked above"), bytes))
}

/// Read a response body with a hard ceiling: reject on declared
/// Content-Length and again on accumulated bytes, so a lying header or a
/// chunked stream cannot balloon memory.
async fn read_capped(
    mut resp: reqwest::Response,
    cap: u64,
    what: &str,
) -> Result<Vec<u8>, AppCommandError> {
    let cap_mib = cap / (1024 * 1024);
    if let Some(len) = resp.content_length() {
        if len > cap {
            return Err(AppCommandError::network(format!(
                "{what} exceeds {cap_mib} MiB cap."
            )));
        }
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AppCommandError::network(format!("{what} failed mid-transfer: {e}")))?
    {
        if buf.len() as u64 + chunk.len() as u64 > cap {
            return Err(AppCommandError::network(format!(
                "{what} exceeds {cap_mib} MiB cap."
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": [
        {
          "id": "abc123",
          "url": "https://wallhaven.cc/w/abc123",
          "path": "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg",
          "thumbs": { "small": "https://th.wallhaven.cc/small/ab/abc123.jpg" },
          "dimension_x": 1920,
          "dimension_y": 1080,
          "category": "general"
        },
        {
          "id": "bad9",
          "url": "https://wallhaven.cc/w/bad9",
          "path": "https://evil.example/full/wallhaven-bad9.jpg",
          "thumbs": { "small": "https://th.wallhaven.cc/small/ba/bad9.jpg" },
          "dimension_x": 800,
          "dimension_y": 600,
          "category": "anime"
        }
      ],
      "meta": { "current_page": 2, "last_page": 10, "per_page": 24, "total": 240 }
    }"#;

    #[test]
    fn categories_maps_known_filters() {
        assert_eq!(wallhaven_categories(Some("all")).unwrap(), "111");
        assert_eq!(wallhaven_categories(Some("general")).unwrap(), "100");
        assert_eq!(wallhaven_categories(Some("anime")).unwrap(), "010");
        assert_eq!(wallhaven_categories(Some("people")).unwrap(), "001");
        assert_eq!(wallhaven_categories(None).unwrap(), "111");
    }

    #[test]
    fn categories_rejects_unknown_value() {
        assert!(wallhaven_categories(Some("nsfw")).is_err());
    }

    #[test]
    fn host_allowlist_accepts_wallhaven_and_subdomains_only() {
        assert!(is_allowed_wallhaven_host("wallhaven.cc"));
        assert!(is_allowed_wallhaven_host("th.wallhaven.cc"));
        assert!(is_allowed_wallhaven_host("w.wallhaven.cc"));
        assert!(!is_allowed_wallhaven_host("wallhaven.cc.evil"));
        assert!(!is_allowed_wallhaven_host("evil.cc"));
    }

    #[test]
    fn url_parser_enforces_https_wallhaven_no_userinfo() {
        assert!(parse_wallhaven_https_url("https://w.wallhaven.cc/full/ab/x.jpg").is_ok());
        assert!(parse_wallhaven_https_url("https://wallhaven.cc/w/abc").is_ok());
        assert!(parse_wallhaven_https_url("http://wallhaven.cc/w/abc").is_err());
        assert!(parse_wallhaven_https_url("https://example.com/a.jpg").is_err());
        assert!(parse_wallhaven_https_url("file:///etc/passwd").is_err());
        assert!(parse_wallhaven_https_url("https://user:pw@wallhaven.cc/w/abc").is_err());
        assert!(parse_wallhaven_https_url("not a url").is_err());
    }

    #[test]
    fn source_url_is_derived_from_id() {
        assert_eq!(wallhaven_source_url(" abc123 "), "https://wallhaven.cc/w/abc123");
    }

    #[test]
    fn search_payload_parses_and_drops_non_wallhaven_entries() {
        let page = parse_search_payload(FIXTURE).expect("parse");
        // 第二项的 path 指向 evil.example —— 整条丢弃，不信任。
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.id, "abc123");
        assert_eq!(item.thumb_url, "https://th.wallhaven.cc/small/ab/abc123.jpg");
        assert_eq!(item.full_url, "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg");
        assert_eq!(item.source_url, "https://wallhaven.cc/w/abc123");
        assert_eq!(item.resolution, "1920×1080");
        assert_eq!(item.category, "general");
        assert_eq!(page.page, 2);
        assert_eq!(page.last_page, 10);
    }

    #[test]
    fn search_payload_tolerates_missing_meta() {
        let page = parse_search_payload(r#"{"data":[]}"#).expect("parse");
        assert!(page.items.is_empty());
        assert_eq!(page.page, 1);
        assert_eq!(page.last_page, 1);
    }

    #[test]
    fn search_payload_rejects_garbage() {
        assert!(parse_search_payload("not json").is_err());
    }
}
