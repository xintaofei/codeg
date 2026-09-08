//! Downloads started by a browser tab.
//!
//! P1 refused every download outright (no destination policy, no UI). Here the
//! host takes the decision the engine offers it: where the file lands, and
//! that nothing is ever overwritten or opened afterwards. The engine does the
//! transfer; we only choose the path, remember the record and tell the
//! frontend, which shows a bar with "show in folder".
//!
//! Two platform facts shape this module:
//! - The name in the engine's suggested destination comes from the SERVER
//!   (`Content-Disposition`) — untrusted. Only its last component is used, and
//!   only after `safe_file_name` has stripped anything that could climb out of
//!   the downloads directory.
//! - On macOS the finished callback carries no path at all (wry / WebKit API
//!   limitation), so the destination chosen at request time is remembered here
//!   and matched back by URL when the transfer ends.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::events;

pub const DOWNLOAD_EVENT: &str = "browser://download";

/// How many finished records are kept for the UI; the bar shows the last few
/// and a download is a file on disk afterwards, not a thing to scroll back to.
const HISTORY_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DownloadState {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDownload {
    pub id: String,
    /// Tab the download started in; the bar shows it there.
    pub tab_id: String,
    pub url: String,
    pub file_name: String,
    /// Absolute path the engine was told to write to.
    pub path: String,
    pub state: DownloadState,
}

#[derive(Default)]
pub struct BrowserDownloads {
    entries: Mutex<Vec<BrowserDownload>>,
}

static DOWNLOAD_SEQ: AtomicU64 = AtomicU64::new(0);

impl BrowserDownloads {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<BrowserDownload>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn push(&self, download: BrowserDownload) {
        let mut entries = self.lock();
        entries.push(download);
        if entries.len() > HISTORY_LIMIT {
            let overflow = entries.len() - HISTORY_LIMIT;
            entries.drain(0..overflow);
        }
    }

    /// Finish the oldest in-flight download of `url` (the only identity the
    /// completion callback carries) and return the updated record.
    fn finish(&self, url: &str, success: bool, path: Option<PathBuf>) -> Option<BrowserDownload> {
        let mut entries = self.lock();
        let entry = entries
            .iter_mut()
            .find(|d| d.url == url && d.state == DownloadState::Started)?;
        entry.state = if success {
            DownloadState::Completed
        } else {
            DownloadState::Failed
        };
        // Windows / Linux report where the file actually landed; macOS does
        // not, and keeps the path chosen when the download was requested.
        if let Some(path) = path {
            if success && !path.as_os_str().is_empty() {
                entry.file_name = file_name_of(&path);
                entry.path = path.to_string_lossy().to_string();
            }
        }
        Some(entry.clone())
    }

    pub fn list(&self) -> Vec<BrowserDownload> {
        self.lock().clone()
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Where downloads land: the OS download folder, else `~/Downloads`. Not
/// configurable in this pass, and deliberately NOT inside the app's data
/// directory — a downloaded file belongs to the user, not to codeg.
pub fn downloads_dir() -> PathBuf {
    if let Some(dir) = dirs::download_dir() {
        return dir;
    }
    dirs::home_dir()
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The server-suggested name, reduced to something that can only ever name a
/// file directly inside the downloads directory. Separators, parent hops,
/// NUL / control characters and leading dots are all removed; an empty or
/// hopeless name becomes `download`.
pub fn safe_file_name(suggested: &str) -> String {
    // Both separators on every platform: a Windows-style name arriving on
    // macOS must not become one file called `..\\..\\x`.
    let last = suggested
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(suggested)
        .trim();
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '\0'))
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() || cleaned == ".." {
        return "download".to_string();
    }
    // Long names are a filesystem error, not a security problem; keep the
    // extension by trimming the stem.
    const MAX: usize = 120;
    if cleaned.chars().count() <= MAX {
        return cleaned.to_string();
    }
    let path = Path::new(cleaned);
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let stem: String = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
        .chars()
        .take(MAX.saturating_sub(ext.chars().count()))
        .collect();
    format!("{stem}{ext}")
}

/// `dir/name`, with ` (1)`, ` (2)`… appended before the extension until the
/// path is free. A download NEVER replaces a file that is already there.
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for counter in 1..10_000 {
        let candidate = dir.join(format!("{stem} ({counter}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological directory; a timestamp is still unique enough to write to.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    dir.join(format!("{stem} ({stamp}){ext}"))
}

/// A download was requested in `tab_id`. Rewrites `destination` to a free path
/// under the downloads directory and records the download. Returns false when
/// the directory cannot be created — the engine then cancels, which is the
/// honest outcome (there is nowhere to write).
pub fn requested(app: &AppHandle, tab_id: &str, url: &str, destination: &mut PathBuf) -> bool {
    let dir = downloads_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            "[browser] refusing the download of {url}: {} is not writable ({err})",
            dir.display()
        );
        return false;
    }
    // The engine already suggested a name (and on macOS a whole path); only
    // its last component is used, and only after sanitising.
    let file_name = safe_file_name(&file_name_of(destination));
    let path = unique_path(&dir, &file_name);
    *destination = path.clone();

    let seq = DOWNLOAD_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let download = BrowserDownload {
        id: format!("dl-{seq}"),
        tab_id: tab_id.to_string(),
        url: url.to_string(),
        file_name: file_name_of(&path),
        path: path.to_string_lossy().to_string(),
        state: DownloadState::Started,
    };
    if let Some(downloads) = app.try_state::<BrowserDownloads>() {
        downloads.push(download.clone());
    }
    // A click that turns into a download leaves the tab's navigation
    // unfinished for ever; tell the tab so its load watcher stands down.
    super::hooks::navigation_became_download(app, tab_id);
    events::emit_download(app, &download);
    true
}

/// The engine finished (or gave up on) a download.
pub fn finished(app: &AppHandle, url: &str, path: Option<PathBuf>, success: bool) {
    let Some(downloads) = app.try_state::<BrowserDownloads>() else {
        return;
    };
    if let Some(download) = downloads.finish(url, success, path) {
        events::emit_download(app, &download);
    }
}

/// Where a tab's downloads go, for the settings section.
pub fn downloads_dir_display() -> String {
    downloads_dir().to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggested_name_can_only_name_a_file_in_the_directory() {
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
        // Path traversal in every shape the server can send.
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("..\\..\\Windows\\system.ini"), "system.ini");
        assert_eq!(safe_file_name("/absolute/evil.sh"), "evil.sh");
        assert_eq!(safe_file_name(".."), "download");
        assert_eq!(safe_file_name("."), "download");
        assert_eq!(safe_file_name(""), "download");
        assert_eq!(safe_file_name("   "), "download");
        // A leading dot would make the file invisible; drive letters and NUL
        // cannot survive either.
        assert_eq!(safe_file_name(".bashrc"), "bashrc");
        assert_eq!(safe_file_name("C:\\x\\y.txt"), "y.txt");
        assert_eq!(safe_file_name("a\u{0}b.txt"), "ab.txt");
        assert_eq!(safe_file_name("line\nbreak.txt"), "linebreak.txt");
    }

    #[test]
    fn a_long_name_keeps_its_extension() {
        let name = safe_file_name(&format!("{}.tar.gz", "x".repeat(400)));
        assert!(name.chars().count() <= 120, "{name}");
        assert!(name.ends_with(".gz"), "{name}");
    }

    #[test]
    fn a_download_never_replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = unique_path(dir.path(), "report.pdf");
        assert_eq!(first, dir.path().join("report.pdf"));
        std::fs::write(&first, b"one").unwrap();

        let second = unique_path(dir.path(), "report.pdf");
        assert_eq!(second, dir.path().join("report (1).pdf"));
        std::fs::write(&second, b"two").unwrap();

        assert_eq!(
            unique_path(dir.path(), "report.pdf"),
            dir.path().join("report (2).pdf")
        );
        // The first file is untouched.
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
    }

    #[test]
    fn an_extensionless_name_is_numbered_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("LICENSE"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "LICENSE"),
            dir.path().join("LICENSE (1)")
        );
    }

    #[test]
    fn the_registry_finishes_the_matching_download_and_caps_its_history() {
        let downloads = BrowserDownloads::default();
        let record = |id: &str, url: &str| BrowserDownload {
            id: id.into(),
            tab_id: "t1".into(),
            url: url.into(),
            file_name: "a.bin".into(),
            path: "/tmp/a.bin".into(),
            state: DownloadState::Started,
        };
        downloads.push(record("dl-1", "https://example.com/a"));
        downloads.push(record("dl-2", "https://example.com/b"));
        // Same URL twice: the oldest still running finishes first.
        downloads.push(record("dl-3", "https://example.com/a"));

        let done = downloads
            .finish("https://example.com/a", true, Some(PathBuf::from("/tmp/z.bin")))
            .unwrap();
        assert_eq!(done.id, "dl-1");
        assert_eq!(done.state, DownloadState::Completed);
        assert_eq!(done.file_name, "z.bin");
        let again = downloads.finish("https://example.com/a", false, None).unwrap();
        assert_eq!(again.id, "dl-3");
        assert_eq!(again.state, DownloadState::Failed);
        assert!(downloads.finish("https://example.com/a", true, None).is_none());
        // A failed download keeps the path it was going to be written to.
        assert_eq!(again.path, "/tmp/a.bin");

        for i in 0..HISTORY_LIMIT + 5 {
            downloads.push(record(&format!("x-{i}"), "https://example.com/c"));
        }
        assert_eq!(downloads.list().len(), HISTORY_LIMIT);
        downloads.clear();
        assert!(downloads.list().is_empty());
    }

    #[test]
    fn wire_names_are_camel_and_kebab() {
        let json = serde_json::to_value(BrowserDownload {
            id: "dl-1".into(),
            tab_id: "t1".into(),
            url: "https://example.com/a.bin".into(),
            file_name: "a.bin".into(),
            path: "/tmp/a.bin".into(),
            state: DownloadState::Started,
        })
        .unwrap();
        assert_eq!(json["tabId"], "t1");
        assert_eq!(json["fileName"], "a.bin");
        assert_eq!(json["state"], "started");
    }
}
