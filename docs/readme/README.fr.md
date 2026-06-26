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
  <a href="./README.de.md">Deutsch</a> |
  <strong>Français</strong> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) est un espace de travail de codage multi-agent. Il réunit plusieurs agents (Claude Code, Codex CLI, OpenCode, Gemini CLI, OpenClaw, Cline, Hermes Agent, CodeBuddy, Kimi Code, etc.) dans un seul espace de travail, prend en charge l'agrégation des conversations et la collaboration multi-agent, ainsi que l'installation desktop et le déploiement serveur/Docker.

![gallery](../images/gallery.svg)

## Sponsors

<table>
  <tr>
    <td colspan="2" align="center">
      <a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg" target="_blank"><img src="https://raw.githubusercontent.com/LeoYeAI/myclaw-sponsor-preview/main/banner.svg" alt="MyClaw.ai — Your OpenClaw Agent, Always On." /></a><br/>
      <strong><a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg">MyClaw.ai</a></strong> — Plateforme OpenClaw cloud entièrement gérée : déploiement en un clic, disponibilité 24h/24 et 7j/7, et propriété totale des données, sans aucune gestion de serveur.
    </td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare (UCloud)</a></strong>
    </td>
    <td>Merci à Compshare pour son parrainage de ce projet ! Compshare est la plateforme cloud IA d'UCloud, proposant des forfaits Plan d'agents avec modèles nationaux en abonnement mensuel ou à l'usage, à partir de 49 ¥/mois. Elle offre également un accès stable aux modèles étrangers via relais officiel. Compatible avec Claude Code, Codex et les appels d'API. Prête pour l'entreprise : forte concurrence, assistance technique 24h/24 et 7j/7, facturation en libre-service. Les utilisateurs qui s'inscrivent via <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">ce lien</a> reçoivent 5 ¥ de crédits gratuits sur la plateforme !</td>
  </tr>
</table>

> Vous souhaitez devenir sponsor de Codeg ? [Contactez-nous par e-mail.](mailto:itpkcn@gmail.com)

## Interface principale

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Collaboration multi-agents

