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
  <a href="./README.fr.md">Français</a> |
  <strong>Português</strong> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) é um workspace de codificação multiagente. Ele reúne vários agentes (Claude Code, Codex CLI, OpenCode, Gemini CLI, OpenClaw, Cline, Hermes Agent, CodeBuddy, Kimi Code, etc.) em um único workspace, com suporte à agregação de conversas e à colaboração multiagente, além de instalação desktop e implantação em servidor/Docker.

![gallery](../images/gallery.svg)

## Patrocinadores

<table>
  <tr>
    <td colspan="2" align="center">
      <a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg" target="_blank"><img src="https://raw.githubusercontent.com/LeoYeAI/myclaw-sponsor-preview/main/banner.svg" alt="MyClaw.ai — Your OpenClaw Agent, Always On." /></a><br/>
      <strong><a href="https://myclaw.ai/?utm_source=github&utm_campaign=codeg">MyClaw.ai</a></strong> — Plataforma OpenClaw na nuvem totalmente gerenciada: implantação em um clique, disponibilidade 24/7 e propriedade total dos dados, sem precisar gerenciar servidores.
    </td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare (UCloud)</a></strong>
    </td>
    <td>Agradecemos à Compshare por patrocinar este projeto! A Compshare é a plataforma de nuvem de IA da UCloud, oferecendo planos Plan de agentes com modelos nacionais em assinatura mensal ou pagamento por uso, a partir de ¥49/mês. Também oferece acesso estável a modelos estrangeiros via proxy oficial. Compatível com Claude Code, Codex e chamadas de API. Pronto para empresas: alta concorrência, suporte técnico 24/7 e emissão de notas fiscais em autoatendimento. Quem se cadastrar através <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">deste link</a> ganha ¥5 de crédito de avaliação grátis na plataforma!</td>
  </tr>
</table>

> Quer se tornar patrocinador do Codeg? [Entre em contato por e-mail.](mailto:itpkcn@gmail.com)

## Interface principal

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Colaboração Multi-Agente

