//! Appearance preset gallery: a static JSON index in a public repository plus
//! the preset files it lists, fetched host-side the way the wallpaper market
//! (`crate::backgrounds::marketplace`) and the pet marketplace proxy their
//! listings. Two operations:
//!
//! - `fetch_index(url)` — the index document, capped at [`MAX_INDEX_BYTES`].
//! - `fetch_preset(url, sha256)` — one preset file, capped at
//!   [`MAX_PRESET_BYTES`] and refused unless the SHA-256 of the bytes equals the
//!   digest the index listed for it, so a file that changed after it was
//!   indexed, or a host serving something else under that path, never reaches
//!   the frontend.
//!
//! Unlike the two marketplaces there is no fixed host: the index URL is a user
//! setting (the default points at a public GitHub repository), so the policy is
//! shape-based rather than an allowlist. https only, no embedded credentials, no
//! IP-literal or loopback hosts (in server mode this proxy runs on a machine
//! whose private network the browser must not be able to read through it), and
//! every redirect hop is held to the same rule. The bytes come back as text for
//! the frontend's strict validator (`src/lib/appearance-preset.ts`); nothing
//! here interprets them, and no error echoes a response body.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;

/// Index cap. A few hundred listings with swatches fit in a fraction of this.
pub const MAX_INDEX_BYTES: u64 = 256 * 1024;
/// Preset file cap. Deliberately equals `MAX_PRESET_BYTES` in
/// `src/lib/appearance-preset.ts`, so the transport cap and the validator's own
/// size check agree on one ceiling.
pub const MAX_PRESET_BYTES: u64 = 32 * 1024;
const GALLERY_USER_AGENT: &str = "codeg-preset-gallery/1.0";
/// Deadline for one fetch; both documents are at most a few hundred KiB.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Redirect hops allowed, each re-checked against the URL policy.
const MAX_REDIRECT_HOPS: usize = 5;

static GALLERY_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        // Per-read: a stalled transfer fails fast, a slow one may finish
        // inside `FETCH_TIMEOUT`.
        .read_timeout(Duration::from_secs(15))
        .redirect(gallery_redirect_policy())
        .user_agent(GALLERY_USER_AGENT)
        .build()
        .map_err(|e| format!("failed to initialize preset gallery HTTP client: {e}"))
});

fn client() -> Result<&'static reqwest::Client, AppCommandError> {
    GALLERY_HTTP_CLIENT
        .as_ref()
        .map_err(|err| AppCommandError::network(err.clone()))
}

// ─── URL policy ──────────────────────────────────────────────────────────

/// Whether a host is one this proxy must never be steered onto: an IP literal
/// (v4, or a bracketed v6) or a loopback name. The URL parser has already
/// normalized shorthand forms such as `0x7f.1`, so the address parse sees them.
pub(crate) fn is_ip_or_local_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<std::net::IpAddr>().is_ok()
        || host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
}

/// Whether a URL, first hop or redirect target, is inside the policy.
///
/// Validating only the URL we dial would cover the first hop; under the default
/// redirect policy the client would then follow a `Location` anywhere, and since
/// the response body is handed back to the caller, a redirect onto a private
/// address would turn this proxy into a reader of whatever the host can reach.
pub(crate) fn is_allowed_gallery_hop(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| !is_ip_or_local_host(host))
}

fn gallery_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if !is_allowed_gallery_hop(attempt.url()) {
            let refused = format!("refused a redirect to {}", attempt.url());
            return attempt.error(refused);
        }
        if attempt.previous().len() > MAX_REDIRECT_HOPS {
            return attempt.error(format!("more than {MAX_REDIRECT_HOPS} redirects"));
        }
        attempt.follow()
    })
}

/// Parse a gallery URL under the policy, with a reason for each refusal.
pub(crate) fn parse_gallery_https_url(raw: &str) -> Result<reqwest::Url, AppCommandError> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| AppCommandError::invalid_input("Gallery URL must be a valid https URL."))?;
    if url.scheme() != "https" {
        return Err(AppCommandError::invalid_input("Gallery URL must use https."));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppCommandError::invalid_input(
            "Gallery URL must not embed credentials.",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppCommandError::invalid_input("Gallery URL must name a host."))?;
    if is_ip_or_local_host(host) {
        return Err(AppCommandError::invalid_input(
            "Gallery URL must name a public host, not an IP address or localhost.",
        ));
    }
    Ok(url)
}

// ─── Wire type ───────────────────────────────────────────────────────────

/// A fetched document: the text for the frontend's validator, the SHA-256 of
/// the bytes it was decoded from, and their count. camelCase on the wire,
/// mirrored by `GalleryDocument` in `src/lib/appearance-preset-gallery.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryDocument {
    pub text: String,
    pub sha256: String,
    pub bytes: u64,
}

// ─── Digest ──────────────────────────────────────────────────────────────

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A listed digest normalized to lowercase hex, or a refusal: 64 hex digits and
/// nothing else, checked before any network is touched.
pub(crate) fn normalize_sha256(listed: &str) -> Result<String, AppCommandError> {
    let lower = listed.trim().to_ascii_lowercase();
    if lower.len() != 64 || !lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppCommandError::invalid_input(
            "Listed digest must be 64 hex characters of SHA-256.",
        ));
    }
    Ok(lower)
}

