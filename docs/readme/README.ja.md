# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![Docs](https://img.shields.io/badge/docs-docs.codeg.app-3451b2)](https://docs.codeg.app)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)

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

Codeg（Code Generation）はマルチエージェント・コーディングワークスペースです。あらゆる AI コーディングエージェントをひとつの場所で動かし、そして協働させます。

対応するすべてのエージェント CLI のセッションを検索可能なワークスペースへ集約し、ひとつのタスクの中でメインエージェントが別種類のサブエージェントへ委譲でき、デスクトップアプリ・スタンドアロンサーバー・Docker コンテナのいずれとしても動作します。さらに、ネイティブの iOS / Android クライアントがあるので、デスクを離れても作業を続けられます。

![ワークスペース](../images/workspace-light.png#gh-light-mode-only)
![ワークスペース](../images/workspace-dark.png#gh-dark-mode-only)

## 📖 ドキュメント

**完全なドキュメントは [docs.codeg.app](https://docs.codeg.app)** — [はじめに](https://docs.codeg.app/getting-started/) · [ガイド](https://docs.codeg.app/guide/) · [リファレンス](https://docs.codeg.app/reference/)

## 💖 スポンサー

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

## 🤖 対応エージェント

Claude Code · Codex · Gemini · OpenClaw · OpenCode · Cline · Hermes · CodeBuddy · Kimi Code · Pi · Grok · Cursor

その多くは Codeg がインストール・バージョン固定・更新まで面倒を見ます。全リスト、各エージェントの実行環境要件、セッションの保存場所は [対応エージェント](https://docs.codeg.app/guide/supported-agents) を参照してください。

## 🤝 マルチエージェント協調

マルチエージェント協調は、キーひとつで完結します。`@` を打ち、エージェントを選び、送信するだけ。あとのスケジューリングは Codeg が引き受けます — 指名されたエージェントをそれぞれ独立したセッションとして起動し、タスクを引き渡し、その作業を今いるスレッドへ流し込みます。ふたつ指名すれば並走します。Claude Code が下書きし、Codex がレビューする。コンテキストの切り替えも、ターミナル間のコピー＆ペーストも不要です。

![ひとつの Codeg 会話からサブエージェントへタスクを委譲する様子](../images/collaboration-light.gif#gh-light-mode-only)
![ひとつの Codeg 会話からサブエージェントへタスクを委譲する様子](../images/collaboration-dark.gif#gh-dark-mode-only)

## 📄 Office ドキュメント

スライドでも、レポートでも、表計算でも、頼めばエージェントは本物の `.pptx` / `.docx` / `.xlsx` を作ります — 右側のペインがそれをリアルタイムに描画しながら。編集は自動でプレビューへ反映され、スライドが埋まり、表が形になり、数値がセルに収まっていきます。4 枚目が気に入らない？次のメッセージでそう伝えるだけ — エージェントは同じファイルをその場で直し、プレビューが追いつきます。書き出しも、外部の Office アプリも、Codeg を離れる必要もありません。

![ライブプレビューを横に置いて Office ドキュメントを編集するエージェント](../images/office-light.png#gh-light-mode-only)
![ライブプレビューを横に置いて Office ドキュメントを編集するエージェント](../images/office-dark.png#gh-dark-mode-only)

## 💻 ワークスペース

ワークスペースはひとつ、エージェントはすべて。動かしているのが Claude Code でも Codex でも Cursor でも、同じエディタ、同じライブ diff、同じ Git クライアントの中で作業します。そして生まれるのはリポジトリの中の本物のファイル — 目の前で変わっていきます。

**セッション**：すでにある履歴をそのまま引き継げます。インストール済みのすべてのエージェントの過去セッションをワンクリックで取り込み、中断したところから再開できます。取り込んだ後は、もう互いに孤立したままではありません — 古いセッションを `@` で指名すれば、いま話しているエージェントがそれを読めます。別のエージェントが書いたものでも構いません。今日の Codex が、先週の Claude Code が終えたところから続けられます。

**ファイル**：エージェントの編集は、着地するそばから会話の隣に diff として現れます。どのファイルもシンタックスハイライト付きの本物のエディタで開け、`⌘L` でファイルを — あるいは選択範囲だけを — そのままエージェントへ渡せます。Markdown、HTML、画像、Office ドキュメントも同じペインでプレビューできます。

**Git**：状態表示ではなく、完全なクライアントです。コミットとプッシュ、コミットごとのプッシュ状態が分かる履歴、ブランチ、マージ、リベース、スタッシュ、リセット、別ブランチとの差分。コンフリクトは三ペインのマージエディタで開き、ハンク単位で採用するか自分で書きます。そして worktree は並行作業をワンアクションに変えます — 新しいブランチ、専用のディレクトリ、そこに根を張った新しい会話。エージェントの一隊が互いのファイルに触れることなく、別々の機能を同時に作れます。

## 📱 iPhone・iPad・Android

デスクを離れても、作業は止まりません。ネイティブの iOS / Android クライアントは、あなたがすでに動かしている Codeg —— デスクトップアプリの **Web サービス**、あるいは自分で立てた `codeg-server` —— に接続します。そこからセッションを開始し、返信やツール呼び出しが流れ込むのを追い、権限の確認に答え、プロジェクトやブランチを見て回れます。端末側には何も移りません。ファイルもエージェント CLI も会話も Codeg を実行しているマシンに残り、アクセストークンは iOS Keychain または Android Keystore が預かります。どちらのクライアントもオープンソース（[iOS](https://github.com/xintaofei/codeg-ios)、[Android](https://github.com/xintaofei/codeg-android)）で、現在はテスト版です。接続はわずか 3 ステップ、詳しくは [モバイルアプリ](https://docs.codeg.app/getting-started/installation#mobile-apps)。

| iPhone・iPad | Android |
| :---: | :---: |
| <img src="../images/mobile-ios.jpg" alt="Codeg iOS クライアントでセッションを開始する画面" width="248" /> | <img src="../images/mobile-android.jpg" alt="Codeg Android クライアントに流れ込むエージェントの返信" width="248" /> |

## ✨ ハイライト

- **[会話の集約](https://docs.codeg.app/guide/aggregation)** — 対応するすべてのエージェントのセッションを統一された検索可能なワークスペースへ取り込み、中断した続きから再開できます
- **[マルチエージェント協調](https://docs.codeg.app/guide/multi-agent)** — `@` でエージェントを指名するだけで委譲。異なる種類のサブエージェントがそれぞれ独立したセッションとして、ひとつのタスク内で並行して動きます
- **[ワークスペース](https://docs.codeg.app/guide/workspace)** — エージェントの隣に開発の一連の流れがすべて揃います：ファイルツリー、エディタと diff、Git の変更、コミット、内蔵ターミナル
- **[Git と Worktree](https://docs.codeg.app/guide/git)** — 変更のレビューとコミット、Git リモートアカウントの管理、内蔵の `git worktree` フローによる並行開発
- **[チャットチャンネル](https://docs.codeg.app/guide/chat-channels)** — Telegram、Lark（飛書）、iLink（微信）からエージェントを操作：タスク作成、権限の承認、進捗のリアルタイム受信
- **[オートメーション](https://docs.codeg.app/guide/automations)** — 設定済みの入力欄を再利用可能なオートメーションとして保存し、cron スケジュールまたは任意のタイミングでヘッドレス実行
- **[Office ドキュメント](https://docs.codeg.app/guide/office)** — 同梱の `officecli` で `.docx` / `.xlsx` / `.pptx` を作成・分析・校正・編集し、タブ内でライブプレビュー
- **[科学研究](https://docs.codeg.app/guide/research)** — 同梱の研究スキル（仮説生成、実験計画、統計、可視化、批判的吟味、文献検索）をどのエージェントからも呼び出せます
- **[プロジェクトブート](https://docs.codeg.app/guide/project-boot)** — ライブプレビュー付きで新規プロジェクトを視覚的に構築し、そのままワークスペースで開きます
- **[MCP](https://docs.codeg.app/guide/mcp) & [スキル](https://docs.codeg.app/guide/skills)** — ローカルスキャンとレジストリ検索/インストール、スキルはグローバル／プロジェクト単位で管理
- **[デスクトップ・サーバー・Docker](https://docs.codeg.app/getting-started/deployment)** — ネイティブなデスクトップアプリ、ブラウザから使えるスタンドアロンの `codeg-server`、あるいは `docker compose up`
- **[iPhone・iPad・Android](https://docs.codeg.app/getting-started/installation#mobile-apps)** — デスクトップやサーバーに接続するネイティブモバイルクライアント：どこからでもセッションを開始し、返信をストリーミングで受け取り、権限を承認し、プロジェクトを閲覧

## 📦 インストールと実行

**デスクトップ** — macOS・Windows・Linux 向けインストーラーを [Releases](https://github.com/xintaofei/codeg/releases) から入手し、[インストール](https://docs.codeg.app/getting-started/installation) の手順に従ってください。

**サーバー** — Codeg をヘッドレスで動かし、任意のブラウザから利用します。Linux / macOS の場合：

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
CODEG_STATIC_DIR=/usr/local/share/codeg/web codeg-server
```

Windows（PowerShell）の場合：

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
$env:CODEG_STATIC_DIR="$env:LOCALAPPDATA\codeg\web"; codeg-server
```

**Docker** — 同じサーバーを、ひとつのコンテナで：

```bash
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest
```

**モバイル** — [iOS アプリ](https://apps.apple.com/app/codeg-client/id6785199071) または [Android APK](https://github.com/xintaofei/codeg-android/releases/latest) をインストールし、デスクトップアプリの **Web サービス**か自分の `codeg-server` を指定するだけ：アドレスとトークンを入れれば完了です。接続手順は [モバイルアプリ](https://docs.codeg.app/getting-started/installation#mobile-apps)。

Compose、ビルド済みバイナリ、ソースからのビルド、その場での更新は [デプロイ](https://docs.codeg.app/getting-started/deployment) に、環境変数は [設定](https://docs.codeg.app/getting-started/configuration) にあります。Codeg 自体のビルドは [開発](https://docs.codeg.app/reference/development) と [アーキテクチャ](https://docs.codeg.app/reference/architecture) を参照。

## 🔒 プライバシーとセキュリティ

- 解析・保存・プロジェクト操作はデフォルトでローカル優先 — ネットワークアクセスはユーザーが起点となった操作でのみ発生します
- Web モードとサーバーモードはトークンベースの認証で保護されます
- 企業環境向けにシステムプロキシに対応

詳細は [プライバシーとセキュリティ](https://docs.codeg.app/reference/privacy) を参照してください。

## 👥 コミュニティ

- QRコードをスキャンして、ディスカッション、フィードバック、アップデートのための WeChat グループに参加してください

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- [LinuxDO](https://linux.do) コミュニティのサポートに感謝します

## 🙏 謝辞

- [Agent Client Protocol](https://agentclientprotocol.com) — Codeg が対応するすべてのエージェントへ接続できる土台
- [Superpowers](https://github.com/obra/superpowers) — Codeg のエキスパートスキルモジュールを支えるプロジェクト
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — Codeg の Office ドキュメントワークフローを支えるプロジェクト
- [scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills) — Codeg の科学研究スキルを支えるプロジェクト（MIT ライセンスのサブセット）

## 📜 ライセンス

Apache-2.0。[LICENSE](../../LICENSE) を参照してください。
