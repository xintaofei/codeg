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
        /// Command name on PATH (fallback launch + `which` probes). For
        /// single-file archives this is also the file name copied out of the
        /// archive into the cache.
        cmd: &'static str,
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        platforms: &'static [PlatformBinary],
        /// `None`: the archive contains one self-contained binary named `cmd`,
        /// which is copied out into the cache (OpenCode). `Some`: the archive
        /// is a whole directory tree that must stay intact (bundled runtime,
        /// e.g. Cursor's agent-cli-package); everything is extracted into the
        /// per-version cache dir and the entry path inside it is launched.
        dir_entry: Option<BinaryDirEntry>,
    },
    /// Python agents launched through `uvx` (the `uv` tool runner), which
    /// fetches + caches the pinned package on first use — analogous to npx.
    /// Used for ACP agents distributed as Python packages (e.g. Hermes).
    Uvx {
        version: &'static str,
        /// The `uvx --from` package spec, e.g. "hermes-agent[acp,mcp]==0.19.0".
        package: &'static str,
        /// The console-script entry point to run, e.g. "hermes-acp".
        cmd: &'static str,
        args: &'static [&'static str],
        env: &'static [(&'static str, &'static str)],
        /// Minimum `uv` version required, e.g. "0.5.0". None means no specific requirement.
        uv_required: Option<&'static str>,
        /// Interpreter to pin via `uvx --python <ver>`, e.g. `Some("3.13")`.
        /// `None` lets uvx pick its default interpreter. Set this when the
        /// package (or a transitive dep) does not support the machine's default
        /// Python — uv auto-downloads a managed build of the pinned version.
        python: Option<&'static str>,
        /// Fallback command resolvable on PATH when `uvx` is unavailable, e.g.
        /// `Some(("hermes", &["acp"]))` — lets users who installed the agent via
        /// its official installer launch it without `uv`.
        system_cmd: Option<(&'static str, &'static [&'static str])>,
    },
}

#[derive(Debug, Clone)]
pub struct PlatformBinary {
    pub platform: &'static str,
    pub url: &'static str,
}

/// Launch entry inside an extracted directory-tree archive (see
/// [`AgentDistribution::Binary::dir_entry`]). Paths are relative to the
/// archive root, '/'-separated; `windows` names the `.cmd`/`.bat` shim.
#[derive(Debug, Clone, Copy)]
pub struct BinaryDirEntry {
    pub unix: &'static str,
    pub windows: &'static str,
}

impl BinaryDirEntry {
    /// Entry path for the current platform.
    pub fn for_current_platform(&self) -> &'static str {
        if cfg!(windows) {
            self.windows
        } else {
            self.unix
        }
    }
}

#[derive(Debug, Clone)]
pub struct AcpAgentMeta {
    pub agent_type: AgentType,
    /// 是否经 ACP 线缆（session/new 的 `mcpServers` 字段）向该 agent 转发 MCP
    /// 服务器——既包括用户配置的服务器，也包括内置 codeg-mcp 伴生进程。
    /// OpenClaw 拒绝 `mcpServers` 中的任何服务器条目（会使 session/new 失败），
    /// 故置 false。注意空列表 `[]` 仍会按 ACP schema 序列化、OpenClaw 可接受——
    /// 闸门只是保证该列表对 OpenClaw 恒为空（不含任何条目）。
    pub supports_mcp: bool,
    pub name: &'static str,
    pub description: &'static str,
    pub distribution: AgentDistribution,
}

