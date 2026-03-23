use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("LSP server not found in registry: {0}")]
    NotFound(String),
    #[error("installation failed: {0}")]
    InstallFailed(String),
    #[error("prerequisite missing: {0}")]
    PrerequisiteMissing(String),
    #[error("platform not supported: {0}")]
    PlatformNotSupported(String),
    #[error("binary download failed: {0}")]
    DownloadFailed(String),
    #[error("database error: {0}")]
    Database(String),
}

impl Serialize for LspError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<crate::db::error::DbError> for LspError {
    fn from(e: crate::db::error::DbError) -> Self {
        LspError::Database(e.to_string())
    }
}
