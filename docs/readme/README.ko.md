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
  <a href="./README.ja.md">日本語</a> |
  <strong>한국어</strong> |
  <a href="./README.es.md">Español</a> |
  <a href="./README.de.md">Deutsch</a> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg(Code Generation)는 멀티 에이전트 코딩 워크스페이스입니다. Claude Code, Codex CLI, OpenCode, Gemini CLI, OpenClaw, Cline, Hermes Agent, CodeBuddy, Kimi Code 등의 여러 에이전트를 하나의 워크스페이스로 통합하며, 대화 집계와 멀티 에이전트 협업을 지원하고 데스크톱 설치와 서버/Docker 배포를 지원합니다.

![gallery](../images/gallery.svg)

## 스폰서

<table>
  <tr>
    <td colspan="2" align="center">
      <a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg" target="_blank"><img src="https://raw.githubusercontent.com/LeoYeAI/myclaw-sponsor-preview/main/banner.svg" alt="MyClaw.ai — Your OpenClaw Agent, Always On." /></a><br/>
      <strong><a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg">MyClaw.ai</a></strong> — 완전관리형 OpenClaw 클라우드 인스턴스 서비스. 원클릭 배포, 24/7 상시 운영, 데이터 완전 소유권 보장 — 서버를 직접 관리할 필요가 없습니다.
    </td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare(UCloud)</a></strong>
    </td>
    <td>본 프로젝트를 후원해 주신 Compshare에 감사드립니다! Compshare는 UCloud 산하의 AI 클라우드 플랫폼으로, 월정액·종량제 방식의 가성비 높은 국내 모델 agent Plan 요금제를 월 49위안부터 제공합니다. 또한 안정적인 공식 프록시 방식의 해외 모델 접근도 지원합니다. Claude Code, Codex 및 API 연동을 지원하며, 기업 환경의 높은 동시성, 7×24 기술 지원, 셀프 인보이스 발급도 지원합니다. <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">이 링크</a>를 통해 가입하시면 무료 5위안 플랫폼 체험 크레딧을 받으실 수 있습니다!</td>
  </tr>
</table>

> Codeg의 스폰서가 되고 싶으신가요? [이메일로 문의해 주세요.](mailto:itpkcn@gmail.com)

## 메인 인터페이스

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## 멀티 에이전트 협업

