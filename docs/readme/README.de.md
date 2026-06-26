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
  <a href="./README.ko.md">한국어</a> |
  <a href="./README.es.md">Español</a> |
  <strong>Deutsch</strong> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) ist ein Multi-Agent-Coding-Workspace. Es vereint mehrere Agenten (Claude Code, Codex CLI, OpenCode, Gemini CLI, OpenClaw, Cline, Hermes Agent, CodeBuddy, Kimi Code usw.) in einem Arbeitsbereich, unterstützt Konversationsaggregation und Multi-Agent-Zusammenarbeit sowie Desktop-Installation und Server-/Docker-Bereitstellung.

![gallery](../images/gallery.svg)

## Sponsoren

<table>
  <tr>
    <td colspan="2" align="center">
      <a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg" target="_blank"><img src="https://raw.githubusercontent.com/LeoYeAI/myclaw-sponsor-preview/main/banner.svg" alt="MyClaw.ai — Your OpenClaw Agent, Always On." /></a><br/>
      <strong><a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg">MyClaw.ai</a></strong> — Vollständig verwaltete OpenClaw-Cloud-Plattform: Ein-Klick-Bereitstellung, 24/7-Verfügbarkeit und vollständiger Datenbesitz – ganz ohne eigene Serververwaltung.
    </td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare (UCloud)</a></strong>
    </td>
    <td>Vielen Dank an Compshare für die Unterstützung dieses Projekts! Compshare ist die KI-Cloud-Plattform von UCloud und bietet preiswerte monatliche und nutzungsbasierte Plan-Tarife für inländische Modell-Agents ab 49 ¥/Monat. Zusätzlich bietet sie stabilen, offiziell weitergeleiteten Zugriff auf Modelle aus Übersee. Unterstützt Claude Code, Codex und API-Aufrufe. Enterprise-tauglich: hohe Parallelität, 24/7-Support, Self-Service-Rechnungsstellung. Wer sich über <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">diesen Link</a> registriert, erhält 5 ¥ Plattformguthaben gratis!</td>
  </tr>
</table>

> Möchten Sie Codeg-Sponsor werden? [Schreiben Sie uns gerne eine E-Mail.](mailto:itpkcn@gmail.com)

## Hauptoberfläche

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Multi-Agent-Zusammenarbeit

