//! Pet marketplace integration with [codex-pets.net](https://codex-pets.net/).
//!
//! Two operations are exposed:
//! - `list(...)` — proxies the public `GET /api/pets` listing endpoint and
//!   annotates each entry with `alreadyInstalled` based on what is already
//!   under `~/.codeg/pets/`.
//! - `install(...)` — downloads `GET /api/pets/{id}/download` (a zip
//!   containing `pet.json` + `spritesheet.webp`), validates the contents
//!   with the same rules as a manual `pet_add`, and atomically renames the
//!   staging dir into place.
//!
//! All HTTP traffic uses a process-wide `reqwest::Client` configured with a
//! short timeout and a stable user-agent so the upstream service can spot
//! codeg traffic distinctly from `codex` traffic.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::app_error::AppCommandError;
use crate::models::pet::{PetManifest, PetSummary, PET_MANIFEST_FILENAME, SPRITESHEET_FILENAME};
use crate::pets::{
    ensure_pets_root_or_create, list_existing_ids, validate_pet_id, validate_spritesheet,
};

const MARKETPLACE_BASE_URL: &str = "https://codex-pets.net";
/// Strict prefix used to validate `downloadUrl` returned by upstream. The
/// trailing slash is load-bearing: without it `https://codex-pets.net.evil/`
/// would also match.
const MARKETPLACE_URL_PREFIX: &str = "https://codex-pets.net/";
/// Hard cap on a single pet zip download. Real packages are well under this
/// (the manifest is ~150 B, the spritesheet ~3 MiB) — 32 MiB is purely a
/// guardrail against a malformed or hostile response.
const MAX_DOWNLOAD_BYTES: u64 = 32 * 1024 * 1024;
/// Cap on the JSON listing payload. The real upstream returns ~50 KiB per
/// page; 4 MiB leaves headroom while preventing memory blow-ups from a
/// hostile or malfunctioning server.
const MAX_LIST_BYTES: u64 = 4 * 1024 * 1024;
/// Cap for the `pet.json` entry inside a downloaded package. Real manifests
/// are ~150 B; 64 KiB is generous while still cheap to verify against
/// zip-bomb–style declared sizes.
const MAX_MANIFEST_ENTRY_BYTES: usize = 64 * 1024;
/// Cap for the `spritesheet.webp` entry. Matches `pets::MAX_SPRITE_BYTES`
/// so a malformed package fails fast during extraction instead of after a
/// full uncompress.
const MAX_SPRITESHEET_ENTRY_BYTES: usize = 16 * 1024 * 1024;
/// Cap for a proxied marketplace image (poster / preview / spritesheet).
/// Matches the spritesheet entry cap so an oversized asset fails fast.
const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;

/// Per-pet-id locks serializing concurrent `install(...)` calls. Keys are
/// pet ids; entries are kept indefinitely (each is a small `Arc<Mutex<()>>`
/// — bounded by the number of distinct pets a user ever installs).
static INSTALL_LOCKS: LazyLock<AsyncMutex<HashMap<String, Arc<AsyncMutex<()>>>>> =
    LazyLock::new(|| AsyncMutex::new(HashMap::new()));

async fn acquire_install_lock(id: &str) -> Arc<AsyncMutex<()>> {
    let mut map = INSTALL_LOCKS.lock().await;
    map.entry(id.to_string())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

static MARKETPLACE_HTTP_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .user_agent("codeg-pet-market/1.0")
        .build()
        .map_err(|e| format!("failed to initialize pet marketplace HTTP client: {e}"))
});

fn client() -> Result<&'static reqwest::Client, AppCommandError> {
    MARKETPLACE_HTTP_CLIENT
        .as_ref()
        .map_err(|err| AppCommandError::network(err.clone()))
}

// ─── Wire types ─────────────────────────────────────────────────────────

