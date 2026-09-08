//! The `codeg-doc:` document guest: a local HTML file shown through a webview
//! of its own, whose every request the host answers from the file's folder.
//! This is what the desktop shows for an `.html` file instead of an inline
//! `srcdoc` preview: a real document with a real URL, so routing, `fetch` of
//! sibling files and relative links work — inside a fence.
//!
//! The fence, in order of importance:
//!
//! - **Only guests have the scheme.** The handler is registered on the guest
//!   webview's own builder, so the app's webview and ordinary browser tabs
//!   cannot address `codeg-doc:` at all; and the handler is bound to one
//!   grant (root + entry) and checks the webview id it is called for.
//! - **Only files under the root.** Same rule as the inline preview: the root
//!   is the workspace folder the file sits in (else its own directory), the
//!   path is canonicalized and confined, and the final component is opened
//!   without following a symlink. No directory listings.
//! - **Safe mode by default.** The document is served with a CSP that runs
//!   no script and opens no connection; images, styles and fonts come from
//!   the root only. **Dynamic mode** is a per-file, per-session decision by
//!   the user: scripts run, but still only from the root, and the only
//!   endpoint they can reach is the root (`connect-src 'self'`).
//! - **What was approved is what runs.** Dynamic mode records when it was
//!   granted; a file whose timestamps are newer than that, or whose content
//!   differs from what was served since, drops the guest back to safe mode
//!   and is not served. The document the user approved cannot be swapped
//!   underneath the approval.
//! - **A guest goes nowhere else.** Top-level navigation to a web address is
//!   refused and reported so the user can open it in a browser tab;
//!   `window.open` and downloads are refused; the data store is
//!   non-persistent and dies with the guest.
//!
//! A grant lives for the session: switching files and coming back keeps the
//! mode the user chose (and the approval time it was chosen at).

use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::{header, Request, Response, StatusCode, Uri};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Url;

/// Label prefix of every document guest webview. Like `browser-`, it must
/// never appear in a capability (`mod.rs` has the test).
pub const DOC_LABEL_PREFIX: &str = "codeg-doc-";
pub const DOC_SCHEME: &str = "codeg-doc";
/// The host part of every document URL. Constant on purpose: the grant is
/// bound to the webview, not carried in the URL, and a constant origin keeps
/// `'self'` in the CSP meaning "this guest's root" on every platform.
pub const DOC_HOST: &str = "doc";

/// Largest file the guest serves. A document preview that needs more than
/// this in one response is not a document; the body is held in memory.
const MAX_BODY_BYTES: u64 = 512 * 1024 * 1024;
/// A range request is answered at most this large per response; the engine
/// asks for the rest as it needs it.
const MAX_RANGE_BYTES: u64 = 16 * 1024 * 1024;

pub fn doc_label(tab_id: &str) -> String {
    format!("{DOC_LABEL_PREFIX}{tab_id}")
}

/// Whether this build can host document guests: the embedded surface with a
/// per-webview scheme handler exists on macOS; the Windows and Linux shims
/// have not been exercised, and the inline preview stays in place there.
pub fn supported() -> bool {
    cfg!(all(
        feature = "browser-child",
        target_os = "macos"
    ))
}

/// A URL that addresses this guest's own root. wry maps a custom scheme to
/// `http(s)://<scheme>.<host>` on Windows, so both spellings count.
pub fn is_document_url(url: &Url) -> bool {
    url.scheme() == DOC_SCHEME
        || (matches!(url.scheme(), "http" | "https")
            && url
                .host_str()
                .is_some_and(|host| host.starts_with(&format!("{DOC_SCHEME}."))))
}

/// What a guest may navigate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestNavigation {
    Allow,
    /// A web address: not for the guest, but the user may want it in a tab.
    External,
    /// Anything else (`mailto:`, `file:`, app schemes, `data:` documents).
    Scheme,
}