![Codeg Light](../images/collaboration-light.png#gh-light-mode-only)
![Codeg Dark](../images/collaboration-dark.png#gh-dark-mode-only)

## Office-Workflow

![Codeg Light](../images/office-light.png#gh-light-mode-only)
![Codeg Dark](../images/office-dark.png#gh-dark-mode-only)

## Highlights

- **Konversations-Aggregation** — Sitzungen aller unterstützten Agenten in einen einheitlichen Workspace importieren
- **Multi-Agent-Kollaboration** — innerhalb einer Sitzung delegiert der Haupt-Agent an Sub-Agenten unterschiedlicher Typen (z. B. Claude Code ruft Codex, Gemini auf), um eine Aufgabe gemeinsam zu erledigen, wobei jeder Sub-Agent als eigenständige Sitzung läuft
- Parallele Entwicklung mit integrierten `git worktree`-Abläufen
- **Projekt-Starter** — neue Projekte visuell erstellen mit Live-Vorschau
- **Office-Dokumente** — erstelle, analysiere, überprüfe und bearbeite .docx / .xlsx / .pptx-Dateien mit dem integrierten officecli-Toolset; Live-Vorschau in einer Datei-Registerkarte, die bei Agent-Bearbeitungen sofort aktualisiert wird
- **Automatisierungen** — speichere eine beliebige Composer-Konfiguration als wiederverwendbare Automatisierung, die headless per Cron-Zeitplan oder auf Abruf ausgeführt wird
- **Chat-Kanäle** — Telegram, Lark (Feishu), iLink (Weixin) und mehr mit Ihren Coding-Agenten verbinden für Echtzeit-Benachrichtigungen, vollständige Sitzungsinteraktion und Remote-Aufgabensteuerung
- MCP-Verwaltung (lokaler Scan + Registry-Suche/Installation)
- Skills-Verwaltung (global und projektbezogen)
- Git-Remote-Kontoverwaltung (GitHub und andere Git-Server)
- Webdienst-Modus — Zugriff auf Codeg über jeden Browser für Remote-Arbeit
- **Standalone-Server-Bereitstellung** — `codeg-server` auf jedem Linux/macOS-Server ausführen, Zugriff über den Browser
- **Docker-Unterstützung** — `docker compose up` oder `docker run`, mit benutzerdefiniertem Token/Port, Datenpersistenz und Projektverzeichnis-Mounts
- Laufzeit-Protokolle — integrierter Echtzeit-Protokollviewer mit Filterung und modulbezogenen Protokollstufen
- Integrierter Engineering-Kreislauf (Dateibaum, Diff, Git-Änderungen, Commit, Terminal)

## Unterstützte Agenten

| Agent        | Umgebungsvariablen-Pfad               | macOS / Linux Standard                | Windows Standard                                      |
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

> Hinweis: Umgebungsvariablen haben Vorrang vor Fallback-Pfaden.

<details>
<summary><h2>Projekt-Starter</h2></summary>

Erstellen Sie neue Projekte visuell mit einer geteilten Oberfläche: links konfigurieren, rechts in Echtzeit Vorschau anzeigen.

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### Funktionen

- **Visuelle Konfiguration** — Stil, Farbthema, Icon-Bibliothek, Schrift, Rahmenradius und mehr über Dropdowns auswählen; die Vorschau aktualisiert sich sofort
- **Live-Vorschau** — das gewählte Look & Feel wird in Echtzeit gerendert, bevor etwas erstellt wird
- **Ein-Klick-Erstellung** — klicken Sie auf „Projekt erstellen" und der Launcher führt `shadcn init` mit Ihrem Preset, Framework-Template (Next.js / Vite / React Router / Astro / Laravel) und Paketmanager (pnpm / npm / yarn / bun) aus
- **Paketmanager-Erkennung** — prüft automatisch, welche Paketmanager installiert sind und zeigt ihre Versionen an
- **Nahtlose Integration** — das neu erstellte Projekt wird sofort im Codeg-Workspace geöffnet

Unterstützt derzeit **shadcn/ui**-Projekt-Scaffolding, mit einem Tab-basierten Design für zukünftige Projekttypen.

</details>

<details>
<summary><h2>Chat-Kanäle</h2></summary>

Verbinden Sie Ihre bevorzugten Messaging-Apps — Telegram, Lark (Feishu), iLink (Weixin) und mehr — mit Ihren KI-Coding-Agenten. Erstellen Sie Aufgaben, senden Sie Folgenachrichten, genehmigen Sie Berechtigungen, setzen Sie Sitzungen fort und überwachen Sie die Aktivität direkt aus dem Chat — empfangen Sie Echtzeit-Antworten der Agenten mit Tool-Call-Details, Berechtigungsanfragen und Abschlusszusammenfassungen, ohne einen Browser zu öffnen.

### Unterstützte Kanäle

| Kanal          | Protokoll                   | Status     |
| -------------- | --------------------------- | ---------- |
| Telegram       | Bot API (HTTP Long-Polling) | Integriert |
| Lark (Feishu)  | WebSocket + REST API        | Integriert |
| iLink (Weixin) | WebSocket + REST API        | Integriert |

> Weitere Kanäle (Discord, Slack, DingTalk usw.) sind für zukünftige Releases geplant.

</details>

<details>
<summary><h2>Office-Dokumente</h2></summary>

Arbeiten Sie mit Word-, Excel- und PowerPoint-Dateien als erstklassigen Workflow. Das integrierte **officecli**-Toolset ermöglicht es Ihren Agenten, .docx-, .xlsx- und .pptx-Dokumente zu erstellen, zu analysieren, zu korrigieren und zu bearbeiten — und das Ergebnis direkt in Codeg zu prüfen.

### Funktionen

- **Erstellen und Bearbeiten** — neue Dokumente generieren oder vorhandene .docx / .xlsx / .pptx-Dateien bearbeiten, einschließlich Diagramme, Tabellen und Formatierung
- **Analysieren und Korrigieren** — Dokumentstruktur prüfen, Formatierungsprobleme aufdecken und Inhalte korrigieren
- **Live-Vorschau** — öffnen Sie eine .docx / .xlsx / .pptx-Datei in einem Datei-Tab; sie wird inline gerendert und aktualisiert sich automatisch, wenn der Agent Änderungen vornimmt — unterstützt durch einen dauerhaft laufenden `officecli watch`-Server (mit Reverse-Proxy und Fähigkeits-Authentifizierung für Web- und Serverumgebungen)
- **Schnellaktionen** — die Willkommensseite bietet Tabs für Codierung und Office, die mit einem Klick den passenden Skill-Aufruf und eine Prompt-Vorlage in den Composer einfügen; nicht aktivierte Skills zeigen ein Schloss-Badge und verlinken zur Aktivierung
- **Office-Tools-Einstellungen** — eine dedizierte Einstellungsseite installiert `officecli` und verwaltet dessen Dokument-Skills über eine Skill×Agent-Matrix: beliebige (Skill, Agent)-Paare umschalten und Massenänderungen anwenden

</details>

<details>
<summary><h2>Automatisierungen</h2></summary>

Speichern Sie jede Composer-Konfiguration — Agent, Modell, Prompt, Arbeitsverzeichnis und Optionen — als wiederverwendbare **Automatisierung**, die ohne geöffnete Benutzeroberfläche ausgeführt wird.

### Funktionen

- **Einmal konfigurieren, immer wieder nutzen** — vollständige Composer-Konfiguration als benannte Automatisierung speichern
- **Geplant oder auf Abruf** — nach Cron-Zeitplan oder manuell starten
- **Headless-Ausführung** — Automatisierungen laufen im Hintergrund und erzeugen echte Sitzungen, die jederzeit im Workspace geöffnet werden können; nach dem Start kehrt die Oberfläche automatisch in den Workspace zurück

</details>

<details>
<summary><h2>Schnellstart</h2></summary>

### Voraussetzungen

- Node.js `>=22` (empfohlen)
- pnpm `>=10`
- Rust stable (2021 edition)
- Tauri-2-Build-Abhängigkeiten (nur Desktop-Modus)

Linux-Beispiel (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Binärdateien

Codeg liefert drei Rust-Binärdateien aus einem einzigen Workspace:

| Binärdatei     | Rolle                                                                                                                | Build                                                                       |
| -------------- | -------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `codeg`        | Tauri-Desktop-App (Fenster, Tray, Updater)                                                                           | `pnpm tauri build` (Release) / `pnpm tauri dev` (Dev)                       |
| `codeg-server` | Standalone HTTP- + WebSocket-Server für Browser-/Headless-Deployments                                                | `pnpm server:build` / `pnpm server:dev`                                     |
| `codeg-mcp`    | Pro-Launch-stdio-MCP-Begleiter, der Agent-CLIs das Werkzeug `delegate_to_agent` bereitstellt (Multi-Agent-Kollaboration) | `pnpm tauri:prepare-sidecars` (automatisch durch `tauri dev` / `tauri build`) |

`codeg-mcp` muss zur Laufzeit neben seiner übergeordneten Binärdatei liegen — Installer, das Docker-Image und der Tauri-Sidecar-Bundler legen ihn alle neben `codeg` / `codeg-server` ab. Quellcode-Builds und benutzerdefinierte Layouts können die Suche mit der Umgebungsvariablen `CODEG_MCP_BIN=/abs/pfad/codeg-mcp` überschreiben. Fehlt der Begleiter, wird die Delegation übersprungen (eine einzige Warnung wird protokolliert) und die restliche Agenten-Sitzung funktioniert weiter.

### Entwicklung

```bash
pnpm install

# Nur Frontend (Next.js-Dev-Server, kein Rust)
pnpm dev

# Frontend-Statikexport nach out/
pnpm build

# Vollständige Desktop-App (Tauri + Next.js, baut codeg-mcp-Sidecar automatisch)
pnpm tauri dev

# Desktop-Release-Build (bündelt codeg-mcp als externalBin)
pnpm tauri build

# Standalone-Server (kein Tauri/GUI erforderlich)
pnpm server:dev
pnpm server:build                  # Release-Binary unter src-tauri/target/release/codeg-server

# codeg-mcp-Begleiter explizit bauen (für das Host-Triple)
pnpm tauri:prepare-sidecars        # Ausgabe: src-tauri/binaries/codeg-mcp-<triple>

# Sidecar-Vorbereitung überspringen, wenn am Frontend gearbeitet wird und keine Delegation benötigt wird
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# Frontend-Tests (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# Rust-Prüfungen (in src-tauri/ ausführen)
cargo check                                                     # Desktop (Standard-Features)
cargo check --no-default-features --bin codeg-server            # Server-Modus
cargo check --no-default-features --bin codeg-mcp               # MCP-Begleiter
cargo clippy --all-targets --features test-utils -- -D warnings

# Rust-Tests
cargo test --features test-utils                                # Desktop (inkl. Integration)
cargo test --no-default-features --bin codeg-server --lib       # Server-Modus
cargo insta review                                              # Parser-Snapshot-Updates akzeptieren
```

> Tipp: Wenn unter `src-tauri/target/release/` ein frischer `codeg-mcp`-Build vorliegt und Sie einen manuell gestarteten `codeg-server` darauf zeigen lassen wollen, ohne ihn neu zu installieren, exportieren Sie `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`.

### Server-Bereitstellung

Codeg kann als eigenständiger Webserver ohne Desktop-Umgebung betrieben werden.

#### Option 1: Ein-Zeilen-Installation (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

Eine bestimmte Version oder in ein benutzerdefiniertes Verzeichnis installieren:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

Dann ausführen:

```bash
codeg-server
```

#### Option 2: Ein-Zeilen-Installation (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

Oder eine bestimmte Version installieren:

```powershell
.\install.ps1 -Version v0.5.2
```

#### Option 3: Von GitHub Releases herunterladen

Vorkompilierte Binärdateien (mit gebündelten Web-Assets) sind auf der [Releases](https://github.com/xintaofei/codeg/releases)-Seite verfügbar:

| Plattform   | Datei                              |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# Beispiel: Herunterladen, Entpacken und Ausführen
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

#### Option 4: Docker

```bash
# Mit Docker Compose (empfohlen)
docker compose up -d

# Oder direkt mit Docker ausführen
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# Mit benutzerdefiniertem Token und Projektverzeichnis-Mount
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

Das Docker-Image verwendet einen Multi-Stage-Build (Node.js + Rust → schlanke Debian-Laufzeitumgebung) und enthält `git` und `ssh` für Repository-Operationen. Daten werden im `/data`-Volume persistent gespeichert. Optional können Projektverzeichnisse gemountet werden, um aus dem Container auf lokale Repositories zuzugreifen.

#### Option 5: Aus Quellcode kompilieren

```bash
pnpm install && pnpm build          # Frontend kompilieren
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # Delegations-Begleiter
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp wird als Geschwisterdatei erkannt
```

Wenn Sie die beiden Binärdateien in getrennten Verzeichnissen halten, setzen Sie `CODEG_MCP_BIN=/abs/pfad/zu/codeg-mcp`, damit die Laufzeit den Begleiter dennoch findet; ohne diese Variable wird die Multi-Agent-Delegation stillschweigend deaktiviert.

#### Konfiguration

Umgebungsvariablen:

| Variable                       | Standardwert           | Beschreibung                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| ------------------------------ | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | HTTP-Port                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `CODEG_HOST`                   | `0.0.0.0`              | Bind-Adresse                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `CODEG_TOKEN`                  | _(zufällig)_           | Authentifizierungstoken (wird beim Start auf stderr ausgegeben)                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | SQLite-Datenbankverzeichnis (auch Wurzel für `uploads/`, `pets/`)                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `CODEG_STATIC_DIR`             | `./web` oder `./out`   | Next.js-Statikexport-Verzeichnis                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `CODEG_MCP_BIN`                | _(nicht gesetzt)_      | Absoluter Pfad zum `codeg-mcp`-Begleiter. Überschreibt die Standardsuche (Geschwisterdatei der ausführbaren Datei + `PATH`). Verwenden Sie dies für Quellcode-Builds oder benutzerdefinierte Layouts, bei denen der Begleiter außerhalb des Installationsverzeichnisses des Servers liegt.                                                                                                                                                                                                              |
| `CODEG_SKIP_SIDECAR`           | _(nicht gesetzt)_      | Frontend-only Komfortvariable für `pnpm tauri dev` / `pnpm tauri build` — bei `1` wird der Build des `codeg-mcp`-Sidecars übersprungen. Die Delegation ist in diesem Build deaktiviert; produktionsreife Artefakte dürfen diese Variable nicht gesetzt haben.                                                                                                                                                                                                                                          |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _(nicht gesetzt)_      | Harte Obergrenze für die Gesamtzahl an Bytes unter `<data dir>/uploads/`. Dezimaler Byte-Wert (z. B. `10737418240` für 10 GiB). Nicht gesetzt, `0` oder ein nicht parsbarer Wert deaktiviert das Limit und gibt eine Startzeile aus, damit der Zustand sichtbar ist. Das Limit wird innerhalb eines einzelnen `codeg-server`-Prozesses durchgesetzt — horizontal skalierte Deployments, die sich ein `uploads/`-Volume teilen, benötigen externe Koordination (Datei-Lock, Redis, Reverse-Proxy-Quota). |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _(nicht gesetzt)_      | Wenn wahr (`1` / `true` / `yes` / `on`), wird der Start mit Exit-Code 2 abgebrochen, falls `CODEG_UPLOAD_MAX_TOTAL_BYTES` auf einen nicht parsbaren Wert gesetzt ist, statt mit einer WARN fail-open zu starten. Verwenden Sie dies, wenn Ihre Sicherheitsrichtlinie verlangt, dass „die konfigurierte Quota wirksam sein muss".                                                                                                                                                                        |

</details>

<details>
<summary><h2>Architektur</h2></summary>

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

## Datenschutz und Sicherheit

- Standardmäßig lokal für Analyse, Speicherung und Projektoperationen
- Netzwerkzugriff erfolgt nur bei benutzergesteuerten Aktionen
- Systemproxy-Unterstützung für Unternehmensumgebungen
- Der Webdienst-Modus verwendet tokenbasierte Authentifizierung

## Community

- Scannen Sie den unten stehenden QR-Code, um unserer WeChat-Gruppe für Diskussionen, Feedback und Updates beizutreten

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- Danke an die [LinuxDO](https://linux.do)-Community für ihre Unterstützung

## Danksagungen

- [ACP](https://agentclientprotocol.com) — das Agent Client Protocol (ACP) ist die Grundlage, die es Codeg ermöglicht, sich mit mehreren Agenten zu verbinden
- [Superpowers](https://github.com/obra/superpowers) — unterstützt das Experten-Skills-Modul von Codeg
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — unterstützt den Office-Dokument-Workflow von Codeg

## Lizenz

Apache-2.0. Siehe `LICENSE`.