/// Query parameters accepted by `list(...)`. `kind` accepts `"pet"` or
/// `"person"`; anything else is forwarded to the upstream as-is so future
/// kinds work without a redeploy.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceListParams {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Per-pet entry exposed to the frontend. We re-serialize a subset of the
/// upstream record rather than passing it through verbatim so the contract
/// is stable even if codex-pets adds fields we don't consume.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplacePetSummary {
    pub id: String,
    pub display_name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_handle: Option<String>,
    #[serde(default)]
    pub view_count: u64,
    #[serde(default)]
    pub download_count: u64,
    #[serde(default)]
    pub like_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    pub download_url: String,
    pub already_installed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceListResponse {
    pub pets: Vec<MarketplacePetSummary>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
    pub total_pages: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceInstallRequest {
    pub id: String,
    pub download_url: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceInstallResponse {
    pub pet: PetSummary,
}

// ─── Internal upstream shape ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpstreamListResponse {
    #[serde(default)]
    pets: Vec<UpstreamPet>,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_page_size")]
    page_size: u32,
    #[serde(default)]
    total: u64,
    #[serde(default)]
    total_pages: u32,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    30
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpstreamPet {
    id: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    owner_name: Option<String>,
    #[serde(default)]
    owner_handle: Option<String>,
    #[serde(default)]
    view_count: u64,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    uploaded_at: Option<String>,
    #[serde(default)]
    poster_url: Option<String>,
    #[serde(default)]
    preview_url: Option<String>,
    #[serde(default)]
    download_url: Option<String>,
}

// ─── list ───────────────────────────────────────────────────────────────

pub async fn list(
    params: MarketplaceListParams,
) -> Result<MarketplaceListResponse, AppCommandError> {
    let url = format!("{MARKETPLACE_BASE_URL}/api/pets");
    let mut qs: Vec<(&'static str, String)> = Vec::new();
    if let Some(p) = params.page {
        qs.push(("page", p.to_string()));
    }
    if let Some(s) = params.page_size {
        qs.push(("pageSize", s.to_string()));
    }
    if let Some(q) = params.q.as_ref() {
        push_q_param(&mut qs, q);
    }
    if let Some(k) = params.kind.as_ref() {
        let trimmed = k.trim();
        if !trimmed.is_empty() {
            qs.push(("kind", trimmed.to_string()));
        }
    }
    if let Some(s) = params.sort.as_ref() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            qs.push(("sort", trimmed.to_string()));
        }
    }
    if let Some(tags) = params.tags.as_ref() {
        for tag in tags {
            let trimmed = tag.trim();
            if !trimmed.is_empty() {
                qs.push(("tags", trimmed.to_string()));
            }
        }
    }

    let response =
        client()?.get(&url).query(&qs).send().await.map_err(|e| {
            AppCommandError::network(format!("Failed to reach pet marketplace: {e}"))
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppCommandError::network(format!(
            "Pet marketplace returned HTTP {status}"
        )));
    }
    let bytes = read_capped_response(response, MAX_LIST_BYTES, "Marketplace listing").await?;
    let upstream: UpstreamListResponse = serde_json::from_slice(&bytes).map_err(|e| {
        AppCommandError::network(format!("Marketplace response is not valid JSON: {e}"))
    })?;

    let installed = list_existing_ids().unwrap_or_default();
    let pets = upstream
        .pets
        .into_iter()
        .map(|p| project_pet(p, &installed))
        .collect::<Vec<_>>();

    Ok(MarketplaceListResponse {
        pets,
        page: upstream.page,
        page_size: upstream.page_size,
        total: upstream.total,
        total_pages: upstream.total_pages,
    })
}

fn push_q_param(qs: &mut Vec<(&'static str, String)>, q: &str) {
    let trimmed = q.trim();
    if !trimmed.is_empty() {
        qs.push(("q", trimmed.to_string()));
    }
}

fn project_pet(p: UpstreamPet, installed: &HashSet<String>) -> MarketplacePetSummary {
    let already_installed = installed.contains(&p.id);
    let description = p.description.unwrap_or_default();
    let download_url = p
        .download_url
        .unwrap_or_else(|| format!("/api/pets/{}/download", p.id));
    MarketplacePetSummary {
        id: p.id,
        display_name: p.display_name,
        description,
        kind: p.kind,
        tags: p.tags,
        owner_name: p.owner_name,
        owner_handle: p.owner_handle,
        view_count: p.view_count,
        download_count: p.download_count,
        like_count: p.like_count,
        uploaded_at: p.uploaded_at,
        poster_url: p.poster_url,
        preview_url: p.preview_url,
        download_url,
        already_installed,
    }
}

// ─── install ────────────────────────────────────────────────────────────

pub async fn install(
    request: MarketplaceInstallRequest,
) -> Result<MarketplaceInstallResponse, AppCommandError> {
    validate_pet_id(&request.id)?;

    // Serialize concurrent installs for the same pet id so two callers can't
    // race on the shared `<id>.market.tmp` staging dir.
    let lock = acquire_install_lock(&request.id).await;
    let _guard = lock.lock().await;

    let download_url = absolute_download_url(&request.download_url)?;
    let bytes = download_zip(&download_url).await?;

    let id = request.id.clone();
    let overwrite = request.overwrite;
    let summary =
        tokio::task::spawn_blocking(move || install_from_zip_bytes(&id, &bytes, overwrite))
            .await
            .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))??;

    Ok(MarketplaceInstallResponse { pet: summary })
}