/// The digest of `bytes` when it equals the listed one; otherwise a refusal that
/// names both digests and nothing of the content.
pub(crate) fn verify_sha256(bytes: &[u8], listed: &str) -> Result<String, AppCommandError> {
    let expected = normalize_sha256(listed)?;
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(AppCommandError::configuration_invalid(format!(
            "Preset file does not match its listed digest (listed {expected}, got {actual})."
        ))
        .with_i18n("presetGallery.errors.hashMismatch", BTreeMap::new()));
    }
    Ok(actual)
}

// ─── Caps ────────────────────────────────────────────────────────────────

fn too_large(cap: u64, what: &str) -> AppCommandError {
    let cap_kib = cap / 1024;
    AppCommandError::network(format!("{what} exceeds the {cap_kib} KiB cap."))
        .with_i18n(
            "presetGallery.errors.tooLarge",
            BTreeMap::from([("limit".to_string(), cap_kib.to_string())]),
        )
}

/// Refuse a declared Content-Length past the cap before reading anything.
pub(crate) fn check_declared_length(
    declared: Option<u64>,
    cap: u64,
    what: &str,
) -> Result<(), AppCommandError> {
    match declared {
        Some(len) if len > cap => Err(too_large(cap, what)),
        _ => Ok(()),
    }
}

/// Append a chunk unless the running total would pass the cap, so a lying
/// Content-Length or a chunked stream cannot balloon memory.
pub(crate) fn push_capped(
    buf: &mut Vec<u8>,
    chunk: &[u8],
    cap: u64,
    what: &str,
) -> Result<(), AppCommandError> {
    if buf.len() as u64 + chunk.len() as u64 > cap {
        return Err(too_large(cap, what));
    }
    buf.extend_from_slice(chunk);
    Ok(())
}

async fn read_capped(
    mut resp: reqwest::Response,
    cap: u64,
    what: &str,
) -> Result<Vec<u8>, AppCommandError> {
    check_declared_length(resp.content_length(), cap, what)?;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AppCommandError::network(format!("{what} failed mid-transfer: {e}")))?
    {
        push_capped(&mut buf, &chunk, cap, what)?;
    }
    Ok(buf)
}

// ─── Fetch ───────────────────────────────────────────────────────────────

/// GET an already policy-checked URL with a byte cap and a deadline. A non-2xx
/// status is a refusal that names the status and nothing else.
async fn fetch_capped(
    url: &reqwest::Url,
    cap: u64,
    what: &str,
) -> Result<Vec<u8>, AppCommandError> {
    let resp = client()?
        .get(url.clone())
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppCommandError::network(format!("{what} fetch failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppCommandError::network(format!(
            "{what} returned HTTP {}",
            resp.status()
        )));
    }
    read_capped(resp, cap, what).await
}

fn document_from_bytes(bytes: Vec<u8>, what: &str) -> Result<GalleryDocument, AppCommandError> {
    let len = bytes.len() as u64;
    let sha256 = sha256_hex(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| {
        AppCommandError::configuration_invalid(format!("{what} is not valid UTF-8."))
    })?;
    Ok(GalleryDocument {
        text,
        sha256,
        bytes: len,
    })
}

/// The gallery index at `url`, as text plus its digest.
pub async fn fetch_index(url: &str) -> Result<GalleryDocument, AppCommandError> {
    let url = parse_gallery_https_url(url)?;
    let bytes = fetch_capped(&url, MAX_INDEX_BYTES, "Preset gallery index").await?;
    document_from_bytes(bytes, "Preset gallery index")
}