/// The guest's navigation policy: its own documents, the blank page, and —
/// inside its own frames only — the opaque-origin content a page composes
/// itself. Everything with a scheme of its own stays out; web addresses are
/// reported as such so the user can follow them elsewhere.
pub fn guest_navigation(url: &Url, main_frame: bool) -> GuestNavigation {
    if is_document_url(url) {
        return GuestNavigation::Allow;
    }
    match url.scheme() {
        "about" if url.as_str() == "about:blank" => GuestNavigation::Allow,
        "about" if !main_frame && url.as_str() == "about:srcdoc" => GuestNavigation::Allow,
        "data" | "blob" if !main_frame => GuestNavigation::Allow,
        "http" | "https" => GuestNavigation::External,
        _ => GuestNavigation::Scheme,
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocMode {
    /// No script, no connection; the document as a picture of itself.
    Safe,
    /// Scripts from the root run and may fetch from the root.
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocResetReason {
    /// The file's timestamps are newer than the approval.
    Newer,
    /// The file's content differs from what was served since the approval.
    Changed,
}

/// Why a guest fell back to safe mode on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocReset {
    /// The offending file, relative to the root.
    pub path: String,
    pub reason: DocResetReason,
}

/// Mode and status of one guest, emitted on `browser://doc-state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocGuestState {
    pub tab_id: String,
    pub mode: DocMode,
    /// Absolute directory every request is confined to.
    pub root: String,
    /// Absolute path of the document.
    pub entry: String,
    /// The document's URL inside the guest.
    pub url: String,
    /// Set after the guest dropped back to safe mode by itself; cleared by
    /// the next explicit mode change.
    pub reset: Option<DocReset>,
}

// ---------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------

/// The state of dynamic mode: when it was granted and what has been served
/// since (path → SHA-256 of the bytes served).
struct Approval {
    approved_at: SystemTime,
    pins: HashMap<PathBuf, [u8; 32]>,
}

struct GrantInner {
    /// `None` = safe mode.
    approval: Option<Approval>,
    reset: Option<DocReset>,
}

/// One document: its root, its entry file, and the mode the user chose.
pub struct DocGrant {
    root: PathBuf,
    entry: PathBuf,
    entry_rel: PathBuf,
    inner: Mutex<GrantInner>,
}

/// Where a document lives, from what the frontend knows: the file, and the
/// root it should be confined to (the owning workspace folder). Both must
/// exist; the root falls back to the file's own directory when it is not
/// given or does not contain the file. Canonical paths come back.
pub fn resolve_document(path: &str, root: Option<&str>) -> Result<(PathBuf, PathBuf), String> {
    let entry = PathBuf::from(path);
    if !entry.is_absolute() {
        return Err(format!("document path must be absolute: {path:?}"));
    }
    let entry = std::fs::canonicalize(&entry).map_err(|e| format!("cannot open {path:?}: {e}"))?;
    if !entry.is_file() {
        return Err(format!("not a file: {path:?}"));
    }
    let parent = entry
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("document has no directory: {path:?}"))?;
    let root = match root {
        Some(root) => match std::fs::canonicalize(root) {
            Ok(root) if root.is_dir() && entry.starts_with(&root) => root,
            _ => parent,
        },
        None => parent,
    };
    Ok((root, entry))
}