fn absolute_download_url(raw: &str) -> Result<String, AppCommandError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppCommandError::invalid_input("Download URL is empty."));
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        // Restrict to the codex-pets host so we never follow a forged URL
        // to an attacker-controlled binary blob. The trailing-slash prefix
        // prevents subdomain bypasses like `https://codex-pets.net.evil/`.
        if !trimmed.starts_with(MARKETPLACE_URL_PREFIX) {
            return Err(AppCommandError::invalid_input(format!(
                "Download URL must point to {MARKETPLACE_BASE_URL}."
            )));
        }
        Ok(trimmed.to_string())
    } else if trimmed.starts_with('/') {
        Ok(format!("{MARKETPLACE_BASE_URL}{trimmed}"))
    } else {
        Err(AppCommandError::invalid_input(
            "Download URL must be absolute or start with '/'.",
        ))
    }
}

async fn download_zip(url: &str) -> Result<Vec<u8>, AppCommandError> {
    let response = client()?
        .get(url)
        .send()
        .await
        .map_err(|e| AppCommandError::network(format!("Failed to download pet: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppCommandError::network(format!(
            "Pet download returned HTTP {status}"
        )));
    }
    read_capped_response(response, MAX_DOWNLOAD_BYTES, "Pet package").await
}

/// Streams `response` into a `Vec<u8>`, aborting as soon as the running
/// total would exceed `cap`. Avoids `response.bytes()` which buffers the
/// entire body before we ever see the size — that lets a server with no
/// `Content-Length` push past the cap into memory before we can stop it.
async fn read_capped_response(
    response: reqwest::Response,
    cap: u64,
    label: &str,
) -> Result<Vec<u8>, AppCommandError> {
    if let Some(content_length) = response.content_length() {
        if content_length > cap {
            return Err(AppCommandError::invalid_input(format!(
                "{label} is {content_length} bytes, exceeds {cap} byte cap."
            )));
        }
    }
    let initial = response
        .content_length()
        .unwrap_or(0)
        .min(cap)
        .min(usize::MAX as u64) as usize;
    let mut buf: Vec<u8> = Vec::with_capacity(initial);
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| AppCommandError::network(format!("Failed to read {label}: {e}")))?
    {
        let next_total = buf.len() as u64 + chunk.len() as u64;
        if next_total > cap {
            return Err(AppCommandError::invalid_input(format!(
                "{label} exceeds {cap} byte cap."
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

// ─── asset proxy ─────────────────────────────────────────────────────────

/// Proxy a marketplace image asset (poster / preview / spritesheet) through the
/// backend. The desktop webview can't always reach codex-pets.net directly
/// (Cloudflare is unreachable on some networks), yet this backend already talks
/// to the service for the listing and install. Fetching the bytes here and
/// handing them to the renderer as a blob URL makes the images load wherever
/// the listing itself does.
///
/// Only `https://codex-pets.net/...` URLs are honored — the same host guard as
/// [`absolute_download_url`], so a forged URL can't turn this into an open proxy
/// / SSRF vector. Returns the response content type (defaulting to `image/webp`)
/// alongside the raw bytes.
pub async fn fetch_asset(url: &str) -> Result<(String, Vec<u8>), AppCommandError> {
    let trimmed = url.trim();
    // The trailing-slash prefix prevents subdomain bypasses like
    // `https://codex-pets.net.evil/` and rejects the `http://` scheme.
    if !trimmed.starts_with(MARKETPLACE_URL_PREFIX) {
        return Err(AppCommandError::invalid_input(format!(
            "Asset URL must point to {MARKETPLACE_BASE_URL}."
        )));
    }

    let response = client()?
        .get(trimmed)
        .send()
        .await
        .map_err(|e| AppCommandError::network(format!("Failed to fetch pet asset: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppCommandError::network(format!(
            "Pet asset returned HTTP {status}"
        )));
    }

    // Capture the content type before `read_capped_response` consumes the body.
    // Keep only `image/*` types; anything else (e.g. an HTML error page served
    // with 200) falls back to the canonical `image/webp` these assets use.
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .filter(|m| m.starts_with("image/"))
        .unwrap_or_else(|| "image/webp".to_string());

    let bytes = read_capped_response(response, MAX_ASSET_BYTES, "Pet asset").await?;
    Ok((mime, bytes))
}

fn install_from_zip_bytes(
    expected_id: &str,
    bytes: &[u8],
    overwrite: bool,
) -> Result<PetSummary, AppCommandError> {
    let (manifest_bytes, sprite_bytes) = extract_zip_payload(bytes)?;

    let mut manifest: PetManifest = serde_json::from_slice(&manifest_bytes).map_err(|e| {
        AppCommandError::invalid_input(format!("Invalid pet.json in download: {e}"))
    })?;

    validate_pet_id(&manifest.id)?;
    if manifest.id != expected_id {
        return Err(AppCommandError::invalid_input(format!(
            "Manifest id '{}' does not match requested pet id '{}'.",
            manifest.id, expected_id
        )));
    }
    if manifest.display_name.trim().is_empty() {
        return Err(AppCommandError::invalid_input(
            "Manifest displayName is empty.",
        ));
    }
    // Always store under the canonical filename, regardless of what the
    // upstream manifest claims.
    manifest.spritesheet_path = SPRITESHEET_FILENAME.to_string();

    validate_spritesheet(&sprite_bytes)?;

    let root = ensure_pets_root_or_create()?;
    let target = root.join(expected_id);
    let staging = root.join(format!("{expected_id}.market.tmp"));
    let aside = root.join(format!("{expected_id}.replaced.tmp"));

    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    if aside.exists() {
        let _ = fs::remove_dir_all(&aside);
    }

    let target_existed = target.exists();
    if target_existed && !overwrite {
        return Err(AppCommandError::already_exists(format!(
            "Pet '{expected_id}' is already installed."
        )));
    }

    fs::create_dir_all(&staging).map_err(AppCommandError::io)?;

    if let Err(err) = write_staging(&staging, &manifest, &sprite_bytes) {
        let _ = fs::remove_dir_all(&staging);
        return Err(err);
    }

    // Move the existing dir aside, then promote staging. If anything fails,
    // try to restore the previous version so the user isn't left without a
    // pet that was working a second ago.
    if target_existed {
        if let Err(err) = fs::rename(&target, &aside) {
            let _ = fs::remove_dir_all(&staging);
            return Err(AppCommandError::io(err));
        }
    }
    if let Err(err) = fs::rename(&staging, &target) {
        let _ = fs::remove_dir_all(&staging);
        if target_existed {
            // Best-effort rollback.
            let _ = fs::rename(&aside, &target);
        }
        return Err(AppCommandError::io(err));
    }
    if target_existed {
        let _ = fs::remove_dir_all(&aside);
    }

    Ok(PetSummary {
        id: manifest.id,
        display_name: manifest.display_name,
        description: manifest.description,
        spritesheet_path: target.join(SPRITESHEET_FILENAME),
    })
}

fn extract_zip_payload(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>), AppCommandError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| {
        AppCommandError::invalid_input(format!("Pet package is not a valid zip: {e}"))
    })?;

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut sprite_bytes: Option<Vec<u8>> = None;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppCommandError::invalid_input(format!("Bad zip entry: {e}")))?;
        let name = match entry.enclosed_name() {
            Some(p) => p,
            None => {
                return Err(AppCommandError::invalid_input(
                    "Zip entry has an invalid path.",
                ));
            }
        };
        let name_str = name.to_string_lossy().replace('\\', "/");
        // Strict allow-list: refuse anything other than the two canonical files,
        // and reject directory traversal or nested paths just in case.
        match name_str.as_str() {
            PET_MANIFEST_FILENAME => {
                manifest_bytes = Some(read_zip_entry_capped(
                    &mut entry,
                    MAX_MANIFEST_ENTRY_BYTES,
                    PET_MANIFEST_FILENAME,
                )?);
            }
            SPRITESHEET_FILENAME => {
                sprite_bytes = Some(read_zip_entry_capped(
                    &mut entry,
                    MAX_SPRITESHEET_ENTRY_BYTES,
                    SPRITESHEET_FILENAME,
                )?);
            }
            other => {
                if entry.is_dir() {
                    continue;
                }
                return Err(AppCommandError::invalid_input(format!(
                    "Unexpected zip entry '{other}'."
                )));
            }
        }
    }

    let manifest = manifest_bytes
        .ok_or_else(|| AppCommandError::invalid_input("Pet package is missing pet.json."))?;
    let sprite = sprite_bytes.ok_or_else(|| {
        AppCommandError::invalid_input("Pet package is missing spritesheet.webp.")
    })?;
    Ok((manifest, sprite))
}