/// One preset file, only if its bytes hash to `listed_sha256`.
pub async fn fetch_preset(
    url: &str,
    listed_sha256: &str,
) -> Result<GalleryDocument, AppCommandError> {
    // Both refusals happen before any network is touched.
    let expected = normalize_sha256(listed_sha256)?;
    let url = parse_gallery_https_url(url)?;
    let bytes = fetch_capped(&url, MAX_PRESET_BYTES, "Preset file").await?;
    verify_sha256(&bytes, &expected)?;
    document_from_bytes(bytes, "Preset file")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy_accepts_public_https_only() {
        assert!(
            parse_gallery_https_url("https://raw.githubusercontent.com/o/r/main/index.json")
                .is_ok()
        );
        assert!(parse_gallery_https_url(" https://example.com/index.json ").is_ok());
        assert!(parse_gallery_https_url("http://example.com/index.json").is_err());
        assert!(parse_gallery_https_url("file:///etc/passwd").is_err());
        assert!(parse_gallery_https_url("https://user:pw@example.com/index.json").is_err());
        assert!(parse_gallery_https_url("https://127.0.0.1/index.json").is_err());
        assert!(parse_gallery_https_url("https://[::1]/index.json").is_err());
        assert!(parse_gallery_https_url("https://localhost/index.json").is_err());
        assert!(parse_gallery_https_url("https://gallery.localhost/index.json").is_err());
        assert!(parse_gallery_https_url("https://169.254.169.254/latest/meta-data/").is_err());
        assert!(parse_gallery_https_url("not a url").is_err());
    }

    #[test]
    fn redirect_hops_are_held_to_the_same_policy() {
        let allowed = |raw: &str| is_allowed_gallery_hop(&reqwest::Url::parse(raw).unwrap());
        assert!(allowed("https://cdn.example.com/presets/x.json"));
        assert!(!allowed("http://cdn.example.com/presets/x.json"));
        assert!(!allowed("https://10.0.0.8/x.json"));
        assert!(!allowed("https://localhost:8443/x.json"));
        assert!(!allowed("https://u:p@cdn.example.com/x.json"));
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn listed_digest_must_be_well_formed_and_match() {
        let body = br#"{"schemaVersion":1}"#;
        let digest = sha256_hex(body);
        assert_eq!(digest.len(), 64);
        assert_eq!(verify_sha256(body, &digest).unwrap(), digest);
        // Case and surrounding whitespace in the listing are tolerated.
        let shouted = format!(" {} ", digest.to_ascii_uppercase());
        assert_eq!(verify_sha256(body, &shouted).unwrap(), digest);
        // Any other digest, or a malformed one, is a refusal.
        assert!(verify_sha256(body, &sha256_hex(b"something else")).is_err());
        assert!(normalize_sha256("abc").is_err());
        assert!(normalize_sha256(&"g".repeat(64)).is_err());
        assert!(normalize_sha256(&"a".repeat(63)).is_err());
    }

    #[test]
    fn caps_apply_to_declared_length_and_running_total() {
        assert!(check_declared_length(None, 8, "x").is_ok());
        assert!(check_declared_length(Some(8), 8, "x").is_ok());
        assert!(check_declared_length(Some(9), 8, "x").is_err());

        let mut buf = Vec::new();
        assert!(push_capped(&mut buf, &[0u8; 5], 8, "x").is_ok());
        assert!(push_capped(&mut buf, &[0u8; 3], 8, "x").is_ok());
        assert_eq!(buf.len(), 8);
        assert!(push_capped(&mut buf, &[0u8; 1], 8, "x").is_err());
        assert_eq!(buf.len(), 8, "a refused chunk is not appended");
    }

    #[tokio::test]
    async fn refusals_happen_before_any_network() {
        // Nothing listens on these; a refusal here is the policy, not a timeout.
        assert!(fetch_index("http://127.0.0.1:9/index.json").await.is_err());
        assert!(fetch_preset("https://127.0.0.1:9/x.json", &"a".repeat(64))
            .await
            .is_err());
        assert!(fetch_preset("https://example.com/x.json", "not-a-digest")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn fetch_enforces_caps_and_reports_the_digest() {
        use axum::routing::get;

        const SMALL: &[u8] =
            br#"{"schemaVersion":1,"id":"x","name":"X","base":"neutral","colors":{}}"#;

        let app = axum::Router::new()
            // Declares an oversized length up front: rejected without reading
            // the body at all.
            .route(
                "/declared.json",
                get(|| async { vec![b' '; (MAX_PRESET_BYTES * 2) as usize] }),
            )
            // Chunked with no Content-Length, so only the running total can
            // stop it; the case a length check alone would miss.
            .route(
                "/chunked.json",
                get(|| async {
                    let chunks = futures_util::stream::iter((0..8).map(|_| {
                        Ok::<_, std::io::Error>(vec![b' '; (MAX_PRESET_BYTES / 4) as usize])
                    }));
                    axum::response::Response::builder()
                        .body(axum::body::Body::from_stream(chunks))
                        .unwrap()
                }),
            )
            .route("/small.json", get(|| async { SMALL }))
            .route(
                "/missing.json",
                get(|| async { axum::http::StatusCode::NOT_FOUND }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let url = |path: &str| reqwest::Url::parse(&format!("http://{addr}{path}")).unwrap();

        assert!(fetch_capped(&url("/declared.json"), MAX_PRESET_BYTES, "x")
            .await
            .is_err());
        assert!(fetch_capped(&url("/chunked.json"), MAX_PRESET_BYTES, "x")
            .await
            .is_err());
        assert!(fetch_capped(&url("/missing.json"), MAX_PRESET_BYTES, "x")
            .await
            .is_err());

        let bytes = fetch_capped(&url("/small.json"), MAX_PRESET_BYTES, "x")
            .await
            .expect("within the cap");
        assert_eq!(bytes, SMALL);
        let doc = document_from_bytes(bytes.clone(), "x").unwrap();
        assert_eq!(doc.text, String::from_utf8_lossy(SMALL));
        assert_eq!(doc.sha256, sha256_hex(SMALL));
        assert_eq!(doc.bytes, SMALL.len() as u64);
        // The pinned digest is what lets the file through; any other refuses it.
        assert!(verify_sha256(&bytes, &doc.sha256).is_ok());
        assert!(verify_sha256(&bytes, &sha256_hex(b"tampered")).is_err());

        server.abort();
    }
}
