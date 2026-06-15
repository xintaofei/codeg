use std::collections::BTreeMap;

use crate::app_error::{AppCommandError, AppErrorCode};

/// Errors raised by the loop engine and its services. `Conflict` is the
/// compare-and-swap miss (concurrent state change) that the frontend retries.
#[derive(Debug, thiserror::Error)]
pub enum LoopError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("illegal loop state transition")]
    IllegalTransition,
    #[error("conflicting concurrent update")]
    Conflict,
    /// The issue is not in a state that can be merged (already terminal, blocked,
    /// cancelled, paused, or finalize has not produced a result). Distinct from
    /// `Conflict`: there is nothing transient to retry.
    #[error("issue is not in a mergeable state")]
    NotMergeable,
    #[error("loop space is detached from its folder")]
    Detached,
    #[error("folder is not a git repository")]
    NotGitRepo,
    #[error("merge conflict")]
    MergeConflict,
    #[error("git command failed: {0}")]
    Git(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid loop config: {0}")]
    InvalidConfig(String),
    #[error("acp error: {0}")]
    Acp(String),
    #[error(transparent)]
    Db(#[from] crate::db::error::DbError),
}

// The state machine (transitions.rs) runs raw sea_orm queries, while the
// service layer returns the wrapper `DbError`. Support `?` on both.
impl From<sea_orm::DbErr> for LoopError {
    fn from(e: sea_orm::DbErr) -> Self {
        LoopError::Db(crate::db::error::DbError::from(e))
    }
}

impl From<LoopError> for AppCommandError {
    fn from(e: LoopError) -> Self {
        match e {
            LoopError::NotFound(m) => AppCommandError::not_found(m),
            LoopError::IllegalTransition => {
                AppCommandError::new(AppErrorCode::InvalidInput, "Illegal loop state transition")
            }
            // Surfaced as a retryable conflict (HTTP 409 via TurnInProgress); the
            // frontend renders the localized `Loops.conflictRetry` toast.
            LoopError::Conflict => AppCommandError::new(
                AppErrorCode::TurnInProgress,
                "Loop state changed concurrently; retry",
            )
            .with_i18n("Loops.conflictRetry", BTreeMap::new()),
            // A non-retryable "can't do that" — the issue is already terminal /
            // blocked / not finalized. `InvalidInput` (not `TurnInProgress`) so the
            // frontend renders a plain failure, never the "retry" toast.
            LoopError::NotMergeable => AppCommandError::new(
                AppErrorCode::InvalidInput,
                "This issue is no longer awaiting merge",
            ),
            LoopError::Detached => AppCommandError::new(
                AppErrorCode::InvalidInput,
                "Loop space is detached from its folder",
            ),
            LoopError::NotGitRepo => {
                AppCommandError::not_a_git_repository("Loop space folder is not a git repository")
            }
            LoopError::MergeConflict => AppCommandError::new(
                AppErrorCode::ExternalCommandFailed,
                "Merge conflict while integrating the issue branch",
            ),
            LoopError::Git(m) => {
                AppCommandError::new(AppErrorCode::ExternalCommandFailed, "Git command failed")
                    .with_detail(m)
            }
            LoopError::InvalidInput(m) => AppCommandError::invalid_input(m),
            LoopError::InvalidConfig(m) => AppCommandError::new(
                AppErrorCode::InvalidInput,
                "Invalid loop config",
            )
            .with_detail(m),
            LoopError::Acp(m) => AppCommandError::task_execution_failed(m),
            LoopError::Db(err) => {
                AppCommandError::database_error("Database operation failed").with_detail(err.to_string())
            }
        }
    }
}