![Codeg Light](../images/collaboration-light.png#gh-light-mode-only)
![Codeg Dark](../images/collaboration-dark.png#gh-dark-mode-only)

## 오피스 워크플로우

![Codeg Light](../images/office-light.png#gh-light-mode-only)
![Codeg Dark](../images/office-dark.png#gh-dark-mode-only)

## 하이라이트

- **세션 통합** — 지원되는 모든 에이전트의 세션을 통합 워크스페이스로 가져오기
- **멀티 에이전트 협업** — 단일 세션 내에서 메인 에이전트가 다양한 유형의 서브 에이전트(예: Claude Code가 Codex, Gemini 등을 호출)를 호출하여 함께 작업을 완료하며, 각 서브 에이전트는 독립된 세션으로 실행
- 내장 `git worktree` 플로를 통한 병렬 개발
- **프로젝트 부트** — 시각적 설정과 실시간 미리보기로 새 프로젝트 생성
- **Office 문서** — 내장 officecli 툴셋으로 .docx / .xlsx / .pptx 파일 생성, 분석, 교정, 편집. 파일 탭 내 실시간 미리보기 지원, 에이전트 편집 시 즉시 갱신
- **자동화** — 컴포저 설정을 재사용 가능한 자동화로 저장하고, cron 스케줄 또는 수동 트리거로 헤드리스 실행
- **채팅 채널** — Telegram, Lark(Feishu), iLink(Weixin) 등을 코딩 에이전트에 연결하여 실시간 알림 수신, 전체 세션 상호작용 및 원격 작업 제어
- MCP 관리 (로컬 스캔 + 레지스트리 검색/설치)
- Skills 관리 (글로벌 및 프로젝트 범위)
- Git 원격 계정 관리 (GitHub 및 기타 Git 서버)
- Web 서비스 모드 — 브라우저에서 Codeg에 접속하여 원격 작업 가능
- **독립형 서버 배포** — 모든 Linux/macOS 서버에서 `codeg-server`를 실행하고 브라우저로 접속
- **Docker 지원** — `docker compose up` 또는 `docker run` 지원, 사용자 정의 토큰/포트, 데이터 영속화 및 프로젝트 디렉토리 마운트 지원
- 런타임 로그 — 필터링 및 모듈별 로그 레벨 설정을 지원하는 실시간 로그 뷰어 내장
- 통합 엔지니어링 루프 (파일 트리, Diff, Git 변경사항, 커밋, 터미널)

## 지원 에이전트

| Agent        | 환경 변수 경로                        | macOS / Linux 기본값                  | Windows 기본값                                        |
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

> 참고: 환경 변수가 기본 경로보다 우선합니다.

<details>
<summary><h2>프로젝트 부트</h2></summary>

분할 패널 인터페이스로 새 프로젝트를 시각적으로 생성: 왼쪽에서 설정, 오른쪽에서 실시간 미리보기.

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### 주요 기능

- **시각적 설정** — 드롭다운에서 스타일, 색상 테마, 아이콘 라이브러리, 글꼴, 테두리 반경 등을 선택하면 미리보기가 즉시 업데이트
- **실시간 미리보기** — 프로젝트 생성 전에 선택한 룩앤필을 실시간으로 확인
- **원클릭 생성** — "프로젝트 생성"을 클릭하면 프리셋 설정, 프레임워크 템플릿(Next.js / Vite / React Router / Astro / Laravel), 패키지 매니저(pnpm / npm / yarn / bun)로 `shadcn init` 실행
- **패키지 매니저 감지** — 설치된 패키지 매니저를 자동으로 감지하고 버전 표시
- **원활한 통합** — 새로 생성된 프로젝트가 Codeg 워크스페이스에서 바로 열림

현재 **shadcn/ui** 프로젝트 스캐폴딩을 지원하며, 탭 기반 디자인으로 향후 더 많은 프로젝트 유형을 지원할 준비가 되어 있습니다.

</details>

<details>
<summary><h2>채팅 채널</h2></summary>

즐겨 사용하는 메신저 앱 — Telegram, Lark(Feishu), iLink(Weixin) 등 — 을 AI 코딩 에이전트에 연결하세요. 채팅에서 직접 작업을 생성하고, 후속 메시지를 보내고, 권한을 승인하고, 세션을 재개하고, 활동을 모니터링할 수 있습니다 — 도구 호출 상세 정보, 권한 프롬프트, 완료 요약이 포함된 실시간 에이전트 응답을 브라우저를 열지 않고도 받을 수 있습니다.

### 지원 채널

| 채널           | 프로토콜              | 상태 |
| -------------- | --------------------- | ---- |
| Telegram       | Bot API (HTTP 롱폴링) | 내장 |
| Lark (Feishu)  | WebSocket + REST API  | 내장 |
| iLink (Weixin) | WebSocket + REST API  | 내장 |

> 추가 채널(Discord, Slack, DingTalk 등)은 향후 릴리스에서 지원 예정입니다.

</details>

<details>
<summary><h2>Office 문서</h2></summary>

Word, Excel, PowerPoint 파일을 일급 워크플로우로 사용하세요. 내장된 **officecli** 툴셋을 통해 에이전트가 .docx, .xlsx, .pptx 문서를 생성·분석·교정·편집하고, Codeg 내에서 바로 미리볼 수 있습니다.

### 기능

- **생성 및 편집** — 새 문서 생성 또는 기존 .docx / .xlsx / .pptx 파일 수정 (차트, 표, 서식 포함)
- **분석 및 교정** — 문서 구조 검사, 서식 문제 발견, 내용 교정
- **실시간 미리보기** — 파일 탭에서 .docx / .xlsx / .pptx 를 열면 인라인으로 렌더링되고, 에이전트 편집 시 자동 갱신——상시 실행되는 `officecli watch` 서버가 지원 (웹 및 독립 서버 환경에서는 리버스 프록시를 통해 제공, 기능 인증 적용)
- **빠른 실행** — 웰컴 페이지의 「코딩」과 「Office」 탭에서 해당 스킬 호출과 프롬프트 템플릿을 한 번의 클릭으로 입력창에 삽입; 선택된 에이전트에 활성화되지 않은 스킬은 잠금 뱃지로 표시되며 활성화 위치로 안내
- **Office 도구 설정** — 전용 설정 페이지에서 `officecli` 설치 및 스킬×에이전트 매트릭스로 문서 스킬 관리: 임의의 (스킬, 에이전트) 쌍 토글, 일괄 활성화/비활성화 지원

</details>

<details>
<summary><h2>자동화</h2></summary>

컴포저 설정——에이전트, 모델, 프롬프트, 작업 디렉토리, 옵션——을 재사용 가능한 **자동화**로 저장하고, UI 를 열지 않고도 실행하세요.

### 기능

- **한 번 설정, 언제든 재사용** — 완전한 컴포저 설정을 이름 있는 자동화로 저장
- **예약 또는 온디맨드 실행** — cron 스케줄에 따라 자동 실행하거나, 언제든지 수동으로 트리거
- **헤드리스 실행** — 자동화는 백그라운드에서 실행되어 실제 세션을 생성하며, 워크스페이스에서 언제든 열 수 있고 시작 후 워크스페이스로 자동 복귀

</details>

<details>
<summary><h2>빠른 시작</h2></summary>

### 요구 사항

- Node.js `>=22` (권장)
- pnpm `>=10`
- Rust stable (2021 edition)
- Tauri 2 빌드 의존성 (데스크톱 모드만 해당)

Linux (Debian/Ubuntu) 예시:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### 바이너리

Codeg는 단일 워크스페이스에서 세 개의 Rust 바이너리를 제공합니다:

| 바이너리       | 역할                                                                                                | 빌드                                                                        |
| -------------- | --------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `codeg`        | Tauri 데스크톱 앱 (윈도우, 트레이, 업데이터)                                                        | `pnpm tauri build` (릴리스) / `pnpm tauri dev` (개발)                       |
| `codeg-server` | 브라우저/헤드리스 배포용 독립형 HTTP + WebSocket 서버                                               | `pnpm server:build` / `pnpm server:dev`                                     |
| `codeg-mcp`    | 에이전트 CLI에 `delegate_to_agent` 도구를 노출하는 실행별 stdio MCP 컴패니언 (멀티 에이전트 협업) | `pnpm tauri:prepare-sidecars` (`tauri dev` / `tauri build`에서 자동 호출) |

`codeg-mcp`는 런타임에 부모 바이너리 옆에 위치해야 합니다 — 설치 프로그램, Docker 이미지, Tauri 사이드카 번들러 모두 이를 `codeg` / `codeg-server` 옆에 배치합니다. 소스 빌드나 사용자 정의 레이아웃의 경우 `CODEG_MCP_BIN=/abs/path/codeg-mcp` 환경 변수로 조회 위치를 재정의할 수 있습니다. 컴패니언이 누락된 경우 위임은 건너뛰어지고(경고가 한 번 기록됨) 나머지 에이전트 세션은 계속 작동합니다.

### 개발

```bash
pnpm install

# 프론트엔드 전용 (Next.js 개발 서버, Rust 없음)
pnpm dev

# 프론트엔드 정적 내보내기 (out/)
pnpm build

# 전체 데스크톱 앱 (Tauri + Next.js, codeg-mcp 사이드카 자동 빌드)
pnpm tauri dev

# 데스크톱 릴리스 빌드 (codeg-mcp를 externalBin으로 번들링)
pnpm tauri build

# 독립형 서버 (Tauri/GUI 불필요)
pnpm server:dev
pnpm server:build                  # 릴리스 바이너리 위치: src-tauri/target/release/codeg-server

# codeg-mcp 컴패니언을 명시적으로 빌드 (호스트 트리플용)
pnpm tauri:prepare-sidecars        # 출력: src-tauri/binaries/codeg-mcp-<triple>

# 프론트엔드 작업 중이고 위임이 필요하지 않을 때 사이드카 준비 건너뛰기
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# 프론트엔드 테스트 (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# Rust 검사 (src-tauri/에서 실행)
cargo check                                                     # 데스크톱 (기본 features)
cargo check --no-default-features --bin codeg-server            # 서버 모드
cargo check --no-default-features --bin codeg-mcp               # MCP 컴패니언
cargo clippy --all-targets --features test-utils -- -D warnings

# Rust 테스트
cargo test --features test-utils                                # 데스크톱 (통합 포함)
cargo test --no-default-features --bin codeg-server --lib       # 서버 모드
cargo insta review                                              # 파서 스냅샷 업데이트 승인
```

> 팁: `src-tauri/target/release/` 아래에 새 `codeg-mcp` 빌드가 있고 재설치 없이 수동으로 실행한 `codeg-server`가 이를 가리키게 하려면, `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`를 export 하십시오.

### 서버 배포

Codeg는 데스크톱 환경 없이 독립형 웹 서버로 실행할 수 있습니다.

#### 옵션 1: 원라인 설치 (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

특정 버전 또는 사용자 지정 디렉토리에 설치:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

실행:

```bash
codeg-server
```

#### 옵션 2: 원라인 설치 (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

또는 특정 버전 설치:

```powershell
.\install.ps1 -Version v0.5.2
```

#### 옵션 3: GitHub Releases에서 다운로드

사전 빌드된 바이너리(웹 에셋 포함)는 [Releases](https://github.com/xintaofei/codeg/releases) 페이지에서 다운로드할 수 있습니다:

| 플랫폼      | 파일                               |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# 예시: 다운로드, 압축 해제, 실행
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

#### 옵션 4: Docker

```bash
# Docker Compose 사용 (권장)
docker compose up -d

# 또는 Docker로 직접 실행
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# 사용자 정의 토큰 및 프로젝트 디렉토리 마운트
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

Docker 이미지는 멀티 스테이지 빌드(Node.js + Rust → 경량 Debian 런타임)를 사용하며, 저장소 작업을 위한 `git`과 `ssh`가 포함되어 있습니다. 데이터는 `/data` 볼륨에 영속적으로 저장됩니다. 선택적으로 프로젝트 디렉토리를 마운트하여 컨테이너 내에서 로컬 저장소에 접근할 수 있습니다.

#### 옵션 5: 소스에서 빌드

```bash
pnpm install && pnpm build          # 프론트엔드 빌드
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # 위임 컴패니언
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp는 형제 파일로 인식됨
```

두 바이너리를 서로 다른 디렉토리에 두는 경우, 런타임이 컴패니언을 찾을 수 있도록 `CODEG_MCP_BIN=/abs/path/to/codeg-mcp`를 설정하십시오. 설정하지 않으면 멀티 에이전트 위임이 조용히 비활성화됩니다.

#### 구성

환경 변수:

| 변수                           | 기본값                 | 설명                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------------------ | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | HTTP 포트                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| `CODEG_HOST`                   | `0.0.0.0`              | 바인드 주소                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `CODEG_TOKEN`                  | _(랜덤)_               | 인증 토큰 (시작 시 stderr에 출력)                                                                                                                                                                                                                                                                                                                                                                                                           |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | SQLite 데이터베이스 디렉토리(`uploads/`, `pets/`의 루트 역할도 함)                                                                                                                                                                                                                                                                                                                                                                          |
| `CODEG_STATIC_DIR`             | `./web` 또는 `./out`   | Next.js 정적 내보내기 디렉토리                                                                                                                                                                                                                                                                                                                                                                                                              |
| `CODEG_MCP_BIN`                | _(설정 안 됨)_         | `codeg-mcp` 컴패니언의 절대 경로. 기본 실행 파일 형제 + `PATH` 조회를 재정의합니다. 컴패니언이 서버의 설치 디렉토리 외부에 있는 소스 빌드나 사용자 정의 레이아웃에 사용하십시오.                                                                                                                                                                                                                                                            |
| `CODEG_SKIP_SIDECAR`           | _(설정 안 됨)_         | `pnpm tauri dev` / `pnpm tauri build`를 위한 프론트엔드 전용 편의 기능 — `1`일 때 `codeg-mcp` 사이드카 빌드를 건너뜁니다. 해당 빌드에서는 위임이 비활성화됩니다. 출시 품질 산출물에서는 설정하지 않아야 합니다.                                                                                                                                                                                                                              |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _(설정 안 됨)_         | `<data dir>/uploads/` 아래 상주하는 모든 파일의 총 바이트 수에 대한 하드 한도. 10진수 바이트 수(예: 10 GiB의 경우 `10737418240`). 설정하지 않거나 `0`, 또는 파싱할 수 없는 값이면 한도가 비활성화되며, 현재 상태가 보이도록 시작 시 로그 라인을 출력합니다. 이 한도는 단일 `codeg-server` 프로세스 내에서만 적용됩니다 — 하나의 `uploads/` 볼륨을 공유하는 수평 확장 배포에는 외부 조정(파일 잠금, Redis, 리버스 프록시 쿼터)이 필요합니다. |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _(설정 안 됨)_         | 참값(`1` / `true` / `yes` / `on`)으로 설정된 경우, `CODEG_UPLOAD_MAX_TOTAL_BYTES`가 파싱할 수 없는 값으로 설정되어 있으면 WARN과 함께 fail-open 하는 대신 종료 코드 2로 시작을 중단합니다. 보안 정책상 "구성된 쿼터가 반드시 적용되어야 한다"는 요구가 있을 때 사용합니다.                                                                                                                                                                  |

</details>

<details>
<summary><h2>아키텍처</h2></summary>

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

## 개인정보 보호 및 보안

- 파싱, 저장, 프로젝트 작업은 기본적으로 로컬 우선
- 네트워크 접근은 사용자가 명시적으로 작업을 실행할 때만 발생
- 엔터프라이즈 환경을 위한 시스템 프록시 지원
- 웹 서비스 모드에서는 토큰 기반 인증 사용

## 커뮤니티

- 아래 QR 코드를 스캔하여 토론, 피드백, 업데이트를 위한 WeChat 그룹에 참여하세요

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- [LinuxDO](https://linux.do) 커뮤니티의 지원에 감사드립니다

## 감사의 말

- [ACP](https://agentclientprotocol.com) — Agent Client Protocol(ACP)은 Codeg가 여러 에이전트에 연결할 수 있게 해주는 기반입니다
- [Superpowers](https://github.com/obra/superpowers) — Codeg의 전문가 스킬 모듈을 지원하는 프로젝트
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — Codeg의 Office 문서 워크플로우를 지원하는 프로젝트

## 라이선스

Apache-2.0. `LICENSE` 참고.
