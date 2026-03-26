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
  <a href="./README.de.md">Deutsch</a> |
  <strong>Français</strong> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) est un workspace de codage multi-agents de niveau entreprise.
Il unifie les agents de codage IA locaux (Claude Code, Codex CLI, OpenCode, Gemini CLI,
OpenClaw, etc.) dans une application de bureau et un service web — permettant le développement à distance depuis n'importe quel navigateur — avec agrégation de sessions, développement
parallèle via `git worktree`, gestion MCP/Skills et workflows intégrés Git/fichiers/terminal.

## Interface principale
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Affichage en tuiles des sessions
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> Statut actuel : `v0.2.x` (itération rapide, adapté aux utilisateurs précoces)

## Points forts

- Workspace multi-agents unifié dans le même projet
- Ingestion locale des sessions avec rendu structuré
- Développement parallèle avec flux `git worktree` intégré
- Gestion MCP (scan local + recherche/installation depuis le registre)
- Gestion des Skills (portée globale et projet)
- Gestion des comptes distants Git (GitHub et autres serveurs Git)
- Mode service web — accédez à Codeg depuis n'importe quel navigateur pour le travail à distance
- Boucle d'ingénierie intégrée (arborescence de fichiers, diff, changements git, commit, terminal)

## Périmètre pris en charge

### 1) Ingestion de sessions (sessions historiques)

| Agent | Chemin via variable d'environnement | Défaut macOS / Linux | Défaut Windows |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> Remarque : les variables d'environnement ont priorité sur les chemins par défaut.

### 2) Sessions temps réel ACP

Prend actuellement en charge 5 agents : Claude Code, Codex CLI, Gemini CLI, OpenCode et OpenClaw.

### 3) Prise en charge des paramètres Skills

- Pris en charge : `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- D'autres adaptateurs seront ajoutés progressivement

### 4) Applications cibles MCP

Cibles en écriture actuelles :

- Claude Code
- Codex
- OpenCode

## Démarrage rapide

### Prérequis

- Node.js `>=22` (recommandé)
- pnpm `>=10`
- Rust stable (2021 edition)
- Dépendances de build Tauri 2

Exemple Linux (Debian/Ubuntu) :

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Développement

```bash
pnpm install

# Application de bureau complète (Tauri + Next.js)
pnpm tauri dev

# Frontend uniquement
pnpm dev

# Export statique du frontend vers out/
pnpm build

# Build de l'application de bureau
pnpm tauri build

# Lint
pnpm eslint .

# Vérifications Rust (exécuter dans src-tauri/)
cargo check
cargo clippy
cargo build
```

## Architecture

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

## Contraintes

- Le frontend utilise l'export statique (`output: "export"`)
- Pas de routes dynamiques Next.js (`[param]`) ; utiliser les paramètres de requête à la place
- Paramètres des commandes Tauri : `camelCase` côté frontend, `snake_case` côté Rust
- TypeScript en mode strict

## Confidentialité et sécurité

- Local-first par défaut pour l'analyse, le stockage et les opérations sur le projet
- L'accès réseau ne se produit que lors d'actions déclenchées par l'utilisateur
- Prise en charge du proxy système pour les environnements d'entreprise

## Licence

Apache-2.0. Voir `LICENSE`.
