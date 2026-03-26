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
  <a href="./README.ko.md">한국어</a> |
  <a href="./README.es.md">Español</a> |
  <strong>Deutsch</strong> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) ist ein unternehmenstauglicher Multi-Agent-Workspace
für die Programmierung.
Es vereint lokale KI-Coding-Agenten (Claude Code, Codex CLI, OpenCode,
Gemini CLI, OpenClaw usw.) in einer Desktop-App und einem Webservice — Remote-Entwicklung von jedem Browser aus — mit Sitzungsaggregation,
paralleler `git worktree`-Entwicklung, MCP/Skills-Verwaltung und integrierten
Git/Datei/Terminal-Workflows.

## Hauptoberfläche
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Sitzungskachelansicht
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> Aktueller Status: `v0.2.x` (schnelle Iteration, geeignet für Early Adopters)

## Highlights

- Einheitlicher Multi-Agent-Workspace im selben Projekt
- Lokale Sitzungserfassung mit strukturierter Darstellung
- Parallele Entwicklung mit integrierten `git worktree`-Abläufen
- MCP-Verwaltung (lokaler Scan + Registry-Suche/Installation)
- Skills-Verwaltung (global und projektbezogen)
- Git-Remote-Kontoverwaltung (GitHub und andere Git-Server)
- Webdienst-Modus — Zugriff auf Codeg über jeden Browser für Remote-Arbeit
- Integrierter Engineering-Kreislauf (Dateibaum, Diff, Git-Änderungen, Commit, Terminal)

## Unterstützter Umfang

### 1) Sitzungserfassung (historische Sitzungen)

| Agent | Umgebungsvariablen-Pfad | macOS / Linux Standard | Windows Standard |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> Hinweis: Umgebungsvariablen haben Vorrang vor Fallback-Pfaden.

### 2) ACP-Echtzeitsitzungen

Unterstützt derzeit 5 Agenten: Claude Code, Codex CLI, Gemini CLI, OpenCode und OpenClaw.

### 3) Skills-Einstellungen

- Unterstützt: `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- Weitere Adapter werden schrittweise hinzugefügt

### 4) MCP-Zielanwendungen

Aktuelle beschreibbare Ziele:

- Claude Code
- Codex
- OpenCode

## Schnellstart

### Voraussetzungen

- Node.js `>=22` (empfohlen)
- pnpm `>=10`
- Rust stable (2021 edition)
- Tauri-2-Build-Abhängigkeiten

Linux-Beispiel (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Entwicklung

```bash
pnpm install

# Vollständige Desktop-App (Tauri + Next.js)
pnpm tauri dev

# Nur Frontend
pnpm dev

# Frontend-Statikexport nach out/
pnpm build

# Desktop-Build
pnpm tauri build

# Lint
pnpm eslint .

# Rust-Prüfungen (in src-tauri/ ausführen)
cargo check
cargo clippy
cargo build
```

## Architektur

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

## Einschränkungen

- Frontend verwendet statischen Export (`output: "export"`)
- Keine dynamischen Next.js-Routen (`[param]`); stattdessen Query-Parameter verwenden
- Tauri-Befehlsparameter: `camelCase` im Frontend, `snake_case` in Rust
- TypeScript im strikten Modus

## Datenschutz und Sicherheit

- Standardmäßig lokal für Analyse, Speicherung und Projektoperationen
- Netzwerkzugriff erfolgt nur bei benutzergesteuerten Aktionen
- Systemproxy-Unterstützung für Unternehmensumgebungen

## Lizenz

Apache-2.0. Siehe `LICENSE`.