impl AcpAgentMeta {
    pub fn registry_version(&self) -> Option<&'static str> {
        match &self.distribution {
            AgentDistribution::Npx { version, .. }
            | AgentDistribution::Binary { version, .. }
            | AgentDistribution::Uvx { version, .. } => Some(*version),
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
        AgentType::Hermes,
        AgentType::CodeBuddy,
        AgentType::KimiCode,
        AgentType::Pi,
        AgentType::Grok,
        AgentType::Cursor,
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
        AgentType::Hermes => "hermes",
        AgentType::CodeBuddy => "codebuddy-code",
        AgentType::KimiCode => "kimi-code",
        AgentType::Pi => "pi-acp",
        AgentType::Grok => "grok-build",
        AgentType::Cursor => "cursor",
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
        "hermes" => Some(AgentType::Hermes),
        "codebuddy-code" => Some(AgentType::CodeBuddy),
        "kimi-code" => Some(AgentType::KimiCode),
        "pi-acp" => Some(AgentType::Pi),
        "grok-build" => Some(AgentType::Grok),
        "cursor" => Some(AgentType::Cursor),
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
            supports_mcp: true,
            name: "Claude Code",
            description: "ACP wrapper for Anthropic's Claude",
            distribution: AgentDistribution::Npx {
                version: "0.61.0",
                package: "@agentclientprotocol/claude-agent-acp@0.61.0",
                cmd: "claude-agent-acp",
                args: &[],
                env: &[],
                node_required: Some("22.0.0"),
            },
        },
        AgentType::Codex => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Codex CLI",
            description: "ACP adapter for OpenAI's coding assistant",
            // codex-acp moved from zed-industries (Rust binary) to the
            // agentclientprotocol org (TypeScript rewrite, npx-distributed).
            // 1.1.7 depends on `@openai/codex` ^0.145.0 and drives `codex
            // app-server`; since 1.0.1 it also resolves the resumed
            // `model_provider` from `~/.codex/config.toml` (#224), so codeg no
            // longer injects `MODEL_PROVIDER` to keep resumed sessions on the
            // custom provider. 1.1.0 (#263) reports `/goal` transitions as a
            // structured `session_info_update` (`_meta.codex.goal`) rather than
            // live agent text — see `crate::acp::codex_goal`. 1.1.3+ adds three
            // new live signals codeg handles in `connection::emit_conversation_update`:
            // `subAgentActivity` tool calls (#304, suppressed via
            // `is_codex_subagent_activity` — redundant with the collab capsule),
            // retryable turn errors (#289, `_meta.codex.error` → a transient
            // retry banner via `codex_retry_indicator`), and the
            // context-compaction lifecycle (#288, `_meta.contextCompaction` tool
            // call → a dedicated frontend card). 1.1.x also adds Plan mode: the
            // `collaboration_mode` config option (rendered by the generic
            // config-option path) and native `request_user_input`, delivered as
            // an ACP `elicitation/create` request — codeg advertises
            // `elicitation.form` for Codex and bridges the WHOLE form surface
            // (Plan-mode questions, MCP-server forms, MCP tool-call approvals)
            // in `handle_elicitation_request` / `question::classify_elicitation`.
            // 1.1.5 (#322) also widened codex-acp's MCP config filtering to
            // project `.codex` layers, which is why codeg forces
            // `DISABLE_MCP_CONFIG_FILTERING` (see `apply_codex_env_policy`) so
            // the injected `codeg-mcp` server always survives. 1.1.6 adds
            // steering (#309): `_session/steering` injects a user prompt into
            // the LIVE turn (initialize advertises `_meta.steering.supported`)
            // — not wired into codeg yet. 1.1.7 (#326) emits Plan-mode plan
            // contents as a plain `agent_message_chunk`
            // (`_meta.codex.phase = "final_answer"`, no `<proposed_plan>` tags),
            // which the adapter's tag-splitter simply no-ops on — tagged output
            // from older codex still renders as the proposed-plan card. 1.1.7
            // declares no `engines.node`, so the 20.0.0 floor is retained.
            distribution: AgentDistribution::Npx {
                version: "1.1.7",
                package: "@agentclientprotocol/codex-acp@1.1.7",
                cmd: "codex-acp",
                args: &[],
                env: &[],
                node_required: Some("20.0.0"),
            },
        },
        AgentType::Gemini => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Gemini CLI",
            description: "Google's official CLI for Gemini",
            distribution: AgentDistribution::Npx {
                version: "0.52.0",
                package: "@google/gemini-cli@0.52.0",
                cmd: "gemini",
                args: &["--acp", "--skip-trust"],
                env: &[],
                node_required: Some("20.0.0"),
            },
        },
        AgentType::OpenClaw => AcpAgentMeta {
            agent_type,
            // OpenClaw 拒绝 `mcpServers` 中的任何服务器条目（会使 session/new 失败），
            // 故不向其转发任何 MCP 条目（含 codeg-mcp 伴生进程）。详见 supports_mcp 字段注释。
            supports_mcp: false,
            name: "OpenClaw",
            description: "OpenClaw is a personal AI assistant you run on your own devices.",
            distribution: AgentDistribution::Npx {
                version: "2026.7.1",
                package: "openclaw@2026.7.1",
                cmd: "openclaw",
                args: &["acp"],
                env: &[],
                node_required: Some("22.22.3"),
            },
        },
        AgentType::Cline => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Cline",
            description: "Autonomous coding agent CLI",
            distribution: AgentDistribution::Npx {
                version: "3.0.46",
                package: "cline@3.0.46",
                cmd: "cline",
                args: &["--acp"],
                env: &[],
                node_required: Some("22.0.0"),
            },
        },
        AgentType::OpenCode => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "OpenCode",
            description: "The open source coding agent",
            distribution: AgentDistribution::Binary {
                version: "1.18.4",
                cmd: "opencode",
                args: &["acp"],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.4/opencode-darwin-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.4/opencode-darwin-x64.zip",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.4/opencode-linux-arm64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.4/opencode-linux-x64.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-aarch64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.4/opencode-windows-arm64.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.4/opencode-windows-x64.zip",
                    },
                ],
                dir_entry: None,
            },
        },
        AgentType::Hermes => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Hermes Agent",
            description: "Nous Research's self-improving agent (ACP via uvx)",
            distribution: AgentDistribution::Uvx {
                version: "0.19.0",
                package: "hermes-agent[acp,mcp]==0.19.0",
                cmd: "hermes-acp",
                args: &[],
                env: &[],
                uv_required: Some("0.5.0"),
                // hermes-agent 0.19.0 is `requires-python = ">=3.11,<3.14"`, and
                // its win32 dep `pywinpty` (>=2.0.0,<3) has no Python 3.14 wheel
                // (the 2.0.15 source build fails against PyO3's 3.13 ceiling).
                // Without this pin uvx grabs the machine's default interpreter
                // (e.g. 3.14) and the install breaks; 3.13 is the newest version
                // Hermes supports.
                python: Some("3.13"),
                system_cmd: Some(("hermes", &["acp"])),
            },
        },
        AgentType::CodeBuddy => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "CodeBuddy",
            description: "Tencent Cloud's official AI coding assistant (ACP)",
            distribution: AgentDistribution::Npx {
                version: "2.126.0",
                package: "@tencent-ai/codebuddy-code@2.126.0",
                cmd: "codebuddy",
                args: &["--acp"],
                env: &[],
                node_required: Some("22.0.0"),
            },
        },
        AgentType::KimiCode => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Kimi Code",
            description: "Moonshot AI's official CLI coding assistant (ACP)",
            distribution: AgentDistribution::Npx {
                version: "0.29.0",
                package: "@moonshot-ai/kimi-code@0.29.0",
                cmd: "kimi",
                args: &["acp"],
                env: &[],
                node_required: Some("22.19.0"),
            },
        },
        AgentType::Pi => AcpAgentMeta {
            agent_type,
            // pi-acp accepts ACP-wire `mcpServers` but drops them (does not
            // forward to pi), and pi has no native MCP. supports_mcp stays
            // `true` only to satisfy the `only_openclaw_opts_out_of_mcp`
            // invariant — actual wire forwarding is short-circuited in
            // `connection.rs` (see the skip-list), so neither user servers nor
            // the codeg-mcp companion are futilely forwarded.
            supports_mcp: true,
            name: "Pi",
            description: "Self-extensible coding agent (ACP via pi-acp)",
            // pi-acp 0.0.31 spawns `pi --mode rpc` as a child, so `pi` (npm
            // `@earendil-works/pi-coding-agent`) must be resolvable on PATH —
            // or pointed at a custom build via the `PI_ACP_PI_COMMAND` env
            // (see BYO-pi). Args are empty: the ACP server is the default mode
            // (`npx -y pi-acp`, no subcommand). `node_required` follows pi's
            // 22+ requirement (pi-acp's own engines say >=20). The embedded
            // context env lets pi-acp advertise `promptCapabilities.embeddedContext`.
            distribution: AgentDistribution::Npx {
                version: "0.0.31",
                package: "pi-acp@0.0.31",
                cmd: "pi-acp",
                args: &[],
                env: &[("PI_ACP_ENABLE_EMBEDDED_CONTEXT", "true")],
                node_required: Some("22.0.0"),
            },
        },
        AgentType::Grok => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Grok",
            description: "xAI's official coding agent and CLI (ACP via grok agent stdio)",
            // `@xai-official/grok` ships each platform's native binary as a
            // brotli-compressed **optional dependency** (`@xai-official/grok-<os>-<arch>`);
            // the npm `bin/grok` trampoline decompresses it into `~/.grok/bin` on
            // first run. Public mirrors (e.g. registry.npmmirror.com, a common CN
            // default) lag far behind this package — at time of writing only 0.1.4,
            // which predates the `grok agent stdio` ACP subcommand — so the pinned
            // version isn't resolvable there.
            //
            // Both concerns are handled by codeg's shared `npm install -g` path
            // (`install_npm_global_package_streaming` in commands/acp.rs), which
            // always passes `--include=optional` (pulls the platform binary) and
            // `--registry=https://registry.npmjs.org` (bypasses lagging mirrors)
            // for every npx agent — so no per-agent launch env is needed here.
            // (It couldn't live here anyway: the launch env is serialized as
            // leading `KEY=value` argv and sacp's `parse_env_var` only accepts
            // `[A-Za-z0-9_]` env names, which npm's `@scope:registry` key is not.)
            distribution: AgentDistribution::Npx {
                version: "0.2.111",
                package: "@xai-official/grok@0.2.111",
                cmd: "grok",
                // Only the ACP subcommand lives here. Grok's ROOT-level launch
                // flags (`--no-auto-update` always, `--permission-mode <value>`
                // only for a non-default permission mode) MUST precede this
                // subcommand — `grok agent stdio` itself rejects them (verified
                // against 0.2.94/0.2.99: it only accepts --debug/--debug-file/
                // --leader-socket) — so `build_agent` inserts them ahead of these
                // args rather than appending after.
                args: &["agent", "stdio"],
                env: &[],
                // `@xai-official/grok@0.2.111` declares `engines.node: ">=20"`;
                // surface that in preflight so Node 18 isn't silently accepted.
                node_required: Some("20.0.0"),
            },
        },
        AgentType::Cursor => AcpAgentMeta {
            agent_type,
            supports_mcp: true,
            name: "Cursor",
            description: "Cursor's coding agent (ACP via cursor-agent acp)",
            // Cursor's CLI ships as a ~230MB directory-tree archive (webpack
            // chunks + bundled Node runtime + ripgrep); the `cursor-agent`
            // entry is a shell script that resolves its own directory and
            // execs the sibling `node`, so the tree must stay intact —
            // `dir_entry` switches the binary cache to whole-tree extraction.
            // codeg deliberately does NOT run Cursor's official install
            // script: it symlinks `~/.local/bin/agent`, which collides with
            // Grok's CLI of the same name (observed overwriting it).
            // URL layout follows the ACP registry's `cursor` entry
            // (downloads.cursor.com/lab/<version>/<os>/<arch>/...); custom
            // versions substitute into the same pattern.
            distribution: AgentDistribution::Binary {
                version: "2026.07.20-8cc9c0b",
                cmd: "cursor-agent",
                args: &["acp"],
                env: &[],
                platforms: &[
                    PlatformBinary {
                        platform: "darwin-aarch64",
                        url: "https://downloads.cursor.com/lab/2026.07.20-8cc9c0b/darwin/arm64/agent-cli-package.tar.gz",
                    },
                    PlatformBinary {
                        platform: "darwin-x86_64",
                        url: "https://downloads.cursor.com/lab/2026.07.20-8cc9c0b/darwin/x64/agent-cli-package.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-aarch64",
                        url: "https://downloads.cursor.com/lab/2026.07.20-8cc9c0b/linux/arm64/agent-cli-package.tar.gz",
                    },
                    PlatformBinary {
                        platform: "linux-x86_64",
                        url: "https://downloads.cursor.com/lab/2026.07.20-8cc9c0b/linux/x64/agent-cli-package.tar.gz",
                    },
                    PlatformBinary {
                        platform: "windows-aarch64",
                        url: "https://downloads.cursor.com/lab/2026.07.20-8cc9c0b/windows/arm64/agent-cli-package.zip",
                    },
                    PlatformBinary {
                        platform: "windows-x86_64",
                        url: "https://downloads.cursor.com/lab/2026.07.20-8cc9c0b/windows/x64/agent-cli-package.zip",
                    },
                ],
                dir_entry: Some(BinaryDirEntry {
                    unix: "dist-package/cursor-agent",
                    windows: "dist-package/cursor-agent.cmd",
                }),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_npx_version(
        agent_type: AgentType,
        expected_version: &str,
        expected_package: &str,
        expected_node_required: Option<&str>,
    ) {
        let meta = get_agent_meta(agent_type);
        match meta.distribution {
            AgentDistribution::Npx {
                version,
                package,
                node_required,
                ..
            } => {
                assert_eq!(version, expected_version);
                assert_eq!(package, expected_package);
                assert_eq!(node_required, expected_node_required);
                assert_eq!(meta.registry_version(), Some(expected_version));
            }
            other => {
                panic!("expected npx distribution for {agent_type:?}, got {other:?}");
            }
        }
    }

    fn assert_uvx_version(
        agent_type: AgentType,
        expected_version: &str,
        expected_package: &str,
        expected_uv_required: Option<&str>,
        expected_python: Option<&str>,
    ) {
        let meta = get_agent_meta(agent_type);
        match meta.distribution {
            AgentDistribution::Uvx {
                version,
                package,
                uv_required,
                python,
                ..
            } => {
                assert_eq!(version, expected_version);
                assert_eq!(package, expected_package);
                assert_eq!(uv_required, expected_uv_required);
                assert_eq!(python, expected_python);
                assert_eq!(meta.registry_version(), Some(expected_version));
            }
            other => {
                panic!("expected uvx distribution for {agent_type:?}, got {other:?}");
            }
        }
    }

    fn assert_binary_version(
        agent_type: AgentType,
        expected_version: &str,
        expected_release_path: &str,
    ) {
        let meta = get_agent_meta(agent_type);
        match meta.distribution {
            AgentDistribution::Binary {
                version, platforms, ..
            } => {
                assert_eq!(version, expected_version);
                assert_eq!(meta.registry_version(), Some(expected_version));
                for platform in platforms {
                    assert!(
                        platform.url.contains(expected_release_path),
                        "{} URL did not use {expected_release_path}: {}",
                        platform.platform,
                        platform.url
                    );
                }
            }
            other => {
                panic!("expected binary distribution for {agent_type:?}, got {other:?}");
            }
        }
    }

    // Cursor is the only dir-tree binary agent: the archive must be kept
    // intact (bundled Node runtime) and launched via the in-tree entry
    // script, never copied out as a single file.
    #[test]
    fn cursor_pins_dir_tree_binary() {
        let meta = get_agent_meta(AgentType::Cursor);
        assert_binary_version(
            AgentType::Cursor,
            "2026.07.20-8cc9c0b",
            "/lab/2026.07.20-8cc9c0b/",
        );
        match meta.distribution {
            AgentDistribution::Binary {
                cmd,
                args,
                platforms,
                dir_entry,
                ..
            } => {
                assert_eq!(cmd, "cursor-agent");
                assert_eq!(args, &["acp"]);
                assert_eq!(platforms.len(), 6);
                let entry = dir_entry.expect("cursor must use dir-tree extraction");
                assert_eq!(entry.unix, "dist-package/cursor-agent");
                assert_eq!(entry.windows, "dist-package/cursor-agent.cmd");
            }
            other => panic!("expected binary distribution for Cursor, got {other:?}"),
        }
        // OpenCode stays on the single-binary copy-out path.
        match get_agent_meta(AgentType::OpenCode).distribution {
            AgentDistribution::Binary { dir_entry, .. } => assert!(dir_entry.is_none()),
            other => panic!("expected binary distribution for OpenCode, got {other:?}"),
        }
    }

    #[test]
    fn registry_pins_current_acp_agent_versions() {
        assert_npx_version(
            AgentType::ClaudeCode,
            "0.61.0",
            "@agentclientprotocol/claude-agent-acp@0.61.0",
            Some("22.0.0"),
        );
        assert_npx_version(
            AgentType::Gemini,
            "0.52.0",
            "@google/gemini-cli@0.52.0",
            Some("20.0.0"),
        );
        assert_npx_version(
            AgentType::OpenClaw,
            "2026.7.1",
            "openclaw@2026.7.1",
            Some("22.22.3"),
        );
        assert_npx_version(
            AgentType::Cline,
            "3.0.46",
            "cline@3.0.46",
            Some("22.0.0"),
        );
        assert_npx_version(
            AgentType::CodeBuddy,
            "2.126.0",
            "@tencent-ai/codebuddy-code@2.126.0",
            Some("22.0.0"),
        );
        assert_npx_version(
            AgentType::KimiCode,
            "0.29.0",
            "@moonshot-ai/kimi-code@0.29.0",
            Some("22.19.0"),
        );
        assert_npx_version(
            AgentType::Codex,
            "1.1.7",
            "@agentclientprotocol/codex-acp@1.1.7",
            Some("20.0.0"),
        );
        assert_npx_version(AgentType::Pi, "0.0.31", "pi-acp@0.0.31", Some("22.0.0"));
        assert_npx_version(
            AgentType::Grok,
            "0.2.111",
            "@xai-official/grok@0.2.111",
            Some("20.0.0"),
        );
        assert_binary_version(AgentType::OpenCode, "1.18.4", "/releases/download/v1.18.4/");
        assert_uvx_version(
            AgentType::Hermes,
            "0.19.0",
            "hermes-agent[acp,mcp]==0.19.0",
            Some("0.5.0"),
            // hermes-agent 0.19.0 is requires-python `<3.14`; uvx must pin an
            // interpreter it (and its win32 `pywinpty` dep) supports.
            Some("3.13"),
        );
    }

    // OpenClaw rejects MCP server entries inside `mcpServers` (the empty `[]`
    // field is still serialized and tolerated) and fails session/new on any
    // entry, so it must be the only agent with `supports_mcp == false`. Every
    // other agent (current and future) keeps it `true`. Iterating the full
    // registry means a newly-added agent that wrongly opts out — or a
    // regression flipping OpenClaw back on — trips this assert.
    #[test]
    fn only_openclaw_opts_out_of_mcp() {
        for agent_type in all_acp_agents() {
            let meta = get_agent_meta(agent_type);
            assert_eq!(
                meta.supports_mcp,
                agent_type != AgentType::OpenClaw,
                "unexpected supports_mcp for {agent_type:?}"
            );
        }
    }
}
