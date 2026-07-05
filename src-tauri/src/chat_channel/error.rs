use crate::app_error::AppCommandError;

#[derive(Debug, thiserror::Error)]
pub enum ChatChannelError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("send failed: {0}")]
    SendFailed(String),
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("configuration invalid: {0}")]
    ConfigurationInvalid(String),
    #[error("not connected")]
    NotConnected,
    #[error("already connected")]
    AlreadyConnected,
    #[error("channel not found: {0}")]
    NotFound(i32),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Other(String),
}

impl From<ChatChannelError> for AppCommandError {
    fn from(err: ChatChannelError) -> Self {
        match &err {
            ChatChannelError::NotFound(_) => AppCommandError::not_found(err.to_string()),
            ChatChannelError::AuthenticationFailed(_) => {
                AppCommandError::authentication_failed(err.to_string())
            }
            ChatChannelError::ConfigurationInvalid(_) => {
                AppCommandError::configuration_invalid(err.to_string())
            }
            ChatChannelError::ConnectionFailed(_) | ChatChannelError::SendFailed(_) => {
                AppCommandError::network(err.to_string())
            }
            _ => AppCommandError::task_execution_failed(err.to_string()),
        }
    }
}
