# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)
[![Docker](https://img.shields.io/badge/Docker-ready-2496ED)](../../Dockerfile)

<p>
  <a href="../../README.md">English</a> |
  <a href="./README.zh-CN.md">简体中文</a> |
  <a href="./README.zh-TW.md">繁體中文</a> |
  <strong>日本語</strong> |
  <a href="./README.ko.md">한국어</a> |
  <a href="./README.es.md">Español</a> |
  <a href="./README.de.md">Deutsch</a> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg（Code Generation）は、マルチエージェント・コーディングワークスペースです。Claude Code、Codex CLI、OpenCode、Gemini CLI、OpenClaw、Cline、Hermes Agent、CodeBuddy、Kimi Code、Pi、Grok Build、Cursor などの複数のエージェントを 1 つのワークスペースに統合し、会話の集約とマルチエージェント協働に対応します。デスクトップへのインストールに加え、サーバー/Docker デプロイにも対応しています。

![gallery](../images/gallery.svg)

## スポンサー

<table>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare（UCloud）</a></strong>
    </td>
    <td>本プロジェクトをスポンサードしてくださった Compshare に感謝します！Compshare は UCloud 傘下の AI クラウドプラットフォームで、月額制・従量制のコストパフォーマンスに優れた国内モデル agent Plan プランを提供しており、月額 49 元から利用可能です。安定した公式リダイレクトによる海外モデルへのアクセスも提供しています。Claude Code、Codex、API 連携に対応。企業向けの高並列対応、7×24 テクニカルサポート、セルフ請求書発行をサポートしています。<a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">こちらのリンク</a>から登録された方には、5 元分の無料プラットフォームクレジットが進呈されます！</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE" target="_blank"><img src="../images/sui-xiang.jpg" alt="随想AI中转站" width="200" /></a><br/>
      <strong><a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">随想AI中转站</a></strong>
    </td>
    <td>本プロジェクトをスポンサードしてくださった随想AI中转站に感謝します！随想AI中转站は、Claude、Codex、Gemini などの中継サービスを提供する、信頼性が高く効率的な API 中継サービスプロバイダーです。新規アカウントは<a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">登録</a>後、毎日のチェックインで 0.5 元のテストクレジットがもらえます。チャージは 1:1 で反映され、サブスクリプション不要の従量課金制です。複数回線の冗長化、リージョン間ディザスタリカバリ、自動フェイルオーバーにより、長時間の SSE 接続も途切れません。</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://hezu.ink/sign-up?aff=0wVz" target="_blank"><img src="../images/hezu-ink.jpg" alt="合租巴士" width="200" /></a><br/>
      <strong><a href="https://hezu.ink/sign-up?aff=0wVz">合租巴士</a></strong>
    </td>
    <td>本プロジェクトをスポンサードしてくださった合租巴士に感謝します！合租巴士は、Codex や Claude Code などの主流モデルに高い安定性の中継機能を提供する、信頼性が高く効率的な AI 中継サービスプラットフォームです。チャージ比率は透明（1:1）で、Codex のレート補助は 0.08 から利用可能です。<a href="https://hezu.ink/sign-up?aff=0wVz">公式サイトからグループに参加すると $5 分の体験クレジットがもらえます</a>。</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta" target="_blank"><img src="../images/onehop.jpg" alt="OneHop" width="120" /></a><br/>
      <strong><a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">OneHop</a></strong>
    </td>
    <td>本プロジェクトをスポンサードしてくださった OneHop に感謝します！OneHop を使えば、Codeg ユーザーは OpenAI 互換の API キー 1 つで、GPT、Claude、Gemini、DeepSeek、Kimi、Qwen など数百もの主要モデルを利用できます。複数のプロバイダーアカウントを管理したり、コードを何度も書き換えたりすることなくモデルを切り替えられ、使った分だけの従量課金です。<a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">Codeg 経由でサインアップ</a>すると $1 分のクレジットが付与され、さらに OneHop コミュニティに参加してウェルカムアクティビティに参加すると追加で $5 分——合計で最大 $6 分のテストクレジットを獲得できます。</td>
  </tr>
</table>

> Codeg のスポンサーになりませんか？[メールでお問い合わせください。](mailto:itpkcn@gmail.com)

## メインインターフェース

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## マルチエージェント協調

![Codeg Light](../images/collaboration-light.png#gh-light-mode-only)
![Codeg Dark](../images/collaboration-dark.png#gh-dark-mode-only)

## オフィスワークフロー

![Codeg Light](../images/office-light.png#gh-light-mode-only)
![Codeg Dark](../images/office-dark.png#gh-dark-mode-only)

## ハイライト

- **会話集約** — サポートされているすべてのエージェントのセッションを統合ワークスペースにインポート
- **マルチエージェント協調** — 同一セッション内で、メインエージェントが異なる種類のサブエージェント（例：Claude Code が Codex、Gemini などを呼び出し）を呼び出してタスクを共同で完了し、各サブエージェントは独立したセッションとして動作
- 内蔵 `git worktree` フローによる並列開発
- **プロジェクトブート** — ビジュアル設定とライブプレビューで新規プロジェクトを作成
- **Office ドキュメント** — 内蔵の officecli ツールセットで .docx / .xlsx / .pptx ファイルを作成・分析・校正・編集。ファイルタブ内でリアルタイムプレビューが可能で、エージェントの編集に合わせて即時更新
- **科学研究** — 科学系スキル（仮説生成、実験計画、統計、可視化、批判的評価、文献検索）を内蔵し、任意のエージェントから呼び出し可能。エージェントごとに管理
- **オートメーション** — 任意のコンポーザー設定を再利用可能なオートメーションとして保存し、cron スケジュールまたは手動トリガーでヘッドレス実行
- **チャットチャンネル** — Telegram、Lark（Feishu）、iLink（Weixin）などをコーディング Agent に接続し、リアルタイム通知の受信、フルセッション操作、リモートタスク制御を実行
- MCP 管理（ローカルスキャン + レジストリ検索/インストール）
- Skills 管理（グローバルおよびプロジェクトスコープ）
- Git リモートアカウント管理（GitHub およびその他の Git サーバー）
- Web サービスモード — ブラウザから Codeg にアクセスでき、リモートワークに対応
- **スタンドアロンサーバーデプロイ** — 任意の Linux/macOS サーバーで `codeg-server` を実行し、ブラウザからアクセス
- **Docker サポート** — `docker compose up` または `docker run` に対応、カスタムトークン・ポート設定、データ永続化およびプロジェクトディレクトリのマウントをサポート
- ランタイムログ — フィルタリングとモジュール別ログレベルに対応したリアルタイムログビューアを内蔵
- 統合エンジニアリングループ（ファイルツリー、Diff、Git 変更、コミット、ターミナル）

## 対応エージェント

| Agent        | 環境変数パス                          | macOS / Linux デフォルト              | Windows デフォルト                                    |
| ------------ | ------------------------------------- | ------------------------------------- | ----------------------------------------------------- |
| Claude Code  | `$CLAUDE_CONFIG_DIR/projects`         | `~/.claude/projects`                  | `%USERPROFILE%\\.claude\\projects`                    |
| Codex CLI    | `$CODEX_HOME/sessions`                | `~/.codex/sessions`                   | `%USERPROFILE%\\.codex\\sessions`                     |
| OpenCode     | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI   | `$GEMINI_CLI_HOME/.gemini`            | `~/.gemini`                           | `%USERPROFILE%\\.gemini`                              |
| OpenClaw     | —                                     | `~/.openclaw/agents`                  | `%USERPROFILE%\\.openclaw\\agents`                    |
| Cline        | `$CLINE_DIR`                          | `~/.cline/data/tasks`                 | `%USERPROFILE%\\.cline\\data\\tasks`                  |
| Hermes Agent | `$HERMES_HOME/state.db`               | `~/.hermes/state.db`                  | `%USERPROFILE%\\.hermes\\state.db`                    |
| CodeBuddy    | `$CODEBUDDY_CONFIG_DIR/projects`      | `~/.codebuddy/projects`               | `%USERPROFILE%\\.codebuddy\\projects`                 |
| Kimi Code    | `$KIMI_CODE_HOME/sessions`            | `~/.kimi-code/sessions`               | `%USERPROFILE%\\.kimi-code\\sessions`                 |
| Pi           | `$PI_CODING_AGENT_SESSION_DIR`        | `~/.pi/agent/sessions`                | `%USERPROFILE%\\.pi\\agent\\sessions`                 |
| Grok Build   | `$GROK_HOME/sessions`                 | `~/.grok/sessions`                    | `%USERPROFILE%\\.grok\\sessions`                      |
| Cursor       | `$CURSOR_CONFIG_DIR/chats`            | `~/.cursor/chats`                     | `%USERPROFILE%\\.cursor\\chats`                       |

> 注: 環境変数はフォールバックパスより優先されます。

<details>
<summary><h2>プロジェクトブート</h2></summary>

分割ペインインターフェースで新規プロジェクトをビジュアルに作成：左側で設定、右側でリアルタイムプレビュー。

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### 主な機能

- **ビジュアル設定** — ドロップダウンからスタイル、カラーテーマ、アイコンライブラリ、フォント、角丸などを選択でき、プレビューが即座に更新
- **ライブプレビュー** — プロジェクト作成前に、選んだルック＆フィールをリアルタイムで確認
- **ワンクリック作成** — 「プロジェクト作成」をクリックすると、プリセット設定、フレームワークテンプレート（Next.js / Vite / React Router / Astro / Laravel）、パッケージマネージャー（pnpm / npm / yarn / bun）で `shadcn init` を実行
- **パッケージマネージャー検出** — インストール済みのパッケージマネージャーを自動検出し、バージョンを表示
- **シームレスな統合** — 新規作成されたプロジェクトは、すぐに Codeg のワークスペースで開きます

現在 **shadcn/ui** プロジェクトのスキャフォールディングをサポートしており、タブベースの設計で将来のプロジェクトタイプ追加に対応しています。

</details>

<details>
<summary><h2>チャットチャンネル</h2></summary>

お気に入りのメッセージングアプリ — Telegram、Lark（Feishu）、iLink（Weixin）など — を AI コーディング Agent に接続。チャットからタスクの作成、フォローアップメッセージの送信、権限の承認、セッションの再開、アクティビティの監視が可能です。Agent のレスポンスはツールコール詳細、権限プロンプト、完了サマリーとともにリアルタイムで受信 — ブラウザを開くことなくすべて対応可能。

Telegram のフォーラムスーパーグループでは [Telegram topic mode](../chat-channels/telegram-topic-mode.md) を使い、各 topic を独立した Codeg セッションに紐付けられます。

### 対応チャンネル

| チャンネル      | プロトコル                       | 状態 |
| --------------- | -------------------------------- | ---- |
| Telegram        | Bot API（HTTP ロングポーリング） | 内蔵 |
| Lark（Feishu）  | WebSocket + REST API             | 内蔵 |
| iLink（Weixin） | WebSocket + REST API             | 内蔵 |

> その他のチャンネル（Discord、Slack、DingTalk など）は今後のリリースで対応予定。

</details>

<details>
<summary><h2>Office ドキュメント</h2></summary>

Word、Excel、PowerPoint ファイルをファーストクラスのワークフローとして扱えます。内蔵の **officecli** ツールセットにより、エージェントが .docx、.xlsx、.pptx ドキュメントの作成・分析・校正・編集を行い、Codeg 内で直接プレビューできます。

### 機能

- **作成・編集** — 新規ドキュメントの生成や既存 .docx / .xlsx / .pptx ファイルの編集（グラフ、表、書式設定を含む）
- **分析・校正** — ドキュメント構造の確認、書式の問題の発見、内容の校正
- **ライブプレビュー** — ファイルタブで .docx / .xlsx / .pptx を開くとインライン表示され、エージェントの編集に合わせて自動更新——常駐の `officecli watch` サーバーが支え（Web およびスタンドアロンサーバー環境ではリバースプロキシ経由で配信、ケイパビリティ認証）
- **クイックアクション** — ウェルカムページの「コーディング」「Office」「科学研究」タブから、対応するスキル呼び出しとプロンプトテンプレートをワンクリックで入力欄に挿入；選択中のエージェントで有効化されていないスキルはロックバッジで表示され、有効化画面へ誘導
- **Office ツール設定** — 専用設定ページで `officecli` のインストールとドキュメントスキルをスキル×エージェントマトリクスで管理：任意の（スキル、エージェント）ペアを切り替え、一括で有効化/無効化も可能

</details>

<details>
<summary><h2>科学研究</h2></summary>

任意のエージェントを、厳密なリサーチアシスタントに変えます。Codeg には、着想から分析、執筆までをカバーする、MIT ライセンスで厳選された一連の**科学研究スキル**が同梱されています。これらはエキスパートや Office のツールセットとまったく同じように、共有の中央スキルストアにインストールされ、選択した任意のエージェントにリンクされます。

### 機能

- **厳選スキル** — 仮説生成、実験計画、統計的検出力、統計解析、探索的データ解析、科学的可視化、批判的評価、査読、引用管理、研究者評価、論文検索、AI 模式図
- **クイックアクション** — ウェルカムページの「科学研究」タブから、対応するスキル呼び出しとローカライズされたプロンプトテンプレートをワンクリックで入力欄に挿入
- **科学研究設定** — 専用設定ページで、スキルをスキル×エージェントマトリクスで管理。API キーや Python 環境が必要なスキルにはバッジを表示

</details>

<details>
<summary><h2>オートメーション</h2></summary>

コンポーザーの設定——エージェント、モデル、プロンプト、作業ディレクトリ、オプション——を再利用可能な**オートメーション**として保存し、UI を開かずに実行できます。

### 機能

- **一度設定すれば再利用可能** — 完全なコンポーザー設定を名前付きオートメーションとして保存
- **スケジュール実行またはオンデマンド** — cron スケジュールで自動実行するか、任意のタイミングで手動トリガー
- **ヘッドレス実行** — バックグラウンドで実行され、通常のセッションを生成。ワークスペースからいつでも開くことができ、起動後はワークスペースに自動で戻る

</details>

<details>
<summary><h2>クイックスタート</h2></summary>

### 必要条件

- Node.js `>=22`（推奨）
- pnpm `>=10`
- Rust stable（2021 edition）
- Tauri 2 ビルド依存パッケージ（デスクトップモードのみ）

Linux（Debian/Ubuntu）の例:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### バイナリ

Codeg は単一の workspace から 3 つの Rust バイナリを提供します:

| バイナリ       | 役割                                                                                              | ビルド                                                                       |
| -------------- | ------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `codeg`        | Tauri デスクトップアプリ（ウィンドウ、トレイ、自動更新）                                          | `pnpm tauri build`（リリース）/ `pnpm tauri dev`（開発）                     |
| `codeg-server` | ブラウザ/ヘッドレスデプロイ向けスタンドアロン HTTP + WebSocket サーバー                           | `pnpm server:build` / `pnpm server:dev`                                      |
| `codeg-mcp`    | 起動ごとの stdio MCP コンパニオン。agent CLI に `delegate_to_agent` ツールを公開（マルチエージェント協調用） | `pnpm tauri:prepare-sidecars`（`tauri dev` / `tauri build` から自動呼び出し）|

`codeg-mcp` は実行時に親バイナリと同じディレクトリに配置されている必要があります — インストーラ、Docker イメージ、Tauri sidecar バンドラはすべて `codeg` / `codeg-server` の隣に配置します。ソースビルドやカスタム配置では、`CODEG_MCP_BIN=/abs/path/codeg-mcp` 環境変数で検索パスを上書きできます。コンパニオンが見つからない場合、デリゲートはスキップされ（警告ログが 1 行記録されます）、agent セッションの他の部分は引き続き動作します。

### 開発

```bash
pnpm install

# フロントエンドのみ（Next.js 開発サーバー、Rust 不要）
pnpm dev

# フロントエンド静的エクスポート（out/ へ）
pnpm build

# デスクトップアプリ全体（Tauri + Next.js、codeg-mcp sidecar を自動ビルド）
pnpm tauri dev

# デスクトップリリースビルド（codeg-mcp を externalBin としてバンドル）
pnpm tauri build

# スタンドアロンサーバー（Tauri/GUI 不要）
pnpm server:dev
pnpm server:build                  # リリースバイナリは src-tauri/target/release/codeg-server

# codeg-mcp コンパニオンを明示的にビルド（ホストトリプル向け）
pnpm tauri:prepare-sidecars        # 出力: src-tauri/binaries/codeg-mcp-<triple>

# フロントエンドのイテレーション中でデリゲートが不要な場合に sidecar 準備をスキップ
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# フロントエンドテスト (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# Rust チェック（src-tauri/ で実行）
cargo check                                                     # デスクトップ（デフォルト features）
cargo check --no-default-features --bin codeg-server            # サーバーモード
cargo check --no-default-features --bin codeg-mcp               # MCP コンパニオン
cargo clippy --all-targets --features test-utils -- -D warnings

# Rust テスト
cargo test --features test-utils                                # デスクトップ（統合テスト含む）
cargo test --no-default-features --bin codeg-server --lib       # サーバーモード
cargo insta review                                              # パーサスナップショットの更新を受理
```

> ヒント: `src-tauri/target/release/` に新しい `codeg-mcp` ビルドがあり、再インストールせずに手動起動の `codeg-server` をそこに向けたい場合は、`CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp` をエクスポートしてください。

### サーバーデプロイ

Codeg はデスクトップ環境なしでスタンドアロン Web サーバーとして実行できます。

#### オプション 1: ワンラインインストール（Linux / macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

特定のバージョンまたはカスタムディレクトリにインストール:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

実行:

```bash
codeg-server
```

#### オプション 2: ワンラインインストール（Windows PowerShell）

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

または特定のバージョンをインストール:

```powershell
.\install.ps1 -Version v0.5.2
```

#### オプション 3: GitHub Releases からダウンロード

ビルド済みバイナリ（Web アセットをバンドル済み）は [Releases](https://github.com/xintaofei/codeg/releases) ページからダウンロードできます:

| プラットフォーム | ファイル                           |
| ---------------- | ---------------------------------- |
| Linux x64        | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64      | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64        | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64      | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64      | `codeg-server-windows-x64.zip`     |

```bash
# 例: ダウンロード、解凍、実行
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

> 無人デプロイでは、`--supervise` を付けて起動すると、インプレース更新が失敗した場合に自動的にロールバックされます — [インプレース更新](#インプレース更新)を参照してください。

#### オプション 4: Docker

```bash
# Docker Compose を使用（推奨）
docker compose up -d

# または Docker で直接実行
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# カスタムトークンとプロジェクトディレクトリのマウント
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

Docker イメージはマルチステージビルド（Node.js + Rust → 軽量 Debian ランタイム）を使用し、リポジトリ操作用の `git` と `ssh` を含みます。データは `/data` ボリュームに永続化されます。オプションでプロジェクトディレクトリをマウントして、コンテナ内からローカルリポジトリにアクセスできます。

#### オプション 5: ソースからビルド

```bash
pnpm install && pnpm build          # フロントエンドをビルド
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # デリゲートコンパニオン
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp は同階層のバイナリとして検出されます
```

> 2 つのバイナリを別々のディレクトリに置く場合は、`CODEG_MCP_BIN=/abs/path/to/codeg-mcp` を設定して、ランタイムからコンパニオンを見つけられるようにしてください。設定しない場合、マルチエージェントのデリゲートはサイレントに無効化されます。

#### インプレース更新

サーバーは **設定 → ソフトウェア更新** から自身を更新できます。プラットフォーム向けの署名済みリリースをダウンロードし、ディスク上のバイナリと Web アセットを差し替えて再起動します — 手動での再デプロイは不要です。これは Linux/macOS のみ対応です（Windows では無効）。以前のバージョンはバックアップとして保持されるため、同じ画面から **ロールバック** 操作で元に戻せます。

**自動ロールバックにはスーパーバイザー配下で実行してください。** スタンドアロンサーバーを `--supervise` を付けて起動すると、更新直後のプロセスが試用期間内に起動できなかった場合、自動的に以前のバージョンに戻されます:

```bash
CODEG_STATIC_DIR=./web ./codeg-server --supervise
```

`--supervise` なしでもサーバーはインプレースで更新されます（自身を再 exec します）が、更新はベストエフォートです。起動できないバージョンを自動的にロールバックするスーパーバイザーは存在しません。Docker イメージはすでにスーパーバイザー配下で実行されています。

**Docker の更新はイメージではなくコンテナを変更します。** インプレース更新は、実行中コンテナの書き込み可能レイヤー内のバイナリと Web アセットを書き換えるため、それらはそのコンテナ内にのみ存在します。`/data` ボリュームは永続化されますが、更新されたファイルは永続化され**ません**。コンテナを再作成すると——`docker compose up --force-recreate`、新規の `docker run`、または `docker pull` 後の再作成——再びイメージから開始され、インプレース更新は破棄されます。（`docker pull` 単体ではローカルイメージを更新するだけで、コンテナを再作成するまで何も戻りません。）更新を恒久化するには、新しいバージョンのイメージをビルドまたはプルし、そこからコンテナを再作成してください。

#### 設定

環境変数:

| 変数                           | デフォルト             | 説明                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| ------------------------------ | ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | HTTP ポート                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `CODEG_HOST`                   | `0.0.0.0`              | バインドアドレス                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `CODEG_TOKEN`                  | _(ランダム)_           | 認証トークン（起動時に stderr に出力）                                                                                                                                                                                                                                                                                                                                                                                                          |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | SQLite データベースディレクトリ（`uploads/`、`pets/` のルートも兼ねる）                                                                                                                                                                                                                                                                                                                                                                         |
| `CODEG_STATIC_DIR`             | `./web` または `./out` | Next.js 静的エクスポートディレクトリ                                                                                                                                                                                                                                                                                                                                                                                                            |
| `CODEG_MCP_BIN`                | _（未設定）_           | `codeg-mcp` コンパニオンの絶対パス。デフォルトの「実行ファイルと同階層 + `PATH`」検索を上書きします。コンパニオンがサーバーのインストールディレクトリ外にあるソースビルドやカスタム配置で使用します。                                                                                                                                                                                                                                              |
| `CODEG_SKIP_SIDECAR`           | _（未設定）_           | `pnpm tauri dev` / `pnpm tauri build` でのフロントエンド作業向け — `1` を指定すると `codeg-mcp` sidecar のビルドをスキップします。そのビルドではデリゲートが無効になります。出荷品質の成果物では未設定のままにしてください。                                                                                                                                                                                                                      |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _（未設定）_           | `<data dir>/uploads/` 配下に存在するファイルの合計バイト数のハードキャップ。10進数のバイト数（例: `10737418240` で 10 GiB）。未設定、`0`、または解析できない値の場合、キャップは無効になり、起動時に現在の状態が分かるログ行を出力します。このキャップは単一の `codeg-server` プロセス内でのみ強制されます——同じ `uploads/` ボリュームを共有する水平スケール構成では、外部協調（ファイルロック、Redis、リバースプロキシのクォータ）が必要です。 |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _（未設定）_           | 真値（`1` / `true` / `yes` / `on`）の場合、`CODEG_UPLOAD_MAX_TOTAL_BYTES` が解析できない値に設定されているときに、WARN を出して fail-open するのではなく、終了コード 2 で起動を中断します。セキュリティポリシーで「設定されたクォータは有効でなければならない」と要求される場合に使用します。                                                                                                                                                   |

</details>

<details>
<summary><h2>アーキテクチャ</h2></summary>

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
              |- Office Tools (officecli) + Automations
              |- SeaORM + SQLite
                      |
              ┌───────┼───────┐
              v       v       v
  Local Filesystem  Git   Chat Channels
    / Git Repos    Repos  (Telegram, Lark, iLink)
```

</details>

## プライバシーとセキュリティ

- 解析、ストレージ、プロジェクト操作はデフォルトでローカルファースト
- ネットワークアクセスはユーザーが明示的に操作した場合のみ発生
- エンタープライズ環境向けのシステムプロキシサポート
- Web サービスモードではトークンベースの認証を使用

## コミュニティ

- QRコードをスキャンして、ディスカッション、フィードバック、アップデートのための WeChat グループに参加してください

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- [LinuxDO](https://linux.do) コミュニティのサポートに感謝します

## 謝辞

- [ACP](https://agentclientprotocol.com) — Agent Client Protocol (ACP) は、Codeg が複数のエージェントに接続できる基盤です
- [Superpowers](https://github.com/obra/superpowers) — Codeg のエキスパートスキルモジュールを支えるプロジェクト
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — Codeg の Office ドキュメントワークフローを支えるプロジェクト
- [scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills) — Codeg の科学研究スキルを支えるプロジェクト（MIT ライセンスのサブセット）

## ライセンス

Apache-2.0。`LICENSE` を参照してください。
