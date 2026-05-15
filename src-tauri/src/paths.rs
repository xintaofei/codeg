//! Centralized resolution of codeg-owned filesystem paths.
//!
//! Mirrors the conventions already used by `preferences.rs` (`~/.codeg/`)
//! and `experts.rs` (`~/.codeg/skills/`). New features that need a
//! user-scoped persistent directory should call into this module instead of
//! re-deriving `dirs::home_dir().join(".codeg")` themselves.

use std::path::PathBuf;

const CODEG_DIR_NAME: &str = ".codeg";
const PETS_DIR_NAME: &str = "pets";
const UPLOADS_DIR_NAME: &str = "uploads";

/// `$CODEG_HOME` if set (and non-empty), else `~/.codeg/`.
///
/// Returns the relative `.codeg` path when no home directory is available;
/// callers must still handle creation failures gracefully.
pub fn codeg_home_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os("CODEG_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(custom);
    }
    dirs::home_dir()
        .map(|h| h.join(CODEG_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(CODEG_DIR_NAME))
}

/// Root directory for desktop-pet assets.
///
/// Resolution order:
/// 1. `$CODEG_HOME/pets` (explicit override, used in tests and custom installs)
/// 2. `$CODEG_DATA_DIR/pets` (server-mode data directory, populated by
///    `codeg-server` from the corresponding env var)
/// 3. `~/.codeg/pets` (default for the desktop app)
pub fn codeg_pets_root() -> PathBuf {
    if let Some(custom) = std::env::var_os("CODEG_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(custom).join(PETS_DIR_NAME);
    }
    if let Some(data) = std::env::var_os("CODEG_DATA_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(data).join(PETS_DIR_NAME);
    }
    dirs::home_dir()
        .map(|h| h.join(CODEG_DIR_NAME).join(PETS_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(CODEG_DIR_NAME).join(PETS_DIR_NAME))
}

/// Root directory for attachments uploaded from the web client.
///
/// Resolution order matches `codeg_pets_root()`:
/// 1. `$CODEG_HOME/uploads`
/// 2. `$CODEG_DATA_DIR/uploads` (server-mode data directory)
/// 3. `~/.codeg/uploads` (desktop default)
///
/// Files in this directory are not garbage-collected by codeg itself —
/// later conversations may still reference them via `file://` URIs
/// embedded in session history. To bound the long-term footprint on
/// shared / multi-tenant servers, operators can set
/// `CODEG_UPLOAD_MAX_TOTAL_BYTES` (see `web::handlers::files`): new
/// uploads beyond the cap are rejected at the API boundary while
/// existing files stay readable.
pub fn codeg_uploads_root() -> PathBuf {
    if let Some(custom) = std::env::var_os("CODEG_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(custom).join(UPLOADS_DIR_NAME);
    }
    if let Some(data) = std::env::var_os("CODEG_DATA_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(data).join(UPLOADS_DIR_NAME);
    }
    dirs::home_dir()
        .map(|h| h.join(CODEG_DIR_NAME).join(UPLOADS_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(CODEG_DIR_NAME).join(UPLOADS_DIR_NAME))
}

// Path resolution depends on global env vars (`CODEG_HOME`, `CODEG_DATA_DIR`),
// so unit tests would need cross-test serialization to avoid races. The
// behaviour is covered end-to-end by `pets::*` tests which set `CODEG_HOME`
// inside a serialized test mutex; we deliberately don't duplicate that here.
