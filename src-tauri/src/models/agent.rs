use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    ClaudeCode,
    Codex,
    OpenCode,
    Gemini,
    Generic,
    Cline,
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentType::ClaudeCode => write!(f, "Claude Code"),
            AgentType::Codex => write!(f, "Codex CLI"),
            AgentType::OpenCode => write!(f, "OpenCode"),
            AgentType::Gemini => write!(f, "Gemini CLI"),
            AgentType::Generic => write!(f, "Generic"),
            AgentType::Cline => write!(f, "Cline"),
        }
    }
}

impl AgentType {
    pub const fn all() -> [AgentType; 6] {
        [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::OpenCode,
            AgentType::Gemini,
            AgentType::Generic,
            AgentType::Cline,
        ]
    }
}
