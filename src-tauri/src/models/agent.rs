use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    ClaudeCode,
    Codex,
    OpenCode,
    Gemini,
    OpenClaw,
    Cline,
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentType::ClaudeCode => write!(f, "Claude Code"),
            AgentType::Codex => write!(f, "Codex CLI"),
            AgentType::OpenCode => write!(f, "OpenCode"),
            AgentType::Gemini => write!(f, "Gemini CLI"),
            AgentType::OpenClaw => write!(f, "OpenClaw"),
            AgentType::Cline => write!(f, "Cline"),
        }
    }
}