![Codeg Light](../images/collaboration-light.png#gh-light-mode-only)
![Codeg Dark](../images/collaboration-dark.png#gh-dark-mode-only)

## Fluxo de trabalho do Office

![Codeg Light](../images/office-light.png#gh-light-mode-only)
![Codeg Dark](../images/office-dark.png#gh-dark-mode-only)

## Destaques

- **Agregação de conversas** — importe sessões de todos os agentes suportados para um workspace unificado
- **Colaboração multi-agentes** — dentro de uma mesma sessão, o agente principal delega para sub-agentes de tipos diferentes (p. ex. Claude Code chamando Codex, Gemini) para concluir uma tarefa em conjunto, com cada sub-agente executando como uma sessão independente
- Desenvolvimento paralelo com fluxos `git worktree` integrados
- **Inicializador de Projeto** — crie novos projetos visualmente com pré-visualização em tempo real
- **Documentos Office** — crie, analise, revise e edite arquivos .docx / .xlsx / .pptx com o conjunto de ferramentas officecli integrado; pré-visualização em tempo real em uma aba de arquivo que atualiza enquanto o agente edita
- **Automações** — salve qualquer configuração do compositor como automação reutilizável que executa em segundo plano segundo cronograma cron ou sob demanda
- **Canais de Chat** — conecte Telegram, Lark (Feishu), iLink (Weixin) e mais aos seus agentes de codificação para notificações em tempo real, interação completa de sessão e controle remoto de tarefas
- Gerenciamento de MCP (varredura local + busca/instalação no registro)
- Gerenciamento de Skills (escopo global e por projeto)
- Gerenciamento de contas remotas Git (GitHub e outros servidores Git)
- Modo de serviço web — acesse o Codeg de qualquer navegador para trabalho remoto
- **Implantação de servidor standalone** — execute `codeg-server` em qualquer servidor Linux/macOS, acesse via navegador
- **Suporte a Docker** — `docker compose up` ou `docker run`, com token/porta personalizáveis, persistência de dados e montagem de diretórios de projetos
- Registros de execução — visualizador de registros em tempo real integrado com filtragem e níveis de log por módulo
- Ciclo de engenharia integrado (árvore de arquivos, diff, alterações git, commit, terminal)

## Agentes suportados

| Agente       | Caminho por variável de ambiente      | Padrão macOS / Linux                  | Padrão Windows                                        |
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

> Nota: as variáveis de ambiente têm prioridade sobre os caminhos padrão.

<details>
<summary><h2>Inicializador de Projeto</h2></summary>

Crie novos projetos visualmente com uma interface de painel dividido: configure à esquerda, pré-visualize em tempo real à direita.

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### O que oferece

- **Configuração visual** — selecione estilo, tema de cores, biblioteca de ícones, fonte, raio de borda e mais nos menus suspensos; o iframe de pré-visualização atualiza instantaneamente
- **Pré-visualização ao vivo** — veja o visual escolhido renderizado em tempo real antes de criar qualquer coisa
- **Criação com um clique** — clique em "Criar Projeto" e o launcher executa `shadcn init` com seu preset, template de framework (Next.js / Vite / React Router / Astro / Laravel) e gerenciador de pacotes (pnpm / npm / yarn / bun)
- **Detecção de gerenciadores de pacotes** — verifica automaticamente quais gerenciadores estão instalados e exibe suas versões
- **Integração perfeita** — o projeto recém-criado abre diretamente no workspace do Codeg

Atualmente suporta scaffolding de projetos **shadcn/ui**, com um design baseado em abas preparado para mais tipos de projetos no futuro.

</details>

<details>
<summary><h2>Canais de Chat</h2></summary>

Conecte seus aplicativos de mensagens favoritos — Telegram, Lark (Feishu), iLink (Weixin) e mais — aos seus agentes de codificação IA. Crie tarefas, envie mensagens de acompanhamento, aprove permissões, retome sessões e monitore a atividade diretamente do chat — recebendo respostas do agente em tempo real com detalhes de chamadas de ferramentas, prompts de permissão e resumos de conclusão, tudo sem abrir o navegador.

### Canais suportados

| Canal          | Protocolo                   | Status    |
| -------------- | --------------------------- | --------- |
| Telegram       | Bot API (HTTP long-polling) | Integrado |
| Lark (Feishu)  | WebSocket + REST API        | Integrado |
| iLink (Weixin) | WebSocket + REST API        | Integrado |

> Mais canais (Discord, Slack, DingTalk, etc.) estão planejados para versões futuras.

</details>

<details>
<summary><h2>Documentos Office</h2></summary>

Trabalhe com arquivos Word, Excel e PowerPoint como fluxo de trabalho de primeira classe. O conjunto de ferramentas **officecli** integrado permite que seus agentes criem, analisem, revisem e editem documentos .docx, .xlsx e .pptx — e você pode pré-visualizar o resultado diretamente no Codeg.

### Funcionalidades

- **Criar e editar** — gere novos documentos ou modifique arquivos .docx / .xlsx / .pptx existentes, incluindo gráficos, tabelas e formatação
- **Analisar e revisar** — inspecione a estrutura do documento, identifique problemas de formatação e revise o conteúdo
- **Pré-visualização em tempo real** — abra um .docx / .xlsx / .pptx em uma aba de arquivo e ele renderiza inline, atualizando automaticamente enquanto o agente edita — suportado por um servidor `officecli watch` persistente (com proxy reverso e autenticação por capacidade para ambientes web e servidor)
- **Ações rápidas** — a página de boas-vindas oferece abas de Codificação e Office que inserem a invocação de habilidade correspondente e um modelo de prompt com um clique; habilidades não habilitadas mostram um badge de bloqueio e redirecionam para onde você pode ativá-las
- **Configurações do Office Tools** — uma página de configurações dedicada instala o `officecli` e gerencia suas habilidades de documentos por meio de uma matriz habilidade×agente: alterne qualquer par (habilidade, agente) e aplique alterações em massa

</details>

<details>
<summary><h2>Automações</h2></summary>

Transforme qualquer configuração do compositor — agente, modelo, prompt, diretório de trabalho e opções — em uma **Automação** reutilizável que executa sem abrir a interface.

### Funcionalidades

- **Configure uma vez, reutilize sempre** — salve uma configuração completa do compositor como automação nomeada
- **Agendada ou sob demanda** — execute segundo um cronograma cron ou dispare manualmente quando necessário
- **Execução sem interface** — automações executam em segundo plano e criam sessões reais que podem ser abertas no workspace a qualquer momento; após iniciar, a interface retorna automaticamente ao workspace

</details>

<details>
<summary><h2>Início rápido</h2></summary>

### Requisitos

- Node.js `>=22` (recomendado)
- pnpm `>=10`
- Rust stable (2021 edition)
- Dependências de build do Tauri 2 (somente modo desktop)

Exemplo Linux (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Binários

O Codeg fornece três binários Rust a partir de um único workspace:

| Binário        | Função                                                                                                       | Build                                                                      |
| -------------- | ------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------- |
| `codeg`        | Aplicativo desktop Tauri (janela, bandeja, atualizador)                                                      | `pnpm tauri build` (release) / `pnpm tauri dev` (dev)                      |
| `codeg-server` | Servidor HTTP + WebSocket standalone para implantações em navegador/headless                                 | `pnpm server:build` / `pnpm server:dev`                                    |
| `codeg-mcp`    | Companion stdio MCP por execução que expõe a ferramenta `delegate_to_agent` às CLIs de agentes (colaboração multi-agente) | `pnpm tauri:prepare-sidecars` (invocado automaticamente por `tauri dev` / `tauri build`) |

`codeg-mcp` deve ficar ao lado de seu binário pai em tempo de execução — instaladores, a imagem Docker e o empacotador de sidecars do Tauri o colocam ao lado de `codeg` / `codeg-server`. Compilações a partir do código-fonte e layouts personalizados podem sobrescrever a busca com a variável de ambiente `CODEG_MCP_BIN=/abs/path/codeg-mcp`. Se o companion estiver ausente, a delegação é ignorada (um único aviso é registrado) e o restante da sessão do agente continua funcionando.

### Desenvolvimento

```bash
pnpm install

# Apenas frontend (servidor de desenvolvimento Next.js, sem Rust)
pnpm dev

# Exportação estática do frontend para out/
pnpm build

# Aplicativo desktop completo (Tauri + Next.js, compila o sidecar codeg-mcp automaticamente)
pnpm tauri dev

# Build de release do desktop (empacota codeg-mcp como externalBin)
pnpm tauri build

# Servidor standalone (sem Tauri/GUI necessário)
pnpm server:dev
pnpm server:build                  # binário de release em src-tauri/target/release/codeg-server

# Compilar explicitamente o companion codeg-mcp (para o triple do host)
pnpm tauri:prepare-sidecars        # saída: src-tauri/binaries/codeg-mcp-<triple>

# Pular a preparação do sidecar ao iterar no frontend quando você não precisa de delegação
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# Testes frontend (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# Verificações Rust (executar em src-tauri/)
cargo check                                                     # desktop (features padrão)
cargo check --no-default-features --bin codeg-server            # modo servidor
cargo check --no-default-features --bin codeg-mcp               # companion MCP
cargo clippy --all-targets --features test-utils -- -D warnings

# Testes Rust
cargo test --features test-utils                                # desktop (incl. integração)
cargo test --no-default-features --bin codeg-server --lib       # modo servidor
cargo insta review                                              # aceitar atualizações de snapshots do parser
```

> Dica: quando você tiver um build recente de `codeg-mcp` em `src-tauri/target/release/` e quiser apontar um `codeg-server` lançado manualmente para ele sem reinstalar, exporte `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`.

### Implantação do servidor

O Codeg pode ser executado como um servidor web standalone sem ambiente desktop.

#### Opção 1: Instalação em uma linha (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

Instalar uma versão específica ou em um diretório personalizado:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

Em seguida, executar:

```bash
codeg-server
```

#### Opção 2: Instalação em uma linha (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

Ou instalar uma versão específica:

```powershell
.\install.ps1 -Version v0.5.2
```

#### Opção 3: Baixar do GitHub Releases

Binários pré-compilados (com recursos web incluídos) estão disponíveis na página de [Releases](https://github.com/xintaofei/codeg/releases):

| Plataforma  | Arquivo                            |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# Exemplo: baixar, extrair e executar
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

#### Opção 4: Docker

```bash
# Usando Docker Compose (recomendado)
docker compose up -d

# Ou executar diretamente com Docker
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# Com token personalizado e diretório de projeto montado
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

A imagem Docker usa um build multi-stage (Node.js + Rust → runtime Debian slim) e inclui `git` e `ssh` para operações com repositórios. Os dados são persistidos no volume `/data`. Opcionalmente, você pode montar diretórios de projetos para acessar repositórios locais de dentro do contêiner.

#### Opção 5: Compilar a partir do código-fonte

```bash
pnpm install && pnpm build          # compilar frontend
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # companion de delegação
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp é detectado como irmão
```

Se você mantiver os dois binários em diretórios separados, defina `CODEG_MCP_BIN=/abs/path/to/codeg-mcp` para que o runtime ainda possa encontrar o companion; sem isso, a delegação multi-agente é desativada silenciosamente.

#### Configuração

Variáveis de ambiente:

| Variável                       | Padrão                 | Descrição                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| ------------------------------ | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CODEG_PORT`                   | `3080`                 | Porta HTTP                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `CODEG_HOST`                   | `0.0.0.0`              | Endereço de bind                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `CODEG_TOKEN`                  | _(aleatório)_          | Token de autenticação (impresso no stderr ao iniciar)                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | Diretório do banco de dados SQLite (também raiz de `uploads/`, `pets/`)                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `CODEG_STATIC_DIR`             | `./web` ou `./out`     | Diretório de exportação estática do Next.js                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `CODEG_MCP_BIN`                | _(não definido)_       | Caminho absoluto para o companion `codeg-mcp`. Sobrescreve a busca padrão por irmão-do-executável + `PATH`. Use isso para compilações a partir do código-fonte ou layouts personalizados em que o companion reside fora do diretório de instalação do servidor.                                                                                                                                                                                                                                   |
| `CODEG_SKIP_SIDECAR`           | _(não definido)_       | Conveniência apenas de frontend para `pnpm tauri dev` / `pnpm tauri build` — quando `1`, pula a compilação do sidecar `codeg-mcp`. A delegação fica desativada nesse build; artefatos de qualidade de release devem deixá-la não definida.                                                                                                                                                                                                                                                        |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _(não definido)_       | Limite rígido do total de bytes residentes em `<data dir>/uploads/`. Contagem decimal de bytes (ex.: `10737418240` para 10 GiB). Não definido, `0` ou um valor não analisável desativa o limite e imprime uma linha de inicialização para tornar o estado visível. O limite é aplicado dentro de um único processo `codeg-server` — implantações escaladas horizontalmente que compartilham um volume `uploads/` precisam de coordenação externa (lock de arquivo, Redis, cota de proxy reverso). |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _(não definido)_       | Quando verdadeiro (`1` / `true` / `yes` / `on`), aborta a inicialização com código de saída 2 se `CODEG_UPLOAD_MAX_TOTAL_BYTES` estiver definido como um valor não analisável, em vez de continuar com um WARN. Use isso quando sua política de segurança exigir que "a cota configurada deve ser efetiva".                                                                                                                                                                                       |

</details>

<details>
<summary><h2>Arquitetura</h2></summary>

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

## Privacidade e segurança

- Local-first por padrão para análise, armazenamento e operações do projeto
- O acesso à rede ocorre apenas em ações iniciadas pelo usuário
- Suporte a proxy do sistema para ambientes corporativos
- O modo de serviço web usa autenticação baseada em token

## Comunidade

- Escaneie o QR code abaixo para entrar em nosso grupo do WeChat para discussões, feedback e atualizações

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- Obrigado à comunidade [LinuxDO](https://linux.do) pelo apoio

## Agradecimentos

- [ACP](https://agentclientprotocol.com) — o Agent Client Protocol (ACP) é a base que permite ao Codeg conectar-se a múltiplos agentes
- [Superpowers](https://github.com/obra/superpowers) — alimenta o módulo de habilidades de especialistas do Codeg
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — alimenta o fluxo de trabalho de documentos Office do Codeg

## Licença

Apache-2.0. Veja `LICENSE`.
