# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB)](https://tauri.app/)
[![Next.js](https://img.shields.io/badge/Next.js-16-black)](https://nextjs.org/)

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

Codeg(Code Generation)는 엔터프라이즈급 멀티 Agent 코딩 워크스페이스입니다.
Claude Code, Codex CLI, OpenCode, Gemini CLI, OpenClaw 등 로컬 AI 코딩 Agent를
데스크톱 앱과 웹 서비스로 통합하여 — 브라우저만으로 어디서든 원격 개발이 가능하며 — 세션 집계, 병렬 `git worktree` 개발, MCP/Skills 관리,
Git/파일/터미널 통합 워크플로를 제공합니다.

## 메인 인터페이스
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## 세션 타일 표시
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> 현재 상태: `v0.2.x` (빠른 반복 개발 중, 얼리 어답터에 적합)

## 하이라이트

- 동일 프로젝트에서 멀티 Agent 통합 워크스페이스
- 로컬 세션 수집 및 구조화 렌더링
- 내장 `git worktree` 플로를 통한 병렬 개발
- MCP 관리 (로컬 스캔 + 레지스트리 검색/설치)
- Skills 관리 (글로벌 및 프로젝트 범위)
- Git 원격 계정 관리 (GitHub 및 기타 Git 서버)
- Web 서비스 모드 — 브라우저에서 Codeg에 접속하여 원격 작업 가능
- 통합 엔지니어링 루프 (파일 트리, Diff, Git 변경사항, 커밋, 터미널)

## 지원 범위

### 1) 세션 수집 (히스토리 세션)

| Agent | 환경 변수 경로 | macOS / Linux 기본값 | Windows 기본값 |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> 참고: 환경 변수가 기본 경로보다 우선합니다.

### 2) ACP 실시간 세션

현재 Claude Code, Codex CLI, Gemini CLI, OpenCode, OpenClaw 5가지 에이전트를 지원합니다.

### 3) Skills 설정 지원

- 지원: `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- 추가 어댑터는 순차적으로 추가 예정

### 4) MCP 대상 앱

현재 쓰기 가능한 대상:

- Claude Code
- Codex
- OpenCode

## 빠른 시작

### 요구 사항

- Node.js `>=22` (권장)
- pnpm `>=10`
- Rust stable (2021 edition)
- Tauri 2 빌드 의존성

Linux (Debian/Ubuntu) 예시:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### 개발

```bash
pnpm install

# 전체 데스크톱 앱 (Tauri + Next.js)
pnpm tauri dev

# 프론트엔드만
pnpm dev

# 프론트엔드 정적 내보내기 (out/)
pnpm build

# 데스크톱 빌드
pnpm tauri build

# Lint
pnpm eslint .

# Rust 검사 (src-tauri/에서 실행)
cargo check
cargo clippy
cargo build
```

## 아키텍처

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

## 제약 사항

- 프론트엔드는 정적 내보내기 사용 (`output: "export"`)
- Next.js 동적 라우트 (`[param]`) 불가; 대신 쿼리 파라미터 사용
- Tauri 명령 파라미터: 프론트엔드 `camelCase`, Rust `snake_case`
- TypeScript strict 모드

## 개인정보 보호 및 보안

- 파싱, 저장, 프로젝트 작업은 기본적으로 로컬 우선
- 네트워크 접근은 사용자가 명시적으로 작업을 실행할 때만 발생
- 엔터프라이즈 환경을 위한 시스템 프록시 지원

## 라이선스

Apache-2.0. `LICENSE` 참고.
