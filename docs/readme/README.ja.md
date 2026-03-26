# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)

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

Codeg（Code Generation）は、エンタープライズ級のマルチ Agent コーディングワークスペースです。
Claude Code、Codex CLI、OpenCode、Gemini CLI、OpenClaw などのローカル AI コーディング Agent を
デスクトップアプリと Web サービスに統合し——ブラウザからどこでもリモート開発が可能——セッション集約、並列 `git worktree` 開発、MCP/Skills 管理、
Git/ファイル/ターミナル連携ワークフローを提供します。

## メインインターフェース
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## セッションタイル表示
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> 現在のステータス: `v0.2.x`（高速イテレーション中、アーリーアダプター向け）

## ハイライト

- 同一プロジェクトでのマルチ Agent 統合ワークスペース
- ローカルセッションの取り込みと構造化レンダリング
- 内蔵 `git worktree` フローによる並列開発
- MCP 管理（ローカルスキャン + レジストリ検索/インストール）
- Skills 管理（グローバルおよびプロジェクトスコープ）
- Git リモートアカウント管理（GitHub およびその他の Git サーバー）
- Web サービスモード — ブラウザから Codeg にアクセスでき、リモートワークに対応
- 統合エンジニアリングループ（ファイルツリー、Diff、Git 変更、コミット、ターミナル）

## 対応範囲

### 1) セッション取り込み（履歴セッション）

| Agent | 環境変数パス | macOS / Linux デフォルト | Windows デフォルト |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> 注: 環境変数はフォールバックパスより優先されます。

### 2) ACP リアルタイムセッション

現在、Claude Code、Codex CLI、Gemini CLI、OpenCode、OpenClaw の 5 つのエージェントをサポートしています。

### 3) Skills 設定サポート

- 対応済み: `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- 他のアダプターは順次追加予定

### 4) MCP ターゲットアプリ

現在の書き込み対象:

- Claude Code
- Codex
- OpenCode

## クイックスタート

### 必要条件

- Node.js `>=22`（推奨）
- pnpm `>=10`
- Rust stable（2021 edition）
- Tauri 2 ビルド依存パッケージ

Linux（Debian/Ubuntu）の例:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### 開発

```bash
pnpm install

# デスクトップアプリ全体（Tauri + Next.js）
pnpm tauri dev

# フロントエンドのみ
pnpm dev

# フロントエンド静的エクスポート（out/ へ）
pnpm build

# デスクトップビルド
pnpm tauri build

# Lint
pnpm eslint .

# Rust チェック（src-tauri/ で実行）
cargo check
cargo clippy
cargo build
```

## アーキテクチャ

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

## 制約事項

- フロントエンドは静的エクスポートを使用（`output: "export"`）
- Next.js の動的ルート（`[param]`）は不可。代わりにクエリパラメータを使用
- Tauri コマンドパラメータ: フロントエンドは `camelCase`、Rust は `snake_case`
- TypeScript strict モード

## プライバシーとセキュリティ

- 解析、ストレージ、プロジェクト操作はデフォルトでローカルファースト
- ネットワークアクセスはユーザーが明示的に操作した場合のみ発生
- エンタープライズ環境向けのシステムプロキシサポート

## ライセンス

Apache-2.0。`LICENSE` を参照してください。
