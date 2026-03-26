# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](./LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)

<p>
  <strong>English</strong> |
  <a href="./docs/readme/README.zh-CN.md">简体中文</a> |
  <a href="./docs/readme/README.zh-TW.md">繁體中文</a> |
  <a href="./docs/readme/README.ja.md">日本語</a> |
  <a href="./docs/readme/README.ko.md">한국어</a> |
  <a href="./docs/readme/README.es.md">Español</a> |
  <a href="./docs/readme/README.de.md">Deutsch</a> |
  <a href="./docs/readme/README.fr.md">Français</a> |
  <a href="./docs/readme/README.pt.md">Português</a> |
  <a href="./docs/readme/README.ar.md">العربية</a>
</p>

Codeg (Code Generation) is an enterprise-grade multi-agent coding workspace.
It unifies local AI coding agents (Claude Code, Codex CLI, OpenCode, Gemini CLI,
OpenClaw, etc.) in a desktop app and web service — enabling remote development from
any browser — with session aggregation, parallel `git worktree` development,
MCP/Skills management, and integrated Git/file/terminal workflows.

## Main Interface
![Codeg Light](./docs/images/main-light.png#gh-light-mode-only)
![Codeg Dark](./docs/images/main-dark.png#gh-dark-mode-only)

## Session tile display
![Codeg Light](./docs/images/main2-light.png#gh-light-mode-only)
![Codeg Dark](./docs/images/main2-dark.png#gh-dark-mode-only)

> Current status: `v0.2.x` (fast iteration, suitable for early adopters)

## Highlights

- Unified multi-agent workspace in the same project
- Local session ingestion with structured rendering
- Parallel development with built-in `git worktree` flows
- MCP management (local scan + registry search/install)
- Skills management (global and project scope)
- Git remote account management (GitHub and other Git servers)
- Web service mode — access Codeg from any browser for remote work
- Integrated engineering loop (file tree, diff, git changes, commit, terminal)

## Supported Scope

### 1) Session Ingestion (historical sessions)

| Agent | Environment Variable Path | macOS / Linux Default | Windows Default |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> Note: environment variables take precedence over fallback paths.

### 2) ACP real-time sessions

Currently supports 5 agents: Claude Code, Codex CLI, Gemini CLI, OpenCode, and OpenClaw.

### 3) Skills settings support

- Supported: `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- More adapters will be added incrementally

### 4) MCP target apps

Current writable targets:

- Claude Code
- Codex
- OpenCode

## Quick Start

### Requirements

- Node.js `>=22` (recommended)
- pnpm `>=10`
- Rust stable (2021 edition)
- Tauri 2 build dependencies

Linux (Debian/Ubuntu) example:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Development

```bash
pnpm install

# Full desktop app (Tauri + Next.js)
pnpm tauri dev

# Frontend only
pnpm dev

# Frontend static export to out/
pnpm build

# Desktop build
pnpm tauri build

# Lint
pnpm eslint .

# Rust checks (run in src-tauri/)
cargo check
cargo clippy
cargo build
```

## Architecture

```text
Next.js 16 (Static Export) + React 19
        |
        | invoke()
        v
Tauri 2 Commands (Rust)
  |- ACP Manager
  |- Parsers (local session ingestion)
  |- Git / File Tree / Terminal runtime
  |- MCP marketplace + local config writer
  |- SeaORM + SQLite
        |
        v
Local Filesystem / Local Agent Data / Git Repos
```

## Constraints

- Frontend uses static export (`output: "export"`)
- No Next.js dynamic routes (`[param]`); use query params instead
- Tauri command params: frontend `camelCase`, Rust `snake_case`
- TypeScript strict mode

## Privacy & Security

- Local-first by default for parsing, storage, and project operations
- Network access happens only on user-triggered actions
- System proxy support for enterprise environments

## License

Apache-2.0. See `LICENSE`.
