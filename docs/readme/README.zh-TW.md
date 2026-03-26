# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)

<p>
  <a href="../../README.md">English</a> |
  <a href="./README.zh-CN.md">简体中文</a> |
  <strong>繁體中文</strong> |
  <a href="./README.ja.md">日本語</a> |
  <a href="./README.ko.md">한국어</a> |
  <a href="./README.es.md">Español</a> |
  <a href="./README.de.md">Deutsch</a> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg（Code Generation）是一個企業級多 Agent 編碼工作台。
它將本地 AI 編碼代理（Claude Code、Codex CLI、OpenCode、Gemini CLI、
OpenClaw 等）整合到桌面應用與 Web 服務中——透過瀏覽器即可遠端開發——支援會話彙整、並行 `git worktree`
開發、MCP/Skills 管理，以及整合的 Git/檔案/終端工作流。

## 主介面
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## 會話平鋪顯示
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> 目前狀態：`v0.2.x`（快速迭代中，適合早期使用者）

## 核心亮點

- 同一專案中的多 Agent 統一工作台
- 本地會話解析與結構化渲染
- 內建 `git worktree` 並行開發流程
- MCP 管理（本地掃描 + 市場搜尋/安裝）
- Skills 管理（全域與專案級）
- Git 遠端帳號管理（支援 GitHub 及其他 Git 伺服器）
- Web 服務模式 — 開啟後可在瀏覽器中存取 Codeg，支援遠端工作
- 整合工程閉環（檔案樹、Diff、Git 變更、提交、終端）

## 支援範圍

### 1) 會話解析（歷史會話）

| Agent | 環境變數優先路徑 | macOS / Linux 預設路徑 | Windows 預設路徑 |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> 注意：環境變數的優先順序高於預設路徑。

### 2) ACP 即時會話

目前支援 5 種代理：Claude Code、Codex CLI、Gemini CLI、OpenCode 和 OpenClaw。

### 3) Skills 設定支援

- 已支援：`Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- 更多適配器將持續補齊

### 4) MCP 目標應用

目前可寫入的目標：

- Claude Code
- Codex
- OpenCode

## 快速開始

### 環境需求

- Node.js `>=22`（建議）
- pnpm `>=10`
- Rust stable（2021 edition）
- Tauri 2 建置依賴

Linux（Debian/Ubuntu）範例：

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### 開發命令

```bash
pnpm install

# 完整桌面應用（Tauri + Next.js）
pnpm tauri dev

# 僅前端
pnpm dev

# 前端靜態匯出到 out/
pnpm build

# 桌面應用建置
pnpm tauri build

# Lint
pnpm eslint .

# Rust 檢查（在 src-tauri/ 下執行）
cargo check
cargo clippy
cargo build
```

## 架構

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

## 開發約束

- 前端使用靜態匯出（`output: "export"`）
- 不使用 Next.js 動態路由（`[param]`），改用查詢參數
- Tauri 命令參數：前端 `camelCase`，Rust `snake_case`
- TypeScript strict 模式

## 隱私與安全

- 預設本地優先：解析、儲存、專案操作均在本地完成
- 僅在使用者主動觸發時才存取網路
- 支援系統代理，適配企業網路環境

## 授權

Apache-2.0，詳見 `LICENSE`。