impl DocGrant {
    /// `root` and `entry` canonical, `entry` under `root`.
    pub fn new(root: PathBuf, entry: PathBuf) -> Result<Self, String> {
        let entry_rel = entry
            .strip_prefix(&root)
            .map_err(|_| format!("{} is not under {}", entry.display(), root.display()))?
            .to_path_buf();
        Ok(Self {
            root,
            entry,
            entry_rel,
            inner: Mutex::new(GrantInner {
                approval: None,
                reset: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, GrantInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entry(&self) -> &Path {
        &self.entry
    }

    pub fn mode(&self) -> DocMode {
        if self.lock().approval.is_some() {
            DocMode::Dynamic
        } else {
            DocMode::Safe
        }
    }

    /// The user's decision. Dynamic mode starts a fresh approval — nothing
    /// served before it counts — and either way a pending reset is done with.
    pub fn set_mode(&self, mode: DocMode) {
        let mut inner = self.lock();
        inner.reset = None;
        inner.approval = match mode {
            DocMode::Safe => None,
            DocMode::Dynamic => Some(Approval {
                approved_at: SystemTime::now(),
                pins: HashMap::new(),
            }),
        };
    }

    /// The entry document's URL inside the guest.
    pub fn document_url(&self) -> String {
        document_url(&self.entry_rel)
    }

    pub fn state(&self, tab_id: &str) -> DocGuestState {
        let inner = self.lock();
        DocGuestState {
            tab_id: tab_id.to_string(),
            mode: if inner.approval.is_some() {
                DocMode::Dynamic
            } else {
                DocMode::Safe
            },
            root: self.root.to_string_lossy().into_owned(),
            entry: self.entry.to_string_lossy().into_owned(),
            url: self.document_url(),
            reset: inner.reset.clone(),
        }
    }

    /// Answer one request from the guest. Never fails: every outcome is a
    /// response, and `reset` says when this request ended dynamic mode.
    pub fn serve(&self, request: &Request<Vec<u8>>) -> Served {
        let mode = self.mode();
        let Some(rel) = request_rel_path(request.uri()) else {
            return Served::error(StatusCode::BAD_REQUEST, "not a document path", mode);
        };
        let (canonical, rel, mut file, metadata) = match self.open_confined(&rel) {
            Ok(opened) => opened,
            Err(status) => {
                return Served::error(status, status.canonical_reason().unwrap_or("error"), mode)
            }
        };
        if metadata.len() > MAX_BODY_BYTES {
            return Served::error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "file too large for a document preview",
                mode,
            );
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        if file.read_to_end(&mut bytes).is_err() {
            return Served::error(StatusCode::INTERNAL_SERVER_ERROR, "cannot read file", mode);
        }
        drop(file);
        let rel_display = rel.to_string_lossy().replace('\\', "/");
        // Dynamic mode: the file must be the one that was approved. Checked
        // and recorded under the lock, so two requests for a changed file
        // cannot both pass by racing the pin.
        if mode == DocMode::Dynamic {
            if let Err(reset) = self.verify_approved(&rel_display, &canonical, &metadata, &bytes) {
                let mut inner = self.lock();
                inner.approval = None;
                inner.reset = Some(reset.clone());
                let mut served = Served::error(
                    StatusCode::FORBIDDEN,
                    "file changed after scripts were enabled; the document is back in safe mode",
                    DocMode::Safe,
                );
                served.reset = Some(reset);
                return served;
            }
        }
        let content_type = content_type(&canonical);
        let total = bytes.len() as u64;
        let mut builder = response_builder(mode).header(header::CONTENT_TYPE, content_type);
        let range = request
            .headers()
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .map(|v| parse_range(v, total));
        let body = match range {
            Some(Some((start, end))) => {
                let end = end.min(start + MAX_RANGE_BYTES - 1).min(total - 1);
                builder = builder.status(StatusCode::PARTIAL_CONTENT).header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total}"),
                );
                bytes[start as usize..=end as usize].to_vec()
            }
            Some(None) => {
                return Served {
                    response: response_builder(mode)
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                        .body(Vec::new())
                        .expect("static response"),
                    reset: None,
                };
            }
            None => bytes,
        };
        Served {
            response: builder
                .header(header::CONTENT_LENGTH, body.len())
                .body(body)
                .expect("static response"),
            reset: None,
        }
    }

    /// Resolve `rel` under the root, confined: canonicalized (so a symlink
    /// cannot lead outside — a linked folder of the workspace is inside),
    /// a directory answered by its `index.html`, and the final component
    /// opened without following a symlink swapped in after the check.
    fn open_confined(&self, rel: &Path) -> Result<(PathBuf, PathBuf, File, Metadata), StatusCode> {
        let mut rel = rel.to_path_buf();
        let mut canonical =
            std::fs::canonicalize(self.root.join(&rel)).map_err(|_| StatusCode::NOT_FOUND)?;
        if canonical.is_dir() {
            rel.push("index.html");
            canonical = std::fs::canonicalize(canonical.join("index.html"))
                .map_err(|_| StatusCode::NOT_FOUND)?;
        }
        if !crate::commands::folders::is_within_workspace(&self.root, &canonical) {
            return Err(StatusCode::FORBIDDEN);
        }
        let file = crate::commands::folders::open_no_follow(&canonical)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let metadata = file.metadata().map_err(|_| StatusCode::NOT_FOUND)?;
        if !metadata.is_file() {
            return Err(StatusCode::NOT_FOUND);
        }
        Ok((canonical, rel, file, metadata))
    }

    /// Dynamic mode's check: the file's timestamps predate the approval, and
    /// its content is what was served since (pinned on first serve).
    fn verify_approved(
        &self,
        rel_display: &str,
        canonical: &Path,
        metadata: &Metadata,
        bytes: &[u8],
    ) -> Result<(), DocReset> {
        let mut inner = self.lock();
        let Some(approval) = inner.approval.as_mut() else {
            // Switched to safe mode while this request was in flight: serve
            // it as safe mode would (the CSP already went out with the
            // document; the next load is a safe one).
            return Ok(());
        };
        if newest_change(metadata) > approval.approved_at {
            return Err(DocReset {
                path: rel_display.to_string(),
                reason: DocResetReason::Newer,
            });
        }
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        match approval.pins.get(canonical) {
            Some(pinned) if *pinned != digest => Err(DocReset {
                path: rel_display.to_string(),
                reason: DocResetReason::Changed,
            }),
            Some(_) => Ok(()),
            None => {
                approval.pins.insert(canonical.to_path_buf(), digest);
                Ok(())
            }
        }
    }
}

/// The last time the file changed by any account the filesystem keeps: its
/// modification time and, where there is one, its status-change time (a
/// rename into place bumps the latter and not the former).
fn newest_change(metadata: &Metadata) -> SystemTime {
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let ctime = UNIX_EPOCH
            + Duration::new(
                metadata.ctime().max(0) as u64,
                metadata.ctime_nsec().clamp(0, 999_999_999) as u32,
            );
        modified.max(ctime)
    }
    #[cfg(not(unix))]
    {
        modified
    }
}

/// Response to a request, plus whether answering it ended dynamic mode.
pub struct Served {
    pub response: Response<Vec<u8>>,
    pub reset: Option<DocReset>,
}

impl Served {
    fn error(status: StatusCode, message: &str, mode: DocMode) -> Self {
        let body = message.as_bytes().to_vec();
        Served {
            response: response_builder(mode)
                .status(status)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .header(header::CONTENT_LENGTH, body.len())
                .body(body)
                .expect("static response"),
            reset: None,
        }
    }
}

/// The answer to a request from a webview that is not the grant's own. The
/// handler is per webview already; this is the belt to that suspender.
pub fn forbidden() -> Response<Vec<u8>> {
    Served::error(StatusCode::FORBIDDEN, "not this document's webview", DocMode::Safe).response
}

// ---------------------------------------------------------------------------
// Requests and responses
// ---------------------------------------------------------------------------

/// The document URL for a path relative to the root; each segment
/// percent-encoded by the URL parser.
pub fn document_url(rel: &Path) -> String {
    let mut url = Url::parse(&format!("{DOC_SCHEME}://{DOC_HOST}/")).expect("static url");
    {
        let mut segments = url.path_segments_mut().expect("url has a host");
        for component in rel.components() {
            segments.push(&component.as_os_str().to_string_lossy());
        }
    }
    url.to_string()
}

/// The path a request asks for, relative to the root, or `None` for a path
/// no document can have: a `.`/`..` segment, a separator inside a segment,
/// a NUL, or bytes that are not UTF-8. The host part is ignored (a Windows
/// guest sees a mapped host).
fn request_rel_path(uri: &Uri) -> Option<PathBuf> {
    let mut rel = PathBuf::new();
    for segment in uri.path().split('/') {
        if segment.is_empty() {
            continue;
        }
        let decoded = percent_encoding::percent_decode_str(segment)
            .decode_utf8()
            .ok()?;
        if decoded == "."
            || decoded == ".."
            || decoded.contains('\0')
            || decoded.contains('/')
            || decoded.contains('\\')
        {
            return None;
        }
        rel.push(&*decoded);
    }
    Some(rel)
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` (one range). `None` = not a range
/// this understands, answer the whole file; `Some(None)` = unsatisfiable.
fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || total == 0 {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            if n == 0 {
                return None;
            }
            (total.saturating_sub(n), total - 1)
        }
        (start, "") => (start.parse().ok()?, total - 1),
        (start, end) => (start.parse().ok()?, end.parse().ok()?),
    };
    if start > end || start >= total {
        return None;
    }
    Some((start, end.min(total - 1)))
}

/// Safe mode: no script, no connection, resources from the root only. The
/// inline preview's strict policy with real URLs instead of `data:`.
const CSP_SAFE: &str = "default-src 'none'; style-src 'self' 'unsafe-inline' data:; img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' data: blob:; script-src 'none'; connect-src 'none'; frame-src 'none'; worker-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'; object-src 'none'";

/// Dynamic mode: the document's own scripts run and may fetch from the root;
/// nothing points outside it. Inline scripts and `eval` are allowed because a
/// generated report is exactly the kind of file that has them, and they add
/// no reach: the only endpoint remains the root.
const CSP_DYNAMIC: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' 'unsafe-inline' data:; img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' data: blob:; connect-src 'self' data: blob:; frame-src 'self' data: blob:; worker-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'; object-src 'none'";

pub fn csp_for(mode: DocMode) -> &'static str {
    match mode {
        DocMode::Safe => CSP_SAFE,
        DocMode::Dynamic => CSP_DYNAMIC,
    }
}

fn response_builder(mode: DocMode) -> http::response::Builder {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_SECURITY_POLICY, csp_for(mode))
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header("X-DNS-Prefetch-Control", "off")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCEPT_RANGES, "bytes")
}

/// Content type by extension. Unknown types are `application/octet-stream`,
/// which with `nosniff` the engine neither renders nor runs.
pub fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" | "xhtml" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json; charset=utf-8",
        "xml" | "xsl" => "application/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" | "ogg" => "video/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "txt" | "md" | "markdown" | "log" | "csv" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Registry of grants
// ---------------------------------------------------------------------------

/// Every document the session has shown, by (root, entry), and which guest
/// tab currently shows which. A grant outlives its guest webview on purpose:
/// the guest is torn down whenever the file leaves the screen, and the mode
/// the user chose — with the approval time it was chosen at — must not be.
#[derive(Default)]
pub struct DocGuests {
    grants: Mutex<HashMap<(PathBuf, PathBuf), Arc<DocGrant>>>,
    by_tab: Mutex<HashMap<String, Arc<DocGrant>>>,
}

impl DocGuests {
    /// The grant for a document, created on first sight.
    pub fn grant_for(&self, root: PathBuf, entry: PathBuf) -> Result<Arc<DocGrant>, String> {
        let mut grants = self.grants.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(grant) = grants.get(&(root.clone(), entry.clone())) {
            return Ok(grant.clone());
        }
        let grant = Arc::new(DocGrant::new(root.clone(), entry.clone())?);
        grants.insert((root, entry), grant.clone());
        Ok(grant)
    }

    pub fn bind(&self, tab_id: &str, grant: Arc<DocGrant>) {
        self.by_tab
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(tab_id.to_string(), grant);
    }

    pub fn unbind(&self, tab_id: &str) {
        self.by_tab
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(tab_id);
    }

    pub fn for_tab(&self, tab_id: &str) -> Option<Arc<DocGrant>> {
        self.by_tab
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(tab_id)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    fn get(grant: &DocGrant, path: &str) -> Served {
        let request = Request::builder()
            .uri(format!("{DOC_SCHEME}://{DOC_HOST}{path}"))
            .body(Vec::new())
            .unwrap();
        grant.serve(&request)
    }

    fn grant_in(dir: &Path) -> DocGrant {
        let entry = write(dir, "site/index.html", b"<!doctype html><script src=app.js></script>");
        write(dir, "site/app.js", b"console.log(1)");
        write(dir, "site/img/a.png", &[0x89, b'P', b'N', b'G']);
        write(dir, "secret.txt", b"outside");
        let root = std::fs::canonicalize(dir.join("site")).unwrap();
        let entry = std::fs::canonicalize(entry).unwrap();
        DocGrant::new(root, entry).unwrap()
    }

    fn csp(served: &Served) -> String {
        served.response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn serves_files_under_the_root_with_types_and_fences() {
        let dir = tempfile::tempdir().unwrap();
        let grant = grant_in(dir.path());
        let page = get(&grant, "/index.html");
        assert_eq!(page.response.status(), StatusCode::OK);
        assert_eq!(page.response.headers()[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert!(csp(&page).contains("script-src 'none'"));
        assert_eq!(page.response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(page.response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(page.response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(page.response.body(), b"<!doctype html><script src=app.js></script>");
        // A directory answers with its index; the root itself too.
        assert_eq!(get(&grant, "/").response.status(), StatusCode::OK);
        assert_eq!(get(&grant, "/img/a.png").response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(get(&grant, "/img/").response.status(), StatusCode::NOT_FOUND);
        assert_eq!(get(&grant, "/missing.css").response.status(), StatusCode::NOT_FOUND);
        // Percent-encoded segments decode; traversal and separators do not.
        assert_eq!(get(&grant, "/img/%61.png").response.status(), StatusCode::OK);
        assert_eq!(get(&grant, "/../secret.txt").response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(get(&grant, "/%2e%2e/secret.txt").response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(get(&grant, "/img%2F..%2F..%2Fsecret.txt").response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(get(&grant, "/a%00.html").response.status(), StatusCode::BAD_REQUEST);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let grant = grant_in(dir.path());
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), dir.path().join("site/leak.txt"))
            .unwrap();
        assert_eq!(get(&grant, "/leak.txt").response.status(), StatusCode::FORBIDDEN);
        // A symlink to a sibling inside the root is fine.
        std::os::unix::fs::symlink(dir.path().join("site/app.js"), dir.path().join("site/alias.js"))
            .unwrap();
        assert_eq!(get(&grant, "/alias.js").response.status(), StatusCode::OK);
    }

    #[test]
    fn dynamic_mode_serves_approved_files_and_resets_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let grant = grant_in(dir.path());
        // Files written just now must count as approved: their timestamps
        // are not newer than an approval taken after them.
        grant.set_mode(DocMode::Dynamic);
        assert_eq!(grant.mode(), DocMode::Dynamic);
        let page = get(&grant, "/index.html");
        assert_eq!(page.response.status(), StatusCode::OK);
        assert!(csp(&page).contains("script-src 'self' 'unsafe-inline'"));
        assert!(csp(&page).contains("connect-src 'self'"));
        assert_eq!(get(&grant, "/app.js").response.status(), StatusCode::OK);
        assert!(page.reset.is_none());

        // The script is rewritten after the approval: refused, and the guest
        // is back in safe mode with the file named.
        std::thread::sleep(Duration::from_millis(20));
        write(dir.path(), "site/app.js", b"exfiltrate()");
        let served = get(&grant, "/app.js");
        assert_eq!(served.response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            served.reset,
            Some(DocReset {
                path: "app.js".into(),
                reason: DocResetReason::Newer
            })
        );
        assert_eq!(grant.mode(), DocMode::Safe);
        assert_eq!(grant.state("t").reset.as_ref().map(|r| r.path.as_str()), Some("app.js"));
        // Safe mode serves it (no script runs under the safe CSP anyway).
        let again = get(&grant, "/app.js");
        assert_eq!(again.response.status(), StatusCode::OK);
        assert!(csp(&again).contains("script-src 'none'"));
        // Re-approving clears the reset and starts afresh.
        grant.set_mode(DocMode::Dynamic);
        assert!(grant.state("t").reset.is_none());
        assert_eq!(get(&grant, "/app.js").response.status(), StatusCode::OK);
    }

    #[test]
    fn dynamic_mode_pins_what_it_served() {
        let dir = tempfile::tempdir().unwrap();
        let grant = grant_in(dir.path());
        grant.set_mode(DocMode::Dynamic);
        assert_eq!(get(&grant, "/app.js").response.status(), StatusCode::OK);
        // Content swapped with the timestamps put back: the pin catches it.
        let path = dir.path().join("site/app.js");
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, b"other()").unwrap();
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(before - Duration::from_secs(5)).unwrap();
        drop(file);
        let served = get(&grant, "/app.js");
        // Either the status-change time (unix) or the pinned hash refuses it.
        assert_eq!(served.response.status(), StatusCode::FORBIDDEN);
        assert!(served.reset.is_some());
        assert_eq!(grant.mode(), DocMode::Safe);
    }

    #[test]
    fn ranges_are_answered_in_bounded_pieces() {
        let dir = tempfile::tempdir().unwrap();
        let grant = grant_in(dir.path());
        write(dir.path(), "site/clip.mp4", &[0u8; 100]);
        let request = Request::builder()
            .uri(format!("{DOC_SCHEME}://{DOC_HOST}/clip.mp4"))
            .header(header::RANGE, "bytes=10-19")
            .body(Vec::new())
            .unwrap();
        let served = grant.serve(&request);
        assert_eq!(served.response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(served.response.headers()[header::CONTENT_RANGE], "bytes 10-19/100");
        assert_eq!(served.response.body().len(), 10);
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=0-1000", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-2", 100), None);
        assert_eq!(parse_range("bytes=0-1,3-4", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }

    #[test]
    fn document_urls_and_navigation_policy() {
        assert_eq!(document_url(Path::new("a b/c#d.html")), "codeg-doc://doc/a%20b/c%23d.html");
        assert_eq!(document_url(Path::new("index.html")), "codeg-doc://doc/index.html");
        let doc = Url::parse("codeg-doc://doc/other.html").unwrap();
        assert!(is_document_url(&doc));
        assert!(is_document_url(&Url::parse("https://codeg-doc.doc/x").unwrap()));
        assert!(!is_document_url(&Url::parse("https://example.com/").unwrap()));
        assert_eq!(guest_navigation(&doc, true), GuestNavigation::Allow);
        assert_eq!(
            guest_navigation(&Url::parse("https://example.com/").unwrap(), true),
            GuestNavigation::External
        );
        assert_eq!(
            guest_navigation(&Url::parse("mailto:a@b.c").unwrap(), true),
            GuestNavigation::Scheme
        );
        assert_eq!(
            guest_navigation(&Url::parse("file:///etc/hosts").unwrap(), true),
            GuestNavigation::Scheme
        );
        assert_eq!(
            guest_navigation(&Url::parse("about:blank").unwrap(), true),
            GuestNavigation::Allow
        );
        // Opaque-origin content in the document's own frames only.
        assert_eq!(
            guest_navigation(&Url::parse("about:srcdoc").unwrap(), false),
            GuestNavigation::Allow
        );
        assert_eq!(
            guest_navigation(&Url::parse("about:srcdoc").unwrap(), true),
            GuestNavigation::Scheme
        );
        assert_eq!(
            guest_navigation(&Url::parse("data:text/html,hi").unwrap(), false),
            GuestNavigation::Allow
        );
        assert_eq!(
            guest_navigation(&Url::parse("data:text/html,hi").unwrap(), true),
            GuestNavigation::Scheme
        );
    }

    #[test]
    fn resolve_document_confines_to_the_root_or_the_file_directory() {
        let dir = tempfile::tempdir().unwrap();
        let entry = write(dir.path(), "ws/docs/report.html", b"<p>hi</p>");
        let ws = std::fs::canonicalize(dir.path().join("ws")).unwrap();
        let entry_c = std::fs::canonicalize(&entry).unwrap();
        let (root, resolved) =
            resolve_document(entry.to_str().unwrap(), Some(ws.to_str().unwrap())).unwrap();
        assert_eq!(root, ws);
        assert_eq!(resolved, entry_c);
        // No root, or a root that does not contain the file: its own folder.
        let (root, _) = resolve_document(entry.to_str().unwrap(), None).unwrap();
        assert_eq!(root, entry_c.parent().unwrap());
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let (root, _) =
            resolve_document(entry.to_str().unwrap(), Some(elsewhere.to_str().unwrap())).unwrap();
        assert_eq!(root, entry_c.parent().unwrap());
        assert!(resolve_document("relative/path.html", None).is_err());
        assert!(resolve_document(ws.to_str().unwrap(), None).is_err());
        let grant = DocGrant::new(ws.clone(), entry_c).unwrap();
        assert_eq!(grant.document_url(), "codeg-doc://doc/docs/report.html");
        let state = grant.state("t1");
        assert_eq!(state.mode, DocMode::Safe);
        assert_eq!(state.root, ws.to_string_lossy());
        assert_eq!(serde_json::to_value(&state).unwrap()["mode"], "safe");
    }

    #[test]
    fn grants_are_shared_per_document_and_bound_per_tab() {
        let dir = tempfile::tempdir().unwrap();
        let entry = write(dir.path(), "a.html", b"x");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let entry = std::fs::canonicalize(entry).unwrap();
        let guests = DocGuests::default();
        let first = guests.grant_for(root.clone(), entry.clone()).unwrap();
        first.set_mode(DocMode::Dynamic);
        let second = guests.grant_for(root, entry).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.mode(), DocMode::Dynamic);
        guests.bind("t1", first.clone());
        assert!(guests.for_tab("t1").is_some());
        guests.unbind("t1");
        assert!(guests.for_tab("t1").is_none());
        assert_eq!(doc_label("t1"), "codeg-doc-t1");
    }

    #[test]
    fn content_types_by_extension() {
        assert_eq!(content_type(Path::new("A.HTML")), "text/html; charset=utf-8");
        assert_eq!(content_type(Path::new("x.mjs")), "text/javascript; charset=utf-8");
        assert_eq!(content_type(Path::new("x.woff2")), "font/woff2");
        assert_eq!(content_type(Path::new("x.bin")), "application/octet-stream");
        assert_eq!(content_type(Path::new("noext")), "application/octet-stream");
    }
}
