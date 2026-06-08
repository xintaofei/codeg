# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](./LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)
[![Docker](https://img.shields.io/badge/Docker-ready-2496ED)](./Dockerfile)

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

Codeg (Code Generation) is a multi-agent coding workspace. It brings multiple agents (Claude Code, Codex CLI, OpenCode, Gemini CLI, OpenClaw, Cline, etc.) into one workspace, supporting conversation aggregation and multi-agent collaboration, with desktop installation plus server/Docker deployment.

![gallery](./docs/images/gallery.svg)

## Sponsors

<table>
  <tr>
    <td colspan="2" align="center">
      <a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg" target="_blank"><img src="https://raw.githubusercontent.com/LeoYeAI/myclaw-sponsor-preview/main/banner.svg" alt="MyClaw.ai — Your OpenClaw Agent, Always On." /></a><br/>
      <strong><a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg">MyClaw.ai</a></strong> — A fully managed OpenClaw cloud platform with one-click setup, 24/7 uptime, and full data ownership — no server management required.
    </td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="./docs/images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare (UCloud)</a></strong>
    </td>
    <td>Thanks to Compshare for sponsoring this project! Compshare is UCloud's AI cloud platform, offering cost-effective monthly and pay-as-you-go agent Plan subscriptions for Chinese models, starting at just ¥49/month. It also provides stable officially-proxied access to overseas models. Supports Claude Code, Codex, and API integrations. Enterprise-ready with high concurrency, 24/7 technical support, and self-service invoicing. Users who sign up via <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">this link</a> receive ¥5 in free platform credits!</td>
  </tr>
</table>

> Want to become a Codeg sponsor? [Reach out to us by email.](mailto:itpkcn@gmail.com)

## Main Interface

![Codeg Light](./docs/images/main-light.png#gh-light-mode-only)
![Codeg Dark](./docs/images/main-dark.png#gh-dark-mode-only)

## Settings

![Codeg Light](./docs/images/settings-light.png#gh-light-mode-only)
![Codeg Dark](./docs/images/settings-dark.png#gh-dark-mode-only)

## Highlights

- **Conversation Aggregation** — import sessions from all supported agents into one unified workspace
- **Multi-Agent Collaboration** — within a single session, the main agent delegates to sub-agents of different types (e.g. Claude Code calling Codex, Gemini) to jointly complete a task, each running as an independent session
- Parallel development with built-in `git worktree` flows
- **Project Boot** — visually scaffold new projects with live preview
- **Chat Channels** — connect Telegram, Lark (Feishu), iLink (Weixin) and more to your coding agents for real-time notifications, full session interaction, and remote task control
- MCP management (local scan + registry search/install)
- Skills management (global and project scope)
- Git remote account management (GitHub and other Git servers)
- Web service mode — access Codeg from any browser for remote work
- **Standalone server deployment** — run `codeg-server` on any Linux/macOS server, access via browser
- **Docker support** — `docker compose up` or `docker run`, with custom token, port, and volume mounts for data persistence and project directories
- Integrated engineering loop (file tree, diff, git changes, commit, terminal)

## Supported Agents

| Agent       | Environment Variable Path             | macOS / Linux Default                 | Windows Default                                       |
| ----------- | ------------------------------------- | ------------------------------------- | ----------------------------------------------------- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects`         | `~/.claude/projects`                  | `%USERPROFILE%\\.claude\\projects`                    |
| Codex CLI   | `$CODEX_HOME/sessions`                | `~/.codex/sessions`                   | `%USERPROFILE%\\.codex\\sessions`                     |
| OpenCode    | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI  | `$GEMINI_CLI_HOME/.gemini`            | `~/.gemini`                           | `%USERPROFILE%\\.gemini`                              |
| OpenClaw    | —                                     | `~/.openclaw/agents`                  | `%USERPROFILE%\\.openclaw\\agents`                    |
| Cline       | `$CLINE_DIR`                          | `~/.cline/data/tasks`                 | `%USERPROFILE%\\.cline\\data\\tasks`                  |

> Note: environment variables take precedence over fallback paths.

<details>
<summary><h2>Project Boot</h2></summary>

Create new projects visually with a split-pane interface: configure on the left, preview in real time on the right.

![Project Boot Light](./docs/images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](./docs/images/project-boot-dark.png#gh-dark-mode-only)

### What it does

- **Visual Configuration** — pick style, color theme, icon library, font, border radius, and more from dropdowns; the preview iframe updates instantly
- **Live Preview** — see your chosen look & feel rendered in real time before creating anything
- **One-Click Scaffolding** — hit "Create Project" and the launcher runs `shadcn init` with your preset, framework template (Next.js / Vite / React Router / Astro / Laravel), and package manager of choice (pnpm / npm / yarn / bun)
- **Package Manager Detection** — automatically checks which package managers are installed and shows their versions
- **Seamless Integration** — the newly created project opens in Codeg's workspace right away

Currently supports **shadcn/ui** project scaffolding, with a tab-based design ready for more project types in the future.

</details>

<details>
<summary><h2>Chat Channels</h2></summary>

Connect your favorite messaging apps — Telegram, Lark (Feishu), iLink (Weixin), and more — to your AI coding agents. Create tasks, send follow-up messages, approve permissions, resume sessions, and monitor activity — all from your chat app. Receive real-time agent responses with tool-call details, permission prompts, and completion summaries without ever opening a browser.

### Supported Channels

| Channel        | Protocol                    | Status   |
| -------------- | --------------------------- | -------- |
| Telegram       | Bot API (HTTP long-polling) | Built-in |
| Lark (Feishu)  | WebSocket + REST API        | Built-in |
| iLink (Weixin) | WebSocket + REST API        | Built-in |

> More channels (Discord, Slack, DingTalk, etc.) are planned for future releases.

</details>

<details>
<summary><h2>Quick Start</h2></summary>

### Requirements

- Node.js `>=22` (recommended)
- pnpm `>=10`
- Rust stable (2021 edition)
- Tauri 2 build dependencies (desktop mode only)

Linux (Debian/Ubuntu) example:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Binaries

Codeg ships three Rust binaries from a single workspace:

| Binary         | Role                                                                                                         | Build                                                                       |
| -------------- | ------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------- |
| `codeg`        | Tauri desktop app (window, tray, updater)                                                                    | `pnpm tauri build` (release) / `pnpm tauri dev` (dev)                       |
| `codeg-server` | Standalone HTTP + WebSocket server for browser/headless deployments                                          | `pnpm server:build` / `pnpm server:dev`                                     |
| `codeg-mcp`    | Per-launch stdio MCP companion that surfaces the `delegate_to_agent` tool to agent CLIs (multi-agent collab) | `pnpm tauri:prepare-sidecars` (auto-invoked by `tauri dev` / `tauri build`) |

`codeg-mcp` must sit next to its parent binary at runtime — installers, the Docker image, and the Tauri sidecar bundler all place it next to `codeg` / `codeg-server`. Source builds and custom layouts can override the lookup with the `CODEG_MCP_BIN=/abs/path/codeg-mcp` env var. If the companion is missing, delegation is skipped (a single warning is logged) and the rest of the agent session keeps working.

### Development

```bash
pnpm install

# Frontend only (Next.js dev server, no Rust)
pnpm dev

# Frontend static export to out/
pnpm build

# Full desktop app (Tauri + Next.js, builds codeg-mcp sidecar automatically)
pnpm tauri dev

# Desktop release build (bundles codeg-mcp as externalBin)
pnpm tauri build

# Standalone server (no Tauri/GUI required)
pnpm server:dev
pnpm server:build                  # release binary at src-tauri/target/release/codeg-server

# Build the codeg-mcp companion explicitly (for the host triple)
pnpm tauri:prepare-sidecars        # output: src-tauri/binaries/codeg-mcp-<triple>

# Skip sidecar prep when iterating on the frontend and you don't need delegation
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# Frontend tests (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# Rust checks (run in src-tauri/)
cargo check                                                     # desktop (default features)
cargo check --no-default-features --bin codeg-server            # server mode
cargo check --no-default-features --bin codeg-mcp               # MCP companion
cargo clippy --all-targets --features test-utils -- -D warnings

# Rust tests
cargo test --features test-utils                                # desktop (incl. integration)
cargo test --no-default-features --bin codeg-server --lib       # server mode
cargo insta review                                              # accept parser snapshot updates
```

> Tip: when you have a fresh `codeg-mcp` build under `src-tauri/target/release/` and want to point a manually-launched `codeg-server` at it without reinstalling, export `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`.

### Server Deployment

Codeg can run as a standalone web server without a desktop environment.

#### Option 1: One-line install (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

Install a specific version or to a custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

Then run:

```bash
codeg-server
```

#### Option 2: One-line install (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

Or install a specific version:

```powershell
.\install.ps1 -Version v0.5.2
```

#### Option 3: Download from GitHub Releases

Pre-built binaries (with bundled web assets) are available on the [Releases](https://github.com/xintaofei/codeg/releases) page:

| Platform    | File                               |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# Example: download, extract, and run
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

> For unattended deployments, start it with `--supervise` so a failed in-place upgrade is automatically rolled back — see [In-place updates](#in-place-updates).

#### Option 4: Docker

```bash
# Using Docker Compose (recommended)
docker compose up -d

# Or run directly with Docker
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# With custom token and project directory mounted
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

The Docker image uses a multi-stage build (Node.js + Rust → slim Debian runtime) and includes `git` and `ssh` for repository operations. Data is persisted in the `/data` volume. You can optionally mount project directories to access local repos from within the container.

#### Option 5: Build from source

```bash
pnpm install && pnpm build          # build frontend
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # delegation companion
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp is picked up as a sibling
```

If you keep the two binaries in separate directories, set `CODEG_MCP_BIN=/abs/path/to/codeg-mcp` so the runtime can still find the companion; without it, multi-agent delegation is silently disabled.

#### In-place updates

The server can update itself from **Settings → Software Update**: it downloads the signed release for its platform, swaps the binaries and web assets on disk, and restarts — no manual re-deploy. This is Linux/macOS only (disabled on Windows). The previous version is kept as a backup, so the same screen offers a **Roll back** action to return to it.

**Run under the supervisor for auto-rollback.** Start the standalone server with `--supervise` so a freshly-upgraded process that fails to boot within the trial window is automatically reverted to the previous version:

```bash
CODEG_STATIC_DIR=./web ./codeg-server --supervise
```

Without `--supervise` the server still updates in place (it re-execs itself), but the upgrade is best-effort: there is no supervisor to auto-roll-back a version that can't start. The Docker image already runs under the supervisor.

**Docker upgrades change the container, not the image.** An in-place upgrade rewrites the binaries and web assets inside the running container's writable layer, so they live only in that container. The `/data` volume persists, but the upgraded files do **not**: recreating the container — `docker compose up --force-recreate`, a fresh `docker run`, or recreating after a `docker pull` — starts from the image again and drops the in-place upgrade. (A `docker pull` on its own only refreshes the local image; nothing reverts until the container is recreated.) To make an upgrade permanent, build or pull an image at the new version and recreate the container from it.

#### Configuration

Environment variables:

| Variable                       | Default                | Description                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------ | ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | HTTP port                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `CODEG_HOST`                   | `0.0.0.0`              | Bind address                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `CODEG_TOKEN`                  | _(random)_             | Auth token (printed to stderr on start)                                                                                                                                                                                                                                                                                                                                                                                          |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | SQLite database directory (also roots `uploads/`, `pets/`)                                                                                                                                                                                                                                                                                                                                                                       |
| `CODEG_STATIC_DIR`             | `./web` or `./out`     | Next.js static export directory                                                                                                                                                                                                                                                                                                                                                                                                  |
| `CODEG_MCP_BIN`                | _(unset)_              | Absolute path to the `codeg-mcp` companion. Overrides the default sibling-of-executable + `PATH` lookup. Use this for source builds or custom layouts where the companion lives outside the server's install directory.                                                                                                                                                                                                          |
| `CODEG_SKIP_SIDECAR`           | _(unset)_              | Frontend-only convenience for `pnpm tauri dev` / `pnpm tauri build` — when `1`, skips building the `codeg-mcp` sidecar. Delegation is disabled in that build; ship-quality artifacts must leave it unset.                                                                                                                                                                                                                        |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _(unset)_              | Hard cap on total bytes resident under `<data dir>/uploads/`. Plain decimal byte count (e.g. `10737418240` for 10 GiB). Unset, `0`, or an unparseable value disables the cap and prints a startup line so the posture is visible. The cap is enforced within a single `codeg-server` process — horizontally-scaled deployments sharing one `uploads/` volume need external coordination (file lock, Redis, reverse-proxy quota). |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _(unset)_              | When truthy (`1` / `true` / `yes` / `on`), abort startup with exit code 2 if `CODEG_UPLOAD_MAX_TOTAL_BYTES` is set to an unparseable value, instead of fail-open with a WARN. Use this when your security policy requires "configured quota must be effective".                                                                                                                                                                  |

</details>

<details>
<summary><h2>Architecture</h2></summary>

```text
Next.js 16 (Static Export) + React 19
        |
        | invoke() (desktop) / fetch() + WebSocket (web)
        v
  ┌─────────────────────────┐
  │   Transport Abstraction  │
  │  (Tauri IPC or HTTP/WS) │
  └─────────────────────────┘
        |
        v
┌─── Tauri Desktop ───┐    ┌─── codeg-server ───┐
│  Tauri 2 Commands    │    │  Axum HTTP + WS    │
│  (window management) │    │  (standalone mode)  │
└──────────┬───────────┘    └──────────┬──────────┘
           └──────────┬───────────────┘
                      v
            Shared Rust Core
              |- AppState
              |- ACP Manager
              |- Parsers (conversation ingestion)
              |- Chat Channels
              |- Git / File Tree / Terminal
              |- MCP marketplace + config
              |- SeaORM + SQLite
                      |
              ┌───────┼───────┐
              v       v       v
  Local Filesystem  Git   Chat Channels
    / Git Repos    Repos  (Telegram, Lark, iLink)
```

</details>

## Privacy & Security

- Local-first by default for parsing, storage, and project operations
- Network access happens only on user-triggered actions
- System proxy support for enterprise environments
- Web service mode uses token-based authentication

## Community

- Scan the QR code below to join our WeChat group for discussions, feedback, and updates

<img src="./docs/images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="./docs/images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- Thanks to the [LinuxDO](https://linux.do) community for their support

## Acknowledgments

- [ACP](https://agentclientprotocol.com) — the Agent Client Protocol (ACP) is the foundation that enables Codeg to connect with multiple agents

## License

Apache-2.0. See `LICENSE`.
