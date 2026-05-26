# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)
[![Docker](https://img.shields.io/badge/Docker-ready-2496ED)](../../Dockerfile)

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

Codeg（Code Generation）是一個多智慧體編碼工作台，它將多個智慧體（Claude Code、Codex CLI、OpenCode、Gemini CLI、OpenClaw、Cline 等）統一到一個工作區中，支援會話彙整和多智慧體協作，支援桌面安裝、伺服器/Docker 部署。

![gallery](../images/gallery.svg)

## 贊助

<table>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="優雲智算" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">優雲智算</a></strong>
    </td>
    <td>感謝優雲智算贊助了本專案！優雲智算是 UCloud 旗下 AI 雲平台，主打包月、按次的高性價比國模 agent Plan 套餐，低至 49 元/月起。同時提供官轉穩定海外模型。支援接入 Claude Code、Codex 及 API 呼叫。支援企業高併發、7*24 技術支援、自助開票。透過<a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">此連結</a>註冊的使用者，可得免費 5 元平台體驗金！</td>
  </tr>
</table>

> 想成為 Codeg 贊助商？[歡迎透過郵件與我們聯絡。](mailto:itpkcn@gmail.com)

## 主介面

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## 設定

![Codeg Light](../images/settings-light.png#gh-light-mode-only)
![Codeg Dark](../images/settings-dark.png#gh-dark-mode-only)

## 核心亮點

- **會話聚合** — 將所有受支援智能體的會話匯入到統一工作台
- **多智能體協作** — 在同一會話中，主智能體可呼叫不同類型的子智能體（如 Claude Code 呼叫 Codex、Gemini 等）協作完成任務，每個子智能體作為獨立會話執行
- 內建 `git worktree` 並行開發流程
- **專案啟動器** — 視覺化建立新專案，即時預覽效果
- **訊息渠道** — 連接 Telegram、飛書、iLink（微信）等即時通訊應用到編碼代理，即時接收通知、完整會話交互、遠端任務控制
- MCP 管理（本地掃描 + 市場搜尋/安裝）
- Skills 管理（全域與專案級）
- Git 遠端帳號管理（支援 GitHub 及其他 Git 伺服器）
- Web 服務模式 — 開啟後可在瀏覽器中存取 Codeg，支援遠端工作
- **獨立伺服器部署** — 在任意 Linux/macOS 伺服器上執行 `codeg-server`，透過瀏覽器存取
- **Docker 支援** — `docker compose up` 或 `docker run`，可自訂令牌、連接埠，支援資料持久化及專案目錄掛載
- 整合工程閉環（檔案樹、Diff、Git 變更、提交、終端）

## 支援的 Agent

| Agent       | 環境變數優先路徑                      | macOS / Linux 預設路徑                | Windows 預設路徑                                      |
| ----------- | ------------------------------------- | ------------------------------------- | ----------------------------------------------------- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects`         | `~/.claude/projects`                  | `%USERPROFILE%\\.claude\\projects`                    |
| Codex CLI   | `$CODEX_HOME/sessions`                | `~/.codex/sessions`                   | `%USERPROFILE%\\.codex\\sessions`                     |
| OpenCode    | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI  | `$GEMINI_CLI_HOME/.gemini`            | `~/.gemini`                           | `%USERPROFILE%\\.gemini`                              |
| OpenClaw    | —                                     | `~/.openclaw/agents`                  | `%USERPROFILE%\\.openclaw\\agents`                    |
| Cline       | `$CLINE_DIR`                          | `~/.cline/data/tasks`                 | `%USERPROFILE%\\.cline\\data\\tasks`                  |

> 注意：環境變數的優先順序高於預設路徑。

<details>
<summary><h2>專案啟動器</h2></summary>

視覺化建立新專案：左側設定面板，右側即時預覽。

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### 功能特色

- **視覺化設定** — 從下拉選單中選擇樣式、色彩主題、圖示庫、字型、圓角等，預覽面板即時更新
- **即時預覽** — 在建立專案前，即時檢視所選樣式的渲染效果
- **一鍵建立** — 點擊「建立專案」，啟動器將使用您的預設設定、框架範本（Next.js / Vite / React Router / Astro / Laravel）和套件管理器（pnpm / npm / yarn / bun）執行 `shadcn init`
- **套件管理器偵測** — 自動偵測已安裝的套件管理器並顯示版本號
- **無縫整合** — 新建立的專案會立即在 Codeg 工作台中開啟

目前支援 **shadcn/ui** 專案腳手架，分頁式設計為未來支援更多專案類型做好了準備。

</details>

<details>
<summary><h2>訊息渠道</h2></summary>

連接你喜愛的即時通訊應用——Telegram、飛書、iLink（微信）等——到 AI 編碼代理。直接在聊天中建立任務、發送後續訊息、審批權限請求、恢復會話、監控代理活動——即時接收代理回應，包含工具呼叫詳情、權限提示和完成摘要。

### 支援的渠道

| 渠道          | 協定                   | 狀態 |
| ------------- | ---------------------- | ---- |
| Telegram      | Bot API（HTTP 長輪詢） | 內建 |
| 飛書          | WebSocket + REST API   | 內建 |
| iLink（微信） | WebSocket + REST API   | 內建 |

> 更多渠道（Discord、Slack、釘釘等）計劃在未來版本中支援。

</details>

<details>
<summary><h2>快速開始</h2></summary>

### 環境需求

- Node.js `>=22`（建議）
- pnpm `>=10`
- Rust stable（2021 edition）
- Tauri 2 建置依賴（僅桌面模式）

Linux（Debian/Ubuntu）範例：

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### 二進位檔

Codeg 在單一 workspace 中提供三個 Rust 二進位檔：

| 二進位         | 角色                                                                                          | 建置方式                                                                    |
| -------------- | --------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `codeg`        | Tauri 桌面應用程式（視窗、系統匣、自動更新）                                                  | `pnpm tauri build`（發行）/ `pnpm tauri dev`（開發）                        |
| `codeg-server` | 用於瀏覽器/無頭部署的獨立 HTTP + WebSocket 伺服器                                             | `pnpm server:build` / `pnpm server:dev`                                     |
| `codeg-mcp`    | 單次啟動的 stdio MCP 協作行程，向 agent CLI 公開 `delegate_to_agent` 工具（多智慧體協作）     | `pnpm tauri:prepare-sidecars`（由 `tauri dev` / `tauri build` 自動呼叫）    |

`codeg-mcp` 在執行階段必須與其父二進位位於同一目錄——安裝程式、Docker 映像和 Tauri sidecar 打包工具都會將它放在 `codeg` / `codeg-server` 旁邊。原始碼建置和自訂部署可以透過 `CODEG_MCP_BIN=/abs/path/codeg-mcp` 環境變數覆寫查詢路徑。若協作行程缺失，委派功能會被略過（僅記錄一則警告日誌），其餘 agent 會話仍可正常運作。

### 開發命令

```bash
pnpm install

# 僅前端（Next.js 開發伺服器，無需 Rust）
pnpm dev

# 前端靜態匯出到 out/
pnpm build

# 完整桌面應用（Tauri + Next.js，自動建置 codeg-mcp sidecar）
pnpm tauri dev

# 桌面發行建置（將 codeg-mcp 作為 externalBin 打包）
pnpm tauri build

# 獨立伺服器（無需 Tauri/GUI）
pnpm server:dev
pnpm server:build                  # 發行二進位位於 src-tauri/target/release/codeg-server

# 顯式建置 codeg-mcp 協作行程（針對當前主機 triple）
pnpm tauri:prepare-sidecars        # 輸出：src-tauri/binaries/codeg-mcp-<triple>

# 當僅迭代前端且不需要委派功能時，略過 sidecar 準備
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# 前端測試（vitest）
pnpm test
pnpm test:watch
pnpm test:coverage

# Rust 檢查（在 src-tauri/ 下執行）
cargo check                                                     # 桌面（預設 features）
cargo check --no-default-features --bin codeg-server            # 伺服器模式
cargo check --no-default-features --bin codeg-mcp               # MCP 協作行程
cargo clippy --all-targets --features test-utils -- -D warnings

# Rust 測試
cargo test --features test-utils                                # 桌面（含整合）
cargo test --no-default-features --bin codeg-server --lib       # 伺服器模式
cargo insta review                                              # 接受解析器快照變更
```

> 提示：當你在 `src-tauri/target/release/` 下有新建置的 `codeg-mcp` 並想讓手動啟動的 `codeg-server` 在不重新安裝的情況下指向它時，可以匯出 `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`。

### 伺服器部署

Codeg 可以作為獨立 Web 伺服器執行，無需桌面環境。

#### 方式一：一鍵安裝（Linux / macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

安裝指定版本或到自訂目錄：

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

然後執行：

```bash
codeg-server
```

#### 方式二：一鍵安裝（Windows PowerShell）

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

或安裝指定版本：

```powershell
.\install.ps1 -Version v0.5.2
```

#### 方式三：從 GitHub Releases 下載

預建置二進位檔（已打包 Web 前端資源）可在 [Releases](https://github.com/xintaofei/codeg/releases) 頁面下載：

| 平台        | 檔案                               |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# 範例：下載、解壓縮、執行
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

#### 方式四：Docker

```bash
# 使用 Docker Compose（推薦）
docker compose up -d

# 或直接使用 Docker 執行
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# 自訂令牌並掛載專案目錄
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

Docker 映像採用多階段建置（Node.js + Rust → 精簡 Debian 執行環境），內建 `git` 和 `ssh` 以支援倉庫操作。資料持久化儲存在 `/data` 卷中。可選掛載專案目錄以從容器內存取本地倉庫。

#### 方式五：從原始碼建置

```bash
pnpm install && pnpm build          # 建置前端
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # 委派協作行程
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp 會作為同級二進位被自動探測
```

> 若兩個二進位分別存放在不同目錄，請設定 `CODEG_MCP_BIN=/abs/path/to/codeg-mcp`，執行階段才能找到協作行程；否則多智慧體委派會被靜默停用。

#### 設定

環境變數：

| 變數                           | 預設值                 | 說明                                                                                                                                                                                                                                                                                                              |
| ------------------------------ | ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | HTTP 連接埠                                                                                                                                                                                                                                                                                                       |
| `CODEG_HOST`                   | `0.0.0.0`              | 綁定位址                                                                                                                                                                                                                                                                                                          |
| `CODEG_TOKEN`                  | _（隨機）_             | 認證令牌（啟動時輸出到 stderr）                                                                                                                                                                                                                                                                                   |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | SQLite 資料庫目錄（同時也是 `uploads/`、`pets/` 的根目錄）                                                                                                                                                                                                                                                        |
| `CODEG_STATIC_DIR`             | `./web` 或 `./out`     | Next.js 靜態匯出目錄                                                                                                                                                                                                                                                                                              |
| `CODEG_MCP_BIN`                | _（未設定）_           | `codeg-mcp` 協作行程的絕對路徑。會覆寫預設的「可執行檔同級目錄 + `PATH`」查詢邏輯。用於原始碼建置或協作行程不在伺服器安裝目錄內的自訂部署。                                                                                                                                                                       |
| `CODEG_SKIP_SIDECAR`           | _（未設定）_           | 僅供 `pnpm tauri dev` / `pnpm tauri build` 調試前端時使用——當值為 `1` 時，略過 `codeg-mcp` sidecar 的建置。此類建置不支援委派功能；發行品質的產出物必須保持此變數未設定。                                                                                                                                          |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _（未設定）_           | `<data dir>/uploads/` 下所有檔案總位元組數的硬上限。十進位位元組數（例如 `10737418240` 表示 10 GiB）。未設定、`0` 或無法解析的值會停用上限，並在啟動時印出一行日誌以便觀察當前狀態。該上限僅在單一 `codeg-server` 行程內生效——共用同一個 `uploads/` 卷的橫向擴展部署需要外部協調（檔案鎖、Redis、反向代理配額）。 |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _（未設定）_           | 當值為真（`1` / `true` / `yes` / `on`）時，若 `CODEG_UPLOAD_MAX_TOTAL_BYTES` 設定為無法解析的值，則以結束代碼 2 中止啟動，而不是發出 WARN 後繼續執行。當安全政策要求「設定的配額必須生效」時使用此選項。                                                                                                          |

</details>

<details>
<summary><h2>架構</h2></summary>

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

## 隱私與安全

- 預設本地優先：解析、儲存、專案操作均在本地完成
- 僅在使用者主動觸發時才存取網路
- 支援系統代理，適配企業網路環境
- Web 服務模式使用基於令牌的身份認證

## 交流

- 掃描下方 QR Code 加入我們的微信群，參與討論、回饋與更新

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- 感謝 [LinuxDO](https://linux.do) 社群的支持

## 致謝

- [ACP](https://agentclientprotocol.com)：智能體客戶端協定 (ACP) 是 codeg 實現多智能體連接的基礎

## 授權

Apache-2.0，詳見 `LICENSE`。
