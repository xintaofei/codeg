use crate::models::agent::AgentType;

#[derive(Debug, Clone)]
pub enum AgentDistribution {
    Npx {
        version: &'static str,
        package: &'static str,
        /// The command name provided by this npx package (e.g. "gemini", "openclaw").
        cmd: &'static str,
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        /// Minimum Node.js version required, e.g. "22.12.0". None means no specific requirement.
        node_required: Option<&'static str>,
    },
    Binary {
        version: &'static str,
        cmd: &'static str,
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        platforms: &'static [PlatformBinary],
    },
}

#[derive(Debug, Clone)]
pub struct PlatformBinary {
    pub platform: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone)]
pub struct AcpAgentMeta {
    pub agent_type: AgentType,
    pub name: &'static str,
    pub description: &'static str,
    pub distribution: AgentDistribution,
}

impl AcpAgentMeta {
    pub fn registry_version(&self) -> Option<&'static str> {
        match &self.distribution {
            AgentDistribution::Npx { version, .. } | AgentDistribution::Binary { version, .. } => {
                Some(*version)
            }
        }
    }
}

pub fn current_platform() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "darwin-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "darwin-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "linux-aarch64"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux-x86_64"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "windows-aarch64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "windows-x86_64"
    }
}

pub fn all_acp_agents() -> Vec<AgentType> {
    vec![
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::Gemini,
        AgentType::OpenClaw,
        AgentType::OpenCode,
        AgentType::Cline,
        AgentType::Grok,
    ]
}

pub fn registry_id_for(agent_type: AgentType) -> &'static str {
    match agent_type {
        AgentType::ClaudeCode => "claude-acp",
        AgentType::Codex => "codex-acp",
        AgentType::Gemini => "gemini",
        AgentType::OpenClaw => "openclaw-acp",
        AgentType::OpenCode => "opencode",
        AgentType::Cline => "cline",
        AgentType::Grok => "grok",
    }
}

pub fn from_registry_id(id: &str) -> Option<AgentType> {
    match id {
        "claude-acp" => Some(AgentType::ClaudeCode),
        "codex-acp" => Some(AgentType::Codex),
        "gemini" => Some(AgentType::Gemini),
        "openclaw-acp" => Some(AgentType::OpenClaw),
        "opencode" => Some(AgentType::OpenCode),
        "cline" => Some(AgentType::Cline),
        "grok" => Some(AgentType::Grok),
        _ => None,
    }
}

pub fn get_agent_meta(agent_type: AgentType) -> AcpAgentMeta {
    debug_assert_eq!(
        from_registry_id(registry_id_for(agent_type)),
        Some(agent_type)
    );
    match agent_type {
        AgentType::ClaudeCode => AcpAgentMeta {
            agent_type,
            name: "Claude Code",
            description: "ACP wrapper for Anthropic's Claude",
            distribution: AgentDistribution::Npx {
                version: "0.33.1",
                package: "@agentclientprotocol/claude-agent-acp@0.33.1",
                cmd: "claude-agent-acp",
                args: &[],
                env: &[],
                node_required: None,
            },
        },
        AgentType::Codex => AcpAgentMeta {
            agent_type,
            name: "Codex CLI",
            description: "ACP adapter for OpenAI's coding assistant",
            distribution: AgentDistribution::Binary {
                version: "0.14.0",
                cmd: "codex-acp",
                args: &[],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.14.0/codex-acp-0.14.0-aarch64-apple-darwin.tar.gz",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.14.0/codex-acp-0.14.0-x86_64-apple-darwin.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.14.0/codex-acp-0.14.0-aarch64-unknown-linux-gnu.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.14.0/codex-acp-0.14.0-x86_64-unknown-linux-gnu.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-aarch64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.14.0/codex-acp-0.14.0-aarch64-pc-windows-msvc.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/zed-industries/codex-acp/releases/download/v0.14.0/codex-acp-0.14.0-x86_64-pc-windows-msvc.zip",
                    },
                ],
            },
        },
        AgentType::Gemini => AcpAgentMeta {
            agent_type,
            name: "Gemini CLI",
            description: "Google's official CLI for Gemini",
            distribution: AgentDistribution::Npx {
                version: "0.42.0",
                package: "@google/gemini-cli@0.42.0",
                cmd: "gemini",
                args: &["--acp", "--skip-trust"],
                env: &[],
                node_required: None,
            },
        },
        AgentType::OpenClaw => AcpAgentMeta {
            agent_type,
            name: "OpenClaw",
            description: "OpenClaw is a personal AI assistant you run on your own devices.",
            distribution: AgentDistribution::Npx {
                version: "2026.5.6",
                package: "openclaw@2026.5.6",
                cmd: "openclaw",
                args: &["acp"],
                env: &[],
                node_required: Some("22.12.0"),
            },
        },
        AgentType::Cline => AcpAgentMeta {
            agent_type,
            name: "Cline",
            description: "Autonomous coding agent CLI",
            distribution: AgentDistribution::Npx {
                version: "2.18.0",
                package: "cline@2.18.0",
                cmd: "cline",
                args: &["--acp"],
                env: &[],
                node_required: None,
            },
        },
        AgentType::OpenCode => AcpAgentMeta {
            agent_type,
            name: "OpenCode",
            description: "The open source coding agent",
            distribution: AgentDistribution::Binary {
                version: "1.14.50",
                cmd: "opencode",
                args: &["acp"],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.14.50/opencode-darwin-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.14.50/opencode-darwin-x64.zip",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.14.50/opencode-linux-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.14.50/opencode-linux-x64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.14.50/opencode-windows-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.14.50/opencode-windows-x64.zip",
                    },
                ],
            },
        },
        AgentType::Grok => AcpAgentMeta {
            agent_type,
            name: "Grok",
            description: "xAI's coding agent CLI",
            distribution: AgentDistribution::Binary {
                version: "0.1.210",
                cmd: "grok",
                args: &["agent", "stdio"],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://storage.googleapis.com/grok-build-public-artifacts/cli/grok-0.1.210-macos-aarch64",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://storage.googleapis.com/grok-build-public-artifacts/cli/grok-0.1.210-macos-x86_64",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://storage.googleapis.com/grok-build-public-artifacts/cli/grok-0.1.210-linux-aarch64",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://storage.googleapis.com/grok-build-public-artifacts/cli/grok-0.1.210-linux-x86_64",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://storage.googleapis.com/grok-build-public-artifacts/cli/grok-0.1.210-windows-x86_64.exe",
                    },
                ],
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_is_registered_as_binary_stdio_agent() {
        assert!(all_acp_agents().contains(&AgentType::Grok));
        assert_eq!(registry_id_for(AgentType::Grok), "grok");
        assert_eq!(from_registry_id("grok"), Some(AgentType::Grok));

        let meta = get_agent_meta(AgentType::Grok);
        assert_eq!(meta.name, "Grok");
        assert_eq!(meta.registry_version(), Some("0.1.210"));

        match meta.distribution {
            AgentDistribution::Binary {
                cmd,
                args,
                env,
                platforms,
                ..
            } => {
                assert_eq!(cmd, "grok");
                assert_eq!(args, &["agent", "stdio"]);
                assert!(env.is_empty());
                assert!(platforms.iter().any(|p| p.platform == "darwin-aarch64"));
            }
            other => panic!("expected binary distribution, got {other:?}"),
        }
    }
}