![Codeg Light](../images/collaboration-light.png#gh-light-mode-only)
![Codeg Dark](../images/collaboration-dark.png#gh-dark-mode-only)

## Flux de travail Office

![Codeg Light](../images/office-light.png#gh-light-mode-only)
![Codeg Dark](../images/office-dark.png#gh-dark-mode-only)

## Points forts

- **Agrégation de conversations** — importez les sessions de tous les agents pris en charge dans un workspace unifié
- **Collaboration multi-agents** — au sein d'une même session, l'agent principal délègue à des sous-agents de différents types (p. ex. Claude Code appelant Codex, Gemini) pour accomplir une tâche conjointement, chacun s'exécutant comme une session indépendante
- Développement parallèle avec flux `git worktree` intégré
- **Lanceur de projet** — créez visuellement de nouveaux projets avec aperçu en temps réel
- **Documents Office** — créez, analysez, relisez et modifiez des fichiers .docx / .xlsx / .pptx via l'outillage officecli intégré ; aperçu en temps réel dans un onglet de fichier mis à jour instantanément lors des modifications de l'agent
- **Automatisations** — enregistrez n'importe quelle configuration du compositeur comme automatisation réutilisable s'exécutant sans interface, selon un calendrier cron ou à la demande
- **Canaux de chat** — connectez Telegram, Lark (Feishu), iLink (Weixin) et plus à vos agents de codage pour des notifications en temps réel, une interaction complète avec les sessions et le contrôle à distance des tâches
- Gestion MCP (scan local + recherche/installation depuis le registre)
- Gestion des Skills (portée globale et projet)
- Gestion des comptes distants Git (GitHub et autres serveurs Git)
- Mode service web — accédez à Codeg depuis n'importe quel navigateur pour le travail à distance
- **Déploiement en serveur autonome** — exécutez `codeg-server` sur n'importe quel serveur Linux/macOS, accédez via le navigateur
- **Support Docker** — `docker compose up` ou `docker run`, avec token/port personnalisables, persistance des données et montage de répertoires de projets
- Journaux d'exécution — visualiseur de journaux en temps réel intégré avec filtrage et niveaux de journalisation par module
- Boucle d'ingénierie intégrée (arborescence de fichiers, diff, changements git, commit, terminal)

## Agents supportés

| Agent        | Chemin via variable d'environnement   | Défaut macOS / Linux                  | Défaut Windows                                        |
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

> Remarque : les variables d'environnement ont priorité sur les chemins par défaut.

<details>
<summary><h2>Lanceur de projet</h2></summary>

Créez visuellement de nouveaux projets avec une interface à panneaux divisés : configuration à gauche, aperçu en temps réel à droite.

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### Fonctionnalités

- **Configuration visuelle** — sélectionnez le style, le thème de couleur, la bibliothèque d'icônes, la police, le rayon de bordure et plus dans les menus déroulants ; l'aperçu se met à jour instantanément
- **Aperçu en direct** — visualisez le rendu de votre configuration en temps réel avant de créer quoi que ce soit
- **Création en un clic** — cliquez sur « Créer un projet » et le launcher exécute `shadcn init` avec votre preset, le template de framework (Next.js / Vite / React Router / Astro / Laravel) et le gestionnaire de paquets (pnpm / npm / yarn / bun)
- **Détection des gestionnaires de paquets** — vérifie automatiquement quels gestionnaires sont installés et affiche leurs versions
- **Intégration transparente** — le projet nouvellement créé s'ouvre directement dans l'espace de travail Codeg

Prend actuellement en charge le scaffolding de projets **shadcn/ui**, avec un design à onglets prêt pour d'autres types de projets à l'avenir.

</details>

<details>
<summary><h2>Canaux de chat</h2></summary>

Connectez vos applications de messagerie préférées — Telegram, Lark (Feishu), iLink (Weixin) et plus — à vos agents de codage IA. Créez des tâches, envoyez des messages de suivi, approuvez les permissions, reprenez des sessions et surveillez l'activité directement depuis votre chat — recevez les réponses des agents en temps réel avec les détails des appels d'outils, les demandes de permissions et les résumés de complétion, le tout sans ouvrir de navigateur.

### Canaux pris en charge

| Canal          | Protocole                   | Statut  |
| -------------- | --------------------------- | ------- |
| Telegram       | Bot API (HTTP long-polling) | Intégré |
| Lark (Feishu)  | WebSocket + REST API        | Intégré |
| iLink (Weixin) | WebSocket + REST API        | Intégré |

> D'autres canaux (Discord, Slack, DingTalk, etc.) sont prévus pour de futures versions.

</details>

<details>
<summary><h2>Documents Office</h2></summary>

Travaillez avec des fichiers Word, Excel et PowerPoint comme flux de travail de premier plan. L'outillage **officecli** intégré permet à vos agents de créer, analyser, relire et modifier des documents .docx, .xlsx et .pptx — et de prévisualiser le résultat directement dans Codeg.

### Fonctionnalités

- **Créer et modifier** — générez de nouveaux documents ou modifiez des .docx / .xlsx / .pptx existants, y compris graphiques, tableaux et mise en forme
- **Analyser et relire** — inspectez la structure du document, détectez les problèmes de mise en forme et relisez le contenu
- **Aperçu en direct** — ouvrez un .docx / .xlsx / .pptx dans un onglet de fichier et il s'affiche en ligne, se mettant à jour automatiquement à chaque modification de l'agent — alimenté par un serveur `officecli watch` persistant (avec proxy inverse et authentification par capacité pour les environnements web et serveur)
- **Actions rapides** — la page d'accueil propose des onglets Codage et Office qui insèrent en un clic l'invocation de compétence correspondante et un modèle de prompt dans le compositeur ; les compétences non activées affichent un badge de verrouillage et renvoient vers l'activation
- **Paramètres Office Tools** — une page de paramètres dédiée installe `officecli` et gère ses compétences documentaires via une matrice compétence×agent : basculez n'importe quelle paire (compétence, agent) et appliquez des modifications en bloc

</details>

<details>
<summary><h2>Automatisations</h2></summary>

Transformez n'importe quelle configuration du compositeur — agent, modèle, prompt, répertoire de travail et options — en une **Automatisation** réutilisable s'exécutant sans ouvrir l'interface.

### Fonctionnalités

- **Configurer une fois, réutiliser à volonté** — enregistrez une configuration complète du compositeur comme automatisation nommée
- **Planifiée ou à la demande** — exécutez-la selon un calendrier cron ou déclenchez-la manuellement
- **Exécution sans interface** — les automatisations s'exécutent en arrière-plan et créent de vraies sessions que vous pouvez ouvrir dans le workspace à tout moment ; après le démarrage, l'interface revient automatiquement au workspace

</details>

<details>
<summary><h2>Démarrage rapide</h2></summary>

### Prérequis

- Node.js `>=22` (recommandé)
- pnpm `>=10`
- Rust stable (2021 edition)
- Dépendances de build Tauri 2 (mode bureau uniquement)

Exemple Linux (Debian/Ubuntu) :

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Binaires

Codeg fournit trois binaires Rust issus d'un seul workspace :

| Binaire        | Rôle                                                                                                                  | Build                                                                          |
| -------------- | --------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `codeg`        | Application de bureau Tauri (fenêtre, tray, updater)                                                                  | `pnpm tauri build` (release) / `pnpm tauri dev` (dev)                          |
| `codeg-server` | Serveur HTTP + WebSocket autonome pour les déploiements navigateur/headless                                           | `pnpm server:build` / `pnpm server:dev`                                        |
| `codeg-mcp`    | Compagnon MCP stdio lancé par session, qui expose l'outil `delegate_to_agent` aux CLI d'agents (collaboration multi-agents) | `pnpm tauri:prepare-sidecars` (invoqué automatiquement par `tauri dev` / `tauri build`) |

`codeg-mcp` doit se trouver à côté de son binaire parent au moment de l'exécution — les installeurs, l'image Docker et le bundler de sidecar Tauri le placent tous à côté de `codeg` / `codeg-server`. Les builds depuis les sources et les agencements personnalisés peuvent surcharger la recherche via la variable d'environnement `CODEG_MCP_BIN=/chemin/abs/codeg-mcp`. Si le compagnon est absent, la délégation est ignorée (un seul avertissement est journalisé) et le reste de la session de l'agent continue de fonctionner.

### Développement

```bash
pnpm install

# Frontend uniquement (serveur de dev Next.js, sans Rust)
pnpm dev

# Export statique du frontend vers out/
pnpm build

# Application de bureau complète (Tauri + Next.js, compile automatiquement le sidecar codeg-mcp)
pnpm tauri dev

# Build de release de l'application de bureau (intègre codeg-mcp comme externalBin)
pnpm tauri build

# Serveur autonome (sans Tauri/GUI requis)
pnpm server:dev
pnpm server:build                  # binaire de release dans src-tauri/target/release/codeg-server

# Compiler explicitement le compagnon codeg-mcp (pour la triplet hôte)
pnpm tauri:prepare-sidecars        # sortie : src-tauri/binaries/codeg-mcp-<triple>

# Sauter la préparation du sidecar lors d'itérations sur le frontend sans besoin de délégation
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# Tests frontend (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# Vérifications Rust (exécuter dans src-tauri/)
cargo check                                                     # bureau (features par défaut)
cargo check --no-default-features --bin codeg-server            # mode serveur
cargo check --no-default-features --bin codeg-mcp               # compagnon MCP
cargo clippy --all-targets --features test-utils -- -D warnings

# Tests Rust
cargo test --features test-utils                                # bureau (avec intégration)
cargo test --no-default-features --bin codeg-server --lib       # mode serveur
cargo insta review                                              # accepter les mises à jour de snapshots de parser
```

> Astuce : lorsque vous avez un build récent de `codeg-mcp` sous `src-tauri/target/release/` et que vous voulez y faire pointer un `codeg-server` lancé manuellement sans réinstaller, exportez `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`.

### Déploiement du serveur

Codeg peut fonctionner comme un serveur web autonome sans environnement de bureau.

#### Option 1 : Installation en une ligne (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

Installer une version spécifique ou dans un répertoire personnalisé :

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

Puis exécuter :

```bash
codeg-server
```

#### Option 2 : Installation en une ligne (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

Ou installer une version spécifique :

```powershell
.\install.ps1 -Version v0.5.2
```

#### Option 3 : Télécharger depuis GitHub Releases

Les binaires pré-compilés (avec les ressources web incluses) sont disponibles sur la page [Releases](https://github.com/xintaofei/codeg/releases) :

| Plateforme  | Fichier                            |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# Exemple : télécharger, extraire et exécuter
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

#### Option 4 : Docker

```bash
# Avec Docker Compose (recommandé)
docker compose up -d

# Ou exécuter directement avec Docker
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# Avec token personnalisé et répertoire de projet monté
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

L'image Docker utilise un build multi-stage (Node.js + Rust → runtime Debian allégé) et inclut `git` et `ssh` pour les opérations sur les dépôts. Les données sont persistées dans le volume `/data`. Vous pouvez optionnellement monter des répertoires de projets pour accéder aux dépôts locaux depuis le conteneur.

#### Option 5 : Compiler depuis les sources

```bash
pnpm install && pnpm build          # compiler le frontend
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # compagnon de délégation
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp est détecté comme fichier voisin
```

Si vous conservez les deux binaires dans des répertoires séparés, définissez `CODEG_MCP_BIN=/chemin/abs/vers/codeg-mcp` pour que le runtime puisse toujours trouver le compagnon ; sans cela, la délégation multi-agents est désactivée silencieusement.

#### Configuration

Variables d'environnement :

| Variable                       | Valeur par défaut      | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| ------------------------------ | ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | Port HTTP                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `CODEG_HOST`                   | `0.0.0.0`              | Adresse de liaison                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `CODEG_TOKEN`                  | _(aléatoire)_          | Jeton d'authentification (affiché sur stderr au démarrage)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | Répertoire de la base de données SQLite (racine également de `uploads/`, `pets/`)                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `CODEG_STATIC_DIR`             | `./web` ou `./out`     | Répertoire d'export statique Next.js                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `CODEG_MCP_BIN`                | _(non défini)_         | Chemin absolu vers le compagnon `codeg-mcp`. Remplace la recherche par défaut (fichier voisin de l'exécutable + `PATH`). À utiliser pour les builds depuis les sources ou les agencements personnalisés où le compagnon réside en dehors du répertoire d'installation du serveur.                                                                                                                                                                                                                                                         |
| `CODEG_SKIP_SIDECAR`           | _(non défini)_         | Variable de confort réservée au frontend pour `pnpm tauri dev` / `pnpm tauri build` — lorsqu'elle vaut `1`, la compilation du sidecar `codeg-mcp` est ignorée. La délégation est désactivée dans ce build ; les artefacts de qualité production doivent la laisser non définie.                                                                                                                                                                                                                                                          |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _(non défini)_         | Limite stricte du nombre total d'octets résidant sous `<data dir>/uploads/`. Nombre d'octets en décimal (p. ex. `10737418240` pour 10 Gio). Non défini, `0` ou une valeur non analysable désactive la limite et imprime une ligne au démarrage pour que la configuration soit visible. La limite est appliquée au sein d'un seul processus `codeg-server` — les déploiements à mise à l'échelle horizontale partageant un même volume `uploads/` nécessitent une coordination externe (verrou de fichier, Redis, quota de proxy inverse). |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _(non défini)_         | Lorsque vrai (`1` / `true` / `yes` / `on`), interrompt le démarrage avec le code de sortie 2 si `CODEG_UPLOAD_MAX_TOTAL_BYTES` est défini sur une valeur non analysable, au lieu de continuer avec un WARN. Utilisez ceci lorsque votre politique de sécurité exige que « le quota configuré doit être effectif ».                                                                                                                                                                                                                        |

</details>

<details>
<summary><h2>Architecture</h2></summary>

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

## Confidentialité et sécurité

- Local-first par défaut pour l'analyse, le stockage et les opérations sur le projet
- L'accès réseau ne se produit que lors d'actions déclenchées par l'utilisateur
- Prise en charge du proxy système pour les environnements d'entreprise
- Le mode service web utilise l'authentification par jeton

## Communauté

- Scannez le QR code ci-dessous pour rejoindre notre groupe WeChat pour des discussions, des retours et des mises à jour

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- Merci à la communauté [LinuxDO](https://linux.do) pour son soutien

## Remerciements

- [ACP](https://agentclientprotocol.com) — l'Agent Client Protocol (ACP) est la base qui permet à Codeg de se connecter à plusieurs agents
- [Superpowers](https://github.com/obra/superpowers) — alimente le module de compétences d'experts de Codeg
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — alimente le flux de travail des documents Office de Codeg

## Licence

Apache-2.0. Voir `LICENSE`.