/// Reads a zip entry into a fresh `Vec<u8>`, aborting if the decompressed
/// stream exceeds `max_bytes`. Crucially, we do not trust the entry's
/// declared `size()` — a hostile package can advertise a tiny size and
/// expand to gigabytes (zip-bomb pattern). Wrapping the reader in
/// `Read::take(max + 1)` enforces the cap regardless of header values.
fn read_zip_entry_capped<R: Read>(
    entry: R,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, AppCommandError> {
    let limit = (max_bytes as u64).saturating_add(1);
    let mut buf = Vec::new();
    entry
        .take(limit)
        .read_to_end(&mut buf)
        .map_err(AppCommandError::io)?;
    if buf.len() > max_bytes {
        return Err(AppCommandError::invalid_input(format!(
            "Zip entry '{label}' exceeds {max_bytes} byte cap."
        )));
    }
    Ok(buf)
}

fn write_staging(
    dir: &std::path::Path,
    manifest: &PetManifest,
    sprite_bytes: &[u8],
) -> Result<(), AppCommandError> {
    use std::io::Write;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| AppCommandError::io_error(format!("Failed to serialize manifest: {e}")))?;
    {
        let mut f =
            fs::File::create(dir.join(PET_MANIFEST_FILENAME)).map_err(AppCommandError::io)?;
        f.write_all(json.as_bytes()).map_err(AppCommandError::io)?;
        f.write_all(b"\n").map_err(AppCommandError::io)?;
        f.sync_all().map_err(AppCommandError::io)?;
    }
    {
        let mut f =
            fs::File::create(dir.join(SPRITESHEET_FILENAME)).map_err(AppCommandError::io)?;
        f.write_all(sprite_bytes).map_err(AppCommandError::io)?;
        f.sync_all().map_err(AppCommandError::io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_q_param_trims_and_skips_empty_values() {
        let mut qs = Vec::new();

        push_q_param(&mut qs, "  cat  ");
        push_q_param(&mut qs, " ");

        assert_eq!(qs, vec![("q", "cat".to_string())]);
    }

    #[test]
    fn absolute_download_url_accepts_relative_paths() {
        let url = absolute_download_url("/api/pets/foo/download").unwrap();
        assert_eq!(url, format!("{MARKETPLACE_BASE_URL}/api/pets/foo/download"));
    }

    #[test]
    fn absolute_download_url_accepts_marketplace_host() {
        let raw = format!("{MARKETPLACE_BASE_URL}/api/pets/foo/download?v=123");
        let url = absolute_download_url(&raw).unwrap();
        assert_eq!(url, raw);
    }

    #[test]
    fn absolute_download_url_rejects_other_hosts() {
        assert!(absolute_download_url("https://evil.example/pet.zip").is_err());
    }

    #[test]
    fn absolute_download_url_rejects_relative_without_slash() {
        assert!(absolute_download_url("api/pets/foo").is_err());
    }

    #[test]
    fn absolute_download_url_rejects_subdomain_bypass() {
        // Without the trailing-slash prefix this would have passed.
        assert!(absolute_download_url("https://codex-pets.net.evil.example/x.zip").is_err());
        assert!(absolute_download_url("https://codex-pets.network/x.zip").is_err());
    }

    #[test]
    fn absolute_download_url_rejects_http_scheme() {
        assert!(absolute_download_url("http://codex-pets.net/api/pets/foo/download").is_err());
    }

    #[tokio::test]
    async fn fetch_asset_rejects_non_marketplace_urls() {
        // Host guard short-circuits before any network call, so these resolve
        // without touching the wire.
        assert!(fetch_asset("https://evil.example/x.webp").await.is_err());
        assert!(fetch_asset("https://codex-pets.net.evil/x.webp")
            .await
            .is_err());
        assert!(fetch_asset("http://codex-pets.net/assets/x.webp")
            .await
            .is_err());
        assert!(fetch_asset("/assets/pets/x.webp").await.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn install_locks_serialize_same_id() {
        let a = acquire_install_lock("foo").await;
        let b = acquire_install_lock("foo").await;
        // Same id → same Arc instance, so concurrent holders block each other.
        assert!(Arc::ptr_eq(&a, &b));

        let c = acquire_install_lock("bar").await;
        assert!(!Arc::ptr_eq(&a, &c));
    }

    #[test]
    fn read_zip_entry_capped_accepts_within_cap() {
        let payload = vec![0x42u8; 1024];
        let out = read_zip_entry_capped(payload.as_slice(), 4096, "test").unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn read_zip_entry_capped_rejects_over_cap() {
        // Simulates a zip-bomb entry that decompresses past the limit.
        let payload = vec![0x42u8; 8192];
        let err = read_zip_entry_capped(payload.as_slice(), 4096, "bomb").unwrap_err();
        assert!(err.message.contains("4096 byte cap"));
    }

    #[test]
    fn read_zip_entry_capped_accepts_exact_cap() {
        let payload = vec![0u8; 1024];
        let out = read_zip_entry_capped(payload.as_slice(), 1024, "exact").unwrap();
        assert_eq!(out.len(), 1024);
    }
}
