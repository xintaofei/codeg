# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)

<p>
  <a href="../../README.md">English</a> |
  <strong>简体中文</strong> |
  <a href="./README.zh-TW.md">繁體中文</a> |
  <a href="./README.ja.md">日本語</a> |
  <a href="./README.ko.md">한국어</a> |
  <a href="./README.es.md">Español</a> |
  <a href="./README.de.md">Deutsch</a> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg（Code Generation）是一个企业级多 Agent 编码工作台。
它将本地 AI 编码代理（Claude Code、Codex CLI、OpenCode、Gemini CLI、
OpenClaw 等）统一到桌面应用与 Web 服务中——通过浏览器即可远程开发——支持会话聚合、
并行 `git worktree` 开发、MCP/Skills 管理，以及集成的 Git/文件/终端工作流。

## 主界面
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## 会话平铺显示
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> 当前状态：`v0.2.x`（快速迭代中，适合早期使用者）

## 核心亮点

- 同一项目中的多 Agent 统一工作台
- 本地会话解析与结构化渲染
- 内置 `git worktree` 并行开发流程
- MCP 管理（本地扫描 + 市场搜索/安装）
- Skills 管理（全局与项目级）
- Git 远程账号管理（支持 GitHub 及其它 Git 服务器）
- Web 服务模式 — 开启后可在浏览器中访问 Codeg，支持远程工作
- 集成工程闭环（文件树、Diff、Git 变更、提交、终端）

## 支持范围

### 1) 会话解析（历史会话）

| Agent | 环境变量优先路径 | macOS / Linux 默认路径 | Windows 默认路径 |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> 注意：环境变量的优先级高于默认路径。

### 2) ACP 实时会话

目前支持 5 种代理：Claude Code、Codex CLI、Gemini CLI、OpenCode 和 OpenClaw。

### 3) Skills 设置支持

- 已支持：`Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- 更多适配器将持续补齐

### 4) MCP 目标应用

当前可写入的目标：

- Claude Code
- Codex
- OpenCode

## 快速开始

### 环境要求

- Node.js `>=22`（推荐）
- pnpm `>=10`
- Rust stable（2021 edition）
- Tauri 2 构建依赖

Linux（Debian/Ubuntu）示例：

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### 开发命令

```bash
pnpm install

# 完整桌面应用（Tauri + Next.js）
pnpm tauri dev

# 仅前端
pnpm dev

# 前端静态导出到 out/
pnpm build

# 桌面应用构建
pnpm tauri build

# Lint
pnpm eslint .

# Rust 检查（在 src-tauri/ 下执行）
cargo check
cargo clippy
cargo build
```

## 架构

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

## 开发约束

- 前端使用静态导出（`output: "export"`）
- 不使用 Next.js 动态路由（`[param]`），统一使用查询参数
- Tauri 命令参数：前端 `camelCase`，Rust `snake_case`
- TypeScript strict 模式

## 隐私与安全

- 默认本地优先：解析、存储、项目操作均在本地完成
- 仅在用户主动触发时才访问网络
- 支持系统代理，适配企业网络环境

## 许可证

Apache-2.0，详见 `LICENSE`。
