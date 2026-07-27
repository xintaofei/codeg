# Codeg

[![Release](https://img.shields.io/github/v/release/xintaofei/codeg)](https://github.com/xintaofei/codeg/releases)
[![Docs](https://img.shields.io/badge/docs-docs.codeg.app-3451b2)](https://docs.codeg.app)
[![License](https://img.shields.io/github/license/xintaofei/codeg)](../../LICENSE)

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

Codeg (Code Generation) ist ein Multi-Agent-Coding-Workspace: Führe jeden KI-Coding-Agenten an einem Ort aus — und lass sie zusammenarbeiten.

Codeg bündelt die Sitzungen aller unterstützten Agenten-CLIs in einem durchsuchbaren Workspace, lässt einen Haupt-Agenten innerhalb einer Aufgabe an Sub-Agenten anderer Typen delegieren und läuft als Desktop-App, eigenständiger Server oder Docker-Container — dazu native iOS- und Android-Clients für die Zeit fernab vom Schreibtisch.

![Workspace](../images/workspace-light.png#gh-light-mode-only)
![Workspace](../images/workspace-dark.png#gh-dark-mode-only)

## 📖 Dokumentation

**Die vollständige Dokumentation liegt unter [docs.codeg.app](https://docs.codeg.app)** — [Erste Schritte](https://docs.codeg.app/getting-started/) · [Guide](https://docs.codeg.app/guide/) · [Referenz](https://docs.codeg.app/reference/)

## 💖 Sponsoren

<table>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare (UCloud)</a></strong>
    </td>
    <td>Vielen Dank an Compshare für die Unterstützung dieses Projekts! Compshare ist die KI-Cloud-Plattform von UCloud und bietet preiswerte monatliche und nutzungsbasierte Plan-Tarife für inländische Modell-Agents ab 49 ¥/Monat. Zusätzlich bietet sie stabilen, offiziell weitergeleiteten Zugriff auf Modelle aus Übersee. Unterstützt Claude Code, Codex und API-Aufrufe. Enterprise-tauglich: hohe Parallelität, 24/7-Support, Self-Service-Rechnungsstellung. Wer sich über <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">diesen Link</a> registriert, erhält 5 ¥ Plattformguthaben gratis!</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE" target="_blank"><img src="../images/sui-xiang.jpg" alt="随想AI中转站" width="200" /></a><br/>
      <strong><a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">随想AI中转站</a></strong>
    </td>
    <td>Vielen Dank an 随想AI中转站 für die Unterstützung dieses Projekts! 随想AI中转站 ist ein zuverlässiger und effizienter API-Relay-Anbieter mit Relay-Diensten für Claude, Codex, Gemini und mehr. Neue Konten erhalten nach der <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">Registrierung</a> für jedes tägliche Einchecken 0,5 ¥ Testguthaben; Aufladungen werden 1:1 gutgeschrieben – ohne Abo, Bezahlung nach Verbrauch. Mehrfach redundante Leitungen, regionsübergreifende Notfallwiederherstellung und automatisches Failover halten langlebige SSE-Verbindungen unterbrechungsfrei.</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://hezu.ink/sign-up?aff=0wVz" target="_blank"><img src="../images/hezu-ink.jpg" alt="合租巴士" width="200" /></a><br/>
      <strong><a href="https://hezu.ink/sign-up?aff=0wVz">合租巴士</a></strong>
    </td>
    <td>Vielen Dank an 合租巴士 für die Unterstützung dieses Projekts! 合租巴士 ist eine zuverlässige und effiziente KI-Relay-Plattform, die hochstabiles Relay für gängige Modelle wie Codex und Claude Code bietet. Das Aufladeverhältnis ist transparent (1:1), mit Codex-Ratenzuschüssen ab nur 0,08. <a href="https://hezu.ink/sign-up?aff=0wVz">Treten Sie der Gruppe über die offizielle Website bei und erhalten Sie 5 USD Testguthaben</a>.</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta" target="_blank"><img src="../images/onehop.jpg" alt="OneHop" width="120" /></a><br/>
      <strong><a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">OneHop</a></strong>
    </td>
    <td>Vielen Dank an OneHop für die Unterstützung dieses Projekts! OneHop bietet Codeg-Nutzern einen einzigen OpenAI-kompatiblen API-Schlüssel für Hunderte führender Modelle, darunter GPT, Claude, Gemini, DeepSeek, Kimi und Qwen. Wechseln Sie zwischen Modellen, ohne mehrere Anbieterkonten zu verwalten oder Ihren Code immer wieder zu ändern, und zahlen Sie nur für das, was Sie nutzen. <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">Registrieren Sie sich über Codeg</a>, um 1 USD Guthaben zu erhalten, und treten Sie zusätzlich der OneHop-Community bei und nehmen Sie an der Willkommensaktion teil, um weitere 5 USD zu erhalten – insgesamt bis zu 6 USD Testguthaben.</td>
  </tr>
</table>

> Möchten Sie Codeg-Sponsor werden? [Schreiben Sie uns gerne eine E-Mail.](mailto:itpkcn@gmail.com)

## 🤖 Unterstützte Agenten

Claude Code · Codex · Gemini · OpenClaw · OpenCode · Cline · Hermes · CodeBuddy · Kimi Code · Pi · Grok · Cursor

Die meisten davon installiert, fixiert und aktualisiert Codeg für dich. Die vollständige Liste, die Laufzeit-Anforderungen jedes Agenten und den Ablageort seiner Sitzungen findest du unter [Unterstützte Agenten](https://docs.codeg.app/guide/supported-agents).

## 🤝 Multi-Agent-Zusammenarbeit

Multi-Agent-Zusammenarbeit, reduziert auf einen Tastendruck: `@` tippen, Agenten auswählen, absenden. Um die Orchestrierung kümmert sich Codeg — es startet jeden erwähnten Agenten als eigene Sitzung, übergibt die Aufgabe und streamt die Arbeit zurück in den Thread, in dem du ohnehin bist. Erwähne zwei, und sie laufen nebeneinander: Claude Code schreibt, Codex prüft. Kein Kontextwechsel, kein Copy-and-paste zwischen Terminals.

![Eine Aufgabe wird aus einer einzigen Codeg-Unterhaltung an Sub-Agenten delegiert](../images/collaboration-light.gif#gh-light-mode-only)
![Eine Aufgabe wird aus einer einzigen Codeg-Unterhaltung an Sub-Agenten delegiert](../images/collaboration-dark.gif#gh-dark-mode-only)

## 📄 Office-Dokumente

Bitte um ein Deck, einen Bericht oder eine Arbeitsmappe, und der Agent baut eine echte `.pptx` / `.docx` / `.xlsx` — während der rechte Bereich sie live rendert. Jede Änderung landet von selbst in der Vorschau: Folien füllen sich, Tabellen nehmen Gestalt an, Zahlen landen in den Zellen. Folie 4 gefällt nicht? Sag es in der nächsten Nachricht — der Agent bearbeitet dieselbe Datei an Ort und Stelle, die Vorschau zieht nach. Kein Export, keine externe Office-App, kein Verlassen von Codeg.

![Ein Agent bearbeitet ein Office-Dokument neben dessen Live-Vorschau](../images/office-light.png#gh-light-mode-only)
![Ein Agent bearbeitet ein Office-Dokument neben dessen Live-Vorschau](../images/office-dark.png#gh-dark-mode-only)

## 💻 Workspace

Ein Workspace, alle Agenten. Egal welcher gerade arbeitet — Claude Code, Codex, Cursor —, er tut es im selben Editor, mit denselben Live-Diffs und demselben Git-Client. Und was dabei entsteht, sind echte Dateien in deinem Repository, die sich vor deinen Augen verändern.

**Sitzungen.** Hol dir die Historie, die du schon hast: vergangene Sitzungen aller installierten Agenten, mit einem Klick importiert und dort fortsetzbar, wo du aufgehört hast. Einmal drin, bleiben sie keine getrennten Silos — erwähne eine alte Sitzung mit `@`, und der Agent, mit dem du gerade sprichst, kann sie lesen, auch wenn ein anderer Agent sie geschrieben hat. So macht der heutige Codex-Lauf da weiter, wo die Claude-Code-Sitzung von letzter Woche aufgehört hat.

**Dateien.** Die Änderungen des Agenten erscheinen als Diffs neben der Unterhaltung, sobald sie landen. Öffne jede Datei in einem echten Editor mit Syntaxhervorhebung, schick eine Datei — oder nur eine Auswahl — mit `⌘L` direkt an den Agenten, und zeig dir Markdown, HTML, Bilder und Office-Dokumente in derselben Ansicht in der Vorschau an.

**Git.** Ein vollwertiger Client, keine Statusanzeige: committen und pushen, die Historie mit dem Push-Status jedes Commits durchgehen, Branches anlegen, mergen, rebasen, stashen, zurücksetzen oder gegen einen anderen Branch diffen. Konflikte öffnen einen dreispaltigen Merge-Editor, in dem du Hunk für Hunk übernimmst oder die Lösung selbst tippst. Und Worktrees machen paralleles Arbeiten zu einer einzigen Aktion — ein neuer Branch, ein eigenes Verzeichnis und eine frische Unterhaltung darin, sodass eine ganze Flotte von Agenten gleichzeitig an verschiedenen Features baut, ohne einander in die Quere zu kommen.

## 📱 iPhone, iPad & Android

Geh vom Schreibtisch weg, nicht von der Arbeit. Die nativen iOS- und Android-Clients verbinden sich mit dem Codeg, das du ohnehin betreibst — dem **Web Service** deiner Desktop-App oder deinem eigenen `codeg-server`. Von dort startest du Sitzungen, verfolgst Antworten und Tool-Aufrufe im Stream, beantwortest Berechtigungsanfragen und siehst dir Projekte und Branches an. Aufs Telefon wandert nichts: Dateien, Agenten-CLIs und Unterhaltungen bleiben auf der Maschine, die Codeg ausführt, und das Zugriffstoken liegt im iOS-Keychain oder im Android Keystore. Beide Clients sind Open Source ([iOS](https://github.com/xintaofei/codeg-ios), [Android](https://github.com/xintaofei/codeg-android)) und derzeit im Testbetrieb; das Koppeln dauert drei Schritte und steht in [Mobile Apps](https://docs.codeg.app/getting-started/installation#mobile-apps).

| iPhone & iPad | Android |
| :---: | :---: |
| <img src="../images/mobile-ios.jpg" alt="Eine Sitzung wird im Codeg-iOS-Client gestartet" width="248" /> | <img src="../images/mobile-android.jpg" alt="Eine Agenten-Antwort, die live in den Codeg-Android-Client läuft" width="248" /> |

## ✨ Highlights

- **[Sitzungs-Aggregation](https://docs.codeg.app/guide/aggregation)** — importiert Sitzungen aller unterstützten Agenten in einen einheitlichen, durchsuchbaren Workspace — und du machst dort weiter, wo du aufgehört hast
- **[Multi-Agent-Zusammenarbeit](https://docs.codeg.app/guide/multi-agent)** — per `@`-Erwähnung an jeden Agenten delegieren: Sub-Agenten unterschiedlicher Typen laufen als eigene Sitzungen, parallel, innerhalb einer Aufgabe
- **[Der Workspace](https://docs.codeg.app/guide/workspace)** — der komplette Engineering-Loop direkt neben dem Agenten: Dateibaum, Editor und Diff, Git-Änderungen, Commit und ein eingebettetes Terminal
- **[Git & Worktrees](https://docs.codeg.app/guide/git)** — Änderungen prüfen und committen, Git-Remote-Konten verwalten und mit integrierten `git worktree`-Abläufen parallel arbeiten
- **[Chat-Kanäle](https://docs.codeg.app/guide/chat-channels)** — steuere deine Agenten aus Telegram, Lark (Feishu) und iLink (Weixin): Aufgaben anlegen, Berechtigungen freigeben, Live-Updates erhalten
- **[Automatisierungen](https://docs.codeg.app/guide/automations)** — einen fertig konfigurierten Composer als wiederverwendbare Automatisierung sichern und headless per Cron-Zeitplan oder auf Zuruf ausführen
- **[Office-Dokumente](https://docs.codeg.app/guide/office)** — `.docx` / `.xlsx` / `.pptx` mit dem mitgelieferten `officecli` erstellen, analysieren, korrigieren und bearbeiten — mit Live-Vorschau im Tab
- **[Wissenschaftliche Recherche](https://docs.codeg.app/guide/research)** — mitgelieferte Research-Skills (Hypothesenbildung, Versuchsplanung, Statistik, Visualisierung, kritische Bewertung, Literatursuche), die jeder Agent aufrufen kann
- **[Project Boot](https://docs.codeg.app/guide/project-boot)** — neue Projekte visuell aufsetzen, mit Live-Vorschau, und direkt im Workspace öffnen
- **[MCP](https://docs.codeg.app/guide/mcp) & [Skills](https://docs.codeg.app/guide/skills)** — lokaler Server-Scan plus Suche/Installation aus der Registry, Skills global oder pro Projekt verwaltet
- **[Desktop, Server & Docker](https://docs.codeg.app/getting-started/deployment)** — eine native Desktop-App, ein eigenständiger `codeg-server` für den Browser oder `docker compose up`
- **[iPhone, iPad & Android](https://docs.codeg.app/getting-started/installation#mobile-apps)** — native Mobile-Clients, die sich mit deinem Desktop oder Server verbinden: Sitzungen starten, Antworten streamen, Berechtigungen freigeben und Projekte von überall durchsehen

## 📦 Installation & Betrieb

**Desktop** — Lade den Installer für macOS, Windows oder Linux aus den [Releases](https://github.com/xintaofei/codeg/releases) und folge der [Installation](https://docs.codeg.app/getting-started/installation).

**Server** — Codeg headless betreiben und aus jedem Browser erreichen. Unter Linux oder macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
CODEG_STATIC_DIR=/usr/local/share/codeg/web codeg-server
```

Unter Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
$env:CODEG_STATIC_DIR="$env:LOCALAPPDATA\codeg\web"; codeg-server
```

**Docker** — derselbe Server, in einem Container:

```bash
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest
```

**Mobil** — installiere die [iOS-App](https://apps.apple.com/app/codeg-client/id6785199071) oder das [Android-APK](https://github.com/xintaofei/codeg-android/releases/latest) und richte sie auf den **Web Service** deiner Desktop-App oder deinen eigenen `codeg-server`: URL, Token, fertig. Die Kopplungsschritte stehen in [Mobile Apps](https://docs.codeg.app/getting-started/installation#mobile-apps).

Compose, vorgebaute Binaries, Builds aus dem Quellcode und In-place-Updates stehen unter [Deployment](https://docs.codeg.app/getting-started/deployment); Umgebungsvariablen unter [Konfiguration](https://docs.codeg.app/getting-started/configuration). Codeg selbst bauen: [Entwicklung](https://docs.codeg.app/reference/development) und [Architektur](https://docs.codeg.app/reference/architecture).

## 🔒 Datenschutz und Sicherheit

- Standardmäßig local-first bei Parsing, Speicherung und Projektoperationen — Netzwerkzugriffe passieren nur bei von dir ausgelösten Aktionen
- Web- und Server-Modus sind durch Token-basierte Authentifizierung geschützt
- System-Proxy-Unterstützung für Unternehmensumgebungen

Details unter [Datenschutz und Sicherheit](https://docs.codeg.app/reference/privacy).

## 👥 Community

- Scannen Sie den unten stehenden QR-Code, um unserer WeChat-Gruppe für Diskussionen, Feedback und Updates beizutreten

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- Danke an die [LinuxDO](https://linux.do)-Community für ihre Unterstützung

## 🙏 Danksagungen

- [Agent Client Protocol](https://agentclientprotocol.com) — die Grundlage, auf der Codeg sich mit jedem unterstützten Agenten verbindet
- [Superpowers](https://github.com/obra/superpowers) — unterstützt das Experten-Skills-Modul von Codeg
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — unterstützt den Office-Dokument-Workflow von Codeg
- [scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills) — unterstützt die wissenschaftlichen Forschungs-Skills von Codeg (MIT-lizenzierte Teilmenge)

## 📜 Lizenz

Apache-2.0. Siehe [LICENSE](../../LICENSE).
