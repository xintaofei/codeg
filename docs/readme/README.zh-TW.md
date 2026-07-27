# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![Docs](https://img.shields.io/badge/docs-docs.codeg.app-3451b2)](https://docs.codeg.app)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)

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

Codeg（Code Generation）是一個多智慧體編碼工作台：把所有 AI 編碼智慧體收進同一個地方 —— 並讓它們協同工作。

它將所有支援的智慧體 CLI 的工作階段聚合進一個可搜尋的工作區，讓主智慧體在同一個任務內委派給其它類型的子智慧體，並可作為桌面應用、獨立伺服器或 Docker 容器執行；此外還有原生 iOS 與 Android 用戶端，讓你離開電腦後也能接手正在跑的任務。

![工作區](../images/workspace-light.png#gh-light-mode-only)
![工作區](../images/workspace-dark.png#gh-dark-mode-only)

## 📖 文件

**完整文件見 [docs.codeg.app](https://docs.codeg.app)** — [快速開始](https://docs.codeg.app/zh/getting-started/) · [指南](https://docs.codeg.app/zh/guide/) · [參考](https://docs.codeg.app/zh/reference/)

## 💖 贊助

<table>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="優雲智算" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">優雲智算</a></strong>
    </td>
    <td>感謝優雲智算贊助了本專案！優雲智算是 UCloud 旗下 AI 雲平台，主打包月、按次的高性價比國模 agent Plan 套餐，低至 49 元/月起。同時提供官轉穩定海外模型。支援接入 Claude Code、Codex 及 API 呼叫。支援企業高併發、7*24 技術支援、自助開票。透過<a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">此連結</a>註冊的使用者，可得免費 5 元平台體驗金！</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE" target="_blank"><img src="../images/sui-xiang.jpg" alt="隨想AI中轉站" width="200" /></a><br/>
      <strong><a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">隨想AI中轉站</a></strong>
    </td>
    <td>感謝隨想AI中轉站對本專案的贊助！隨想AI中轉站是一家可靠高效的 API 中繼服務提供商，提供 Claude、Codex、Gemini 等的中繼服務。新帳戶<a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">註冊</a>後每日簽到就送 0.5 元測試額度，儲值額度 1:1，無需訂閱，按量付費。多線路冗餘、跨區域容災、自動故障切換，長連線 SSE 不中斷。</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://hezu.ink/sign-up?aff=0wVz" target="_blank"><img src="../images/hezu-ink.jpg" alt="合租巴士" width="200" /></a><br/>
      <strong><a href="https://hezu.ink/sign-up?aff=0wVz">合租巴士</a></strong>
    </td>
    <td>感謝合租巴士對本專案的贊助！合租巴士是一家可靠高效的 AI 中轉服務平台，主要提供 Codex、Claude Code 等主流模型的高穩定中轉能力，儲值比例透明（1:1），Codex 倍率補貼低至 0.08。<a href="https://hezu.ink/sign-up?aff=0wVz">官網進群送 5 美元體驗金</a>。</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta" target="_blank"><img src="../images/onehop.jpg" alt="OneHop" width="120" /></a><br/>
      <strong><a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">OneHop</a></strong>
    </td>
    <td>感謝 OneHop 對本專案的贊助！OneHop 讓 Codeg 使用者僅用一個 OpenAI 相容的 API 金鑰即可呼叫數百款領先模型，包括 GPT、Claude、Gemini、DeepSeek、Kimi 和 Qwen。無需管理多個供應商帳號或反覆修改程式碼即可切換模型，且按用量付費。<a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">透過 Codeg 註冊</a>即可獲得 1 美元額度，再加入 OneHop 社群並參與歡迎活動可額外獲得 5 美元——最高共計 6 美元測試額度。</td>
  </tr>
</table>

> 想成為 Codeg 贊助商？[歡迎透過郵件與我們聯絡。](mailto:itpkcn@gmail.com)

## 🤖 支援的 Agent

Claude Code · Codex · Gemini · OpenClaw · OpenCode · Cline · Hermes · CodeBuddy · Kimi Code · Pi · Grok · Cursor

其中大部分 Codeg 都能替你安裝、鎖定版本並更新。完整名單、各自的執行環境需求以及工作階段在磁碟上的存放位置，見 [支援的智慧體](https://docs.codeg.app/zh/guide/supported-agents)。

## 🤝 多智慧體協作

多智慧體協作，從此只需一個按鍵：輸入 `@`，選取智慧體，送出。剩下的排程全交給 Codeg —— 它把每個被提及的智慧體拉起為獨立工作階段，交付任務，再把工作即時匯流回你正在進行的對話。提及兩個，它們就並肩開工：Claude Code 起草，Codex 同步審查。不必來回切換脈絡，也不用在多個終端機之間複製貼上。

![在單一 Codeg 對話中將任務委派給子智慧體](../images/collaboration-light.gif#gh-light-mode-only)
![在單一 Codeg 對話中將任務委派給子智慧體](../images/collaboration-dark.gif#gh-dark-mode-only)

## 📄 Office 文件

讓智慧體做一份簡報、一份報告或一張試算表，它交付的是真正的 `.pptx` / `.docx` / `.xlsx` —— 右側面板同時即時算繪。每一次改動都會自己落進預覽：投影片逐頁成形，表格逐步鋪開，數字落入儲存格。第 4 頁不滿意？下一則訊息說一聲就行 —— 智慧體原地修改同一個檔案，預覽隨即跟上。無需匯出，無需外部 Office 應用程式，全程不用離開 Codeg。

![智慧體編輯 Office 文件，旁邊是即時預覽](../images/office-light.png#gh-light-mode-only)
![智慧體編輯 Office 文件，旁邊是即時預覽](../images/office-dark.png#gh-dark-mode-only)

## 💻 工作區

一個工作區，容納所有智慧體。無論正在幹活的是 Claude Code、Codex 還是 Cursor，它們都在同一個編輯器、同一套即時 diff、同一個 Git 用戶端裡工作，而產出的是你儲存庫裡真實的檔案，就在你眼前變化。

**工作階段**：把你已有的歷史一併接管 —— 所有已安裝智慧體的過往工作階段，一鍵匯入，並可從中斷處繼續。進來之後它們不再是彼此隔絕的孤島 —— `@` 提及一個舊工作階段，你正在對話的智慧體就能讀到它，哪怕那是另一個智慧體留下的，於是今天的 Codex 能接著上週 Claude Code 停下的地方往下做。

**檔案**：智慧體的改動會以 diff 的形式，隨著落檔即時呈現在對話旁邊。任意檔案都能在帶語法高亮的真實編輯器裡開啟，用 `⌘L` 把整個檔案（或僅一段選取範圍）直接交給智慧體，Markdown、HTML、圖片與 Office 文件也都在同一面板內預覽。

**Git**：一個完整的用戶端，而不只是狀態顯示 —— 提交與推送、帶每筆提交推送狀態的歷史、新增分支、合併、變基、貯藏、重設，以及與另一個分支比較。遇到衝突會開啟三欄合併編輯器，逐塊採納或自己動手寫。而工作樹把平行開發壓縮成一個動作 —— 新分支、獨立目錄，外加一個紮根其中的新工作階段，於是一隊智慧體可以同時開發不同功能，誰也不碰誰的檔案。

## 📱 iPhone、iPad 與 Android

離開電腦，工作也不必停下。原生 iOS 與 Android 用戶端連接的就是你自己在跑的那個 Codeg —— 桌面應用的 **Web 服務**，或者你自己的 `codeg-server` —— 在手機上發起工作階段、看著回覆與工具呼叫即時流回、處理權限審核、瀏覽專案與分支。手機上不會多出任何東西：檔案、智慧體 CLI 與工作階段仍留在執行 Codeg 的那台機器上，存取權杖則交由 iOS Keychain 或 Android Keystore 保管。兩個用戶端皆已開源（[iOS](https://github.com/xintaofei/codeg-ios)、[Android](https://github.com/xintaofei/codeg-android)），目前處於測試階段；三個步驟即可完成配對，見 [行動應用](https://docs.codeg.app/zh/getting-started/installation#mobile-apps)。

| iPhone 與 iPad | Android |
| :---: | :---: |
| <img src="../images/mobile-ios.jpg" alt="在 Codeg iOS 用戶端中發起工作階段" width="248" /> | <img src="../images/mobile-android.jpg" alt="智慧體回覆即時流入 Codeg Android 用戶端" width="248" /> |

## ✨ 核心亮點

- **[對話聚合](https://docs.codeg.app/zh/guide/aggregation)** — 把所有支援的智慧體的工作階段匯入統一、可搜尋的工作區，並從上次中斷處繼續
- **[多智慧體協作](https://docs.codeg.app/zh/guide/multi-agent)** — `@` 提及任一智慧體即可委派：不同類型的子智慧體各自作為獨立工作階段，在同一個任務內平行執行
- **[工作區](https://docs.codeg.app/zh/guide/workspace)** — 智慧體旁邊就是完整的工程閉環：檔案樹、編輯器與 diff、Git 變更、提交，以及內建終端機
- **[Git 與 Worktree](https://docs.codeg.app/zh/guide/git)** — 檢視並提交變更、管理 Git 遠端帳號，用內建 `git worktree` 流程平行開發
- **[訊息渠道](https://docs.codeg.app/zh/guide/chat-channels)** — 在 Telegram、飛書、iLink（微信）裡直接驅動智慧體：建立任務、核准權限、即時接收進展
- **[自動化](https://docs.codeg.app/zh/guide/automations)** — 把設定好的輸入框存成可重複使用的自動化任務，依 cron 排程或手動觸發、無介面執行
- **[Office 文件](https://docs.codeg.app/zh/guide/office)** — 透過內建 `officecli` 建立、分析、校對與編輯 `.docx` / `.xlsx` / `.pptx`，並在分頁內即時預覽
- **[科學研究](https://docs.codeg.app/zh/guide/research)** — 內建科研技能（假設生成、實驗設計、統計、視覺化、批判性評估、文獻檢索），任一智慧體皆可呼叫
- **[專案啟動器](https://docs.codeg.app/zh/guide/project-boot)** — 視覺化建立新專案並即時預覽，建立完直接在工作區開啟
- **[MCP](https://docs.codeg.app/zh/guide/mcp) & [技能](https://docs.codeg.app/zh/guide/skills)** — 本機伺服器掃描 + 市集搜尋/安裝，技能支援全域與專案層級管理
- **[桌面端、伺服器與 Docker](https://docs.codeg.app/zh/getting-started/deployment)** — 原生桌面應用、可用瀏覽器存取的獨立 `codeg-server`，或者 `docker compose up`
- **[iPhone、iPad 與 Android](https://docs.codeg.app/zh/getting-started/installation#mobile-apps)** — 原生行動用戶端連接你的桌面端或伺服器：隨時隨地發起工作階段、接收串流回覆、核准權限、瀏覽專案

## 📦 安裝與執行

**桌面端** — 從 [Releases](https://github.com/xintaofei/codeg/releases) 下載 macOS、Windows 或 Linux 的安裝檔，再依 [安裝](https://docs.codeg.app/zh/getting-started/installation) 操作。

**伺服器** — 無介面執行 Codeg，用任意瀏覽器存取。Linux 或 macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
CODEG_STATIC_DIR=/usr/local/share/codeg/web codeg-server
```

Windows（PowerShell）：

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
$env:CODEG_STATIC_DIR="$env:LOCALAPPDATA\codeg\web"; codeg-server
```

**Docker** — 同一個伺服器，裝進一個容器：

```bash
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest
```

**行動裝置** — 安裝 [iOS 應用](https://apps.apple.com/app/codeg-client/id6785199071) 或 [Android APK](https://github.com/xintaofei/codeg-android/releases/latest)，再把它指向桌面應用的 **Web 服務**或你自己的 `codeg-server`：填位址、填權杖，完成。配對步驟見 [行動應用](https://docs.codeg.app/zh/getting-started/installation#mobile-apps)。

Compose、預編譯二進位檔、原始碼建置與就地升級見 [部署](https://docs.codeg.app/zh/getting-started/deployment)；環境變數見 [設定](https://docs.codeg.app/zh/getting-started/configuration)。想建置 Codeg 本身：[開發](https://docs.codeg.app/zh/reference/development) 與 [架構](https://docs.codeg.app/zh/reference/architecture)。

## 🔒 隱私與安全

- 預設本機優先：解析、儲存與專案操作都在本機完成 —— 僅在使用者主動觸發時才存取網路
- Web 模式與伺服器模式皆使用基於權杖的身分驗證
- 支援系統代理，適配企業網路環境

詳見 [隱私與安全](https://docs.codeg.app/zh/reference/privacy)。

## 👥 交流

- 掃描下方 QR Code 加入我們的微信群，參與討論、回饋與更新

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- 感謝 [LinuxDO](https://linux.do) 社群的支持

## 🙏 致謝

- [Agent Client Protocol](https://agentclientprotocol.com)：Codeg 得以連接所有支援的智慧體的基礎
- [Superpowers](https://github.com/obra/superpowers)：為 Codeg 的專家技能模組提供支援
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI)：為 Codeg 的 Office 文件工作流程提供支援
- [scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills)：為 Codeg 的科學研究技能提供支援（MIT 授權子集）

## 📜 授權

Apache-2.0，詳見 [LICENSE](../../LICENSE)。
