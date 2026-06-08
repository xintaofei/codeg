use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::db::error::DbError;

// ─── Shared i18n keys ─────────────────────────────────────────────────
//
// The wire-format strings that backend errors stamp via `with_i18n` and
// the frontend branches on via `extractAppCommandError(err).i18n_key`.
// They live here — not in any individual command module — because the
// same key can be emitted from multiple Rust sites (e.g.
// `errors.upload.tooLarge` comes from `commands/remote_proxy.rs` AND
// from `web/handlers/files.rs`) and consumed by a single TS branch.
//
// **MUST stay in lockstep with the TypeScript constants** in
// `src/lib/api.ts` (`UPLOAD_I18N_KEY_TOO_LARGE` /
// `UPLOAD_I18N_KEY_NOT_A_FILE`). The unit test in
// `commands/remote_proxy.rs::tests::upload_i18n_keys_have_expected_values`
// asserts the literal values on the Rust side so an accidental rename
// becomes a loud CI failure rather than a silent demotion to the
// generic "upload failed" toast.

/// Error key emitted when an upload payload exceeds `UPLOAD_MAX_BYTES`,
/// at any of three layers (local pre-read, base64 pre-decode, post-decode).
/// Frontend params: `size`, `limit`.
pub const UPLOAD_I18N_KEY_TOO_LARGE: &str = "errors.upload.tooLarge";

/// Error key emitted when `read_local_file_for_upload` is handed a path
/// that resolves to a directory, FIFO, device node, or other non-regular
/// file. No params.
///
/// Only `commands/remote_proxy.rs` emits this today (the command is gated
/// on `feature = "tauri-runtime"`), so the server-only build won't see a
/// use site. The constant still has to exist there because it is part of
/// the wire-format contract the frontend depends on, hence `allow(dead_code)`.
#[allow(dead_code)]
pub const UPLOAD_I18N_KEY_NOT_A_FILE: &str = "errors.upload.notAFile";

/// Error key emitted when accepting one more upload would push the
/// `uploads_root/` directory past `CODEG_UPLOAD_MAX_TOTAL_BYTES`. The
/// per-file 2 MiB cap protects against one big payload; this cap
/// protects against an attacker accumulating many small ones.
/// Frontend params: `used`, `limit` (both byte counts as strings).
pub const UPLOAD_I18N_KEY_QUOTA_EXCEEDED: &str = "errors.upload.quotaExceeded";

// ─── Backup / restore i18n keys ──────────────────────────────────────
//
// Emitted by `commands::backup::*` and consumed by `BackupSettings` on the
// frontend. MUST stay in lockstep with the TS constants in
// `src/lib/api.ts`. No-param keys unless noted.

/// Decryption failed the GCM tag — wrong passphrase or a tampered/corrupt
/// archive (the two are cryptographically indistinguishable).
pub const BACKUP_I18N_KEY_BAD_PASSPHRASE: &str = "backup.restore.error.badPassphrase";
/// A backup entry's bytes did not match the manifest checksum.
pub const BACKUP_I18N_KEY_CORRUPTED: &str = "backup.restore.error.corrupted";
/// The file is not a codeg backup, or its `format_version` is newer than this
/// binary understands.
pub const BACKUP_I18N_KEY_UNKNOWN_FORMAT: &str = "backup.restore.error.unknownFormat";
/// The backup was taken by a newer app version whose DB schema this binary
/// cannot represent. Params: `backupVersion`, `appVersion`.
pub const BACKUP_I18N_KEY_NEWER_VERSION: &str = "backup.restore.error.newerVersion";
/// Ran out of disk space while writing the archive / staging a restore.
pub const BACKUP_I18N_KEY_DISK_SPACE: &str = "backup.error.diskSpace";
/// The operation was cancelled by the user.
pub const BACKUP_I18N_KEY_CANCELLED: &str = "backup.error.cancelled";
/// A restore is already staged and awaiting restart; only one at a time.
pub const BACKUP_I18N_KEY_ALREADY_PENDING: &str = "backup.restore.error.alreadyPending";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppErrorCode {
    InvalidInput,
    ConfigurationMissing,
    ConfigurationInvalid,
    NotFound,
    NotAGitRepository,
    AlreadyExists,
    PermissionDenied,
    DependencyMissing,
    NetworkError,
    AuthenticationFailed,
    DatabaseError,
    IoError,
    ExternalCommandFailed,
    WindowOperationFailed,
    TaskExecutionFailed,
    /// A prompt was rejected because a turn is already in flight on the
    /// connection (a second, concurrent send). Maps to HTTP 409 — an expected,
    /// recoverable condition in multi-client co-control, not a server fault.
    TurnInProgress,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct AppCommandError {
    pub code: AppErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional dotted i18n key (e.g. `"mcp.errors.unsupportedType"`) the
    /// frontend can use to render a localized message. When absent, the
    /// frontend falls back to `message` (English).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i18n_key: Option<String>,
    /// Optional named parameters substituted into the localized template.
    /// All values are pre-stringified so the wire format stays JSON-safe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i18n_params: Option<BTreeMap<String, String>>,
}

impl AppCommandError {
    pub fn new(code: AppErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            i18n_key: None,
            i18n_params: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Attach a localized rendering hint. The frontend prefers this over
    /// `message` when displaying the error to the user. `params` may be empty.
    pub fn with_i18n(mut self, key: impl Into<String>, params: BTreeMap<String, String>) -> Self {
        self.i18n_key = Some(key.into());
        if !params.is_empty() {
            self.i18n_params = Some(params);
        }
        self
    }

    pub fn db(err: DbError) -> Self {
        Self::new(AppErrorCode::DatabaseError, "Database operation failed")
            .with_detail(err.to_string())
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::InvalidInput, message)
    }

    pub fn configuration_missing(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::ConfigurationMissing, message)
    }

    pub fn configuration_invalid(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::ConfigurationInvalid, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::NotFound, message)
    }

    pub fn not_a_git_repository(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::NotAGitRepository, message)
    }

    pub fn already_exists(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::AlreadyExists, message)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::PermissionDenied, message)
    }

    pub fn dependency_missing(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::DependencyMissing, message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::NetworkError, message)
    }

    pub fn authentication_failed(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::AuthenticationFailed, message)
    }

    pub fn database_error(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::DatabaseError, message)
    }

    pub fn io_error(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::IoError, message)
    }

    pub fn task_execution_failed(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::TaskExecutionFailed, message)
    }

    pub fn io(err: std::io::Error) -> Self {
        let code = match err.kind() {
            std::io::ErrorKind::NotFound => AppErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied => AppErrorCode::PermissionDenied,
            std::io::ErrorKind::AlreadyExists => AppErrorCode::AlreadyExists,
            _ => AppErrorCode::IoError,
        };

        let message = match code {
            AppErrorCode::NotFound => "Resource not found",
            AppErrorCode::PermissionDenied => "Permission denied",
            AppErrorCode::AlreadyExists => "Resource already exists",
            _ => "I/O operation failed",
        };

        Self::new(code, message).with_detail(err.to_string())
    }

    pub fn window(message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(AppErrorCode::WindowOperationFailed, message).with_detail(detail)
    }

    pub fn external_command(message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(AppErrorCode::ExternalCommandFailed, message).with_detail(detail)
    }
}

impl From<DbError> for AppCommandError {
    fn from(value: DbError) -> Self {
        Self::db(value)
    }
}
