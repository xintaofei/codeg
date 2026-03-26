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
  <a href="./README.fr.md">Français</a> |
  <strong>Português</strong> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) é um workspace de codificação multi-agentes de nível empresarial.
Ele unifica agentes de codificação IA locais (Claude Code, Codex CLI, OpenCode, Gemini CLI,
OpenClaw, etc.) em um aplicativo desktop e um serviço web — possibilitando o desenvolvimento remoto a partir de qualquer navegador — com agregação de sessões, desenvolvimento
paralelo via `git worktree`, gerenciamento de MCP/Skills e fluxos integrados de Git/arquivos/terminal.

## Interface principal
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Exibição em mosaico das sessões
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> Status atual: `v0.2.x` (iteração rápida, adequado para adotantes iniciais)

## Destaques

- Workspace multi-agentes unificado no mesmo projeto
- Ingestão local de sessões com renderização estruturada
- Desenvolvimento paralelo com fluxos `git worktree` integrados
- Gerenciamento de MCP (varredura local + busca/instalação no registro)
- Gerenciamento de Skills (escopo global e por projeto)
- Gerenciamento de contas remotas Git (GitHub e outros servidores Git)
- Modo de serviço web — acesse o Codeg de qualquer navegador para trabalho remoto
- Ciclo de engenharia integrado (árvore de arquivos, diff, alterações git, commit, terminal)

## Escopo suportado

### 1) Ingestão de sessões (sessões históricas)

| Agente | Caminho por variável de ambiente | Padrão macOS / Linux | Padrão Windows |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> Nota: as variáveis de ambiente têm prioridade sobre os caminhos padrão.

### 2) Sessões em tempo real ACP

Atualmente suporta 5 agentes: Claude Code, Codex CLI, Gemini CLI, OpenCode e OpenClaw.

### 3) Suporte a configurações de Skills

- Suportado: `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- Mais adaptadores serão adicionados progressivamente

### 4) Aplicativos alvo MCP

Alvos de escrita atuais:

- Claude Code
- Codex
- OpenCode

## Início rápido

### Requisitos

- Node.js `>=22` (recomendado)
- pnpm `>=10`
- Rust stable (2021 edition)
- Dependências de build do Tauri 2

Exemplo Linux (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Desenvolvimento

```bash
pnpm install

# Aplicativo desktop completo (Tauri + Next.js)
pnpm tauri dev

# Apenas frontend
pnpm dev

# Exportação estática do frontend para out/
pnpm build

# Build do aplicativo desktop
pnpm tauri build

# Lint
pnpm eslint .

# Verificações Rust (executar em src-tauri/)
cargo check
cargo clippy
cargo build
```

## Arquitetura

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

## Restrições

- O frontend usa exportação estática (`output: "export"`)
- Sem rotas dinâmicas do Next.js (`[param]`); use parâmetros de consulta em vez disso
- Parâmetros de comandos Tauri: `camelCase` no frontend, `snake_case` no Rust
- TypeScript em modo strict

## Privacidade e segurança

- Local-first por padrão para análise, armazenamento e operações do projeto
- O acesso à rede ocorre apenas em ações iniciadas pelo usuário
- Suporte a proxy do sistema para ambientes corporativos

## Licença

Apache-2.0. Veja `LICENSE`.
