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
  <strong>Español</strong> |
  <a href="./README.de.md">Deutsch</a> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) es un espacio de trabajo empresarial para codificación
con múltiples agentes.
Integra agentes locales de codificación con IA (Claude Code, Codex CLI, OpenCode,
Gemini CLI, OpenClaw, etc.) en una aplicación de escritorio y un servicio web — permitiendo el desarrollo remoto desde cualquier navegador — con agregación
de sesiones, desarrollo paralelo con `git worktree`, gestión de MCP/Skills y
flujos integrados de Git/archivos/terminal.

## Interfaz principal
![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Vista de mosaico de sesiones
![Codeg Light](../images/main2-light.png#gh-light-mode-only)
![Codeg Dark](../images/main2-dark.png#gh-dark-mode-only)

> Estado actual: `v0.2.x` (iteración rápida, adecuado para early adopters)

## Puntos destacados

- Espacio de trabajo unificado para múltiples agentes en el mismo proyecto
- Ingesta local de sesiones con renderizado estructurado
- Desarrollo paralelo con flujos integrados de `git worktree`
- Gestión de MCP (escaneo local + búsqueda/instalación desde registro)
- Gestión de Skills (ámbito global y por proyecto)
- Gestión de cuentas remotas de Git (GitHub y otros servidores Git)
- Modo de servicio web — accede a Codeg desde cualquier navegador para trabajo remoto
- Ciclo de ingeniería integrado (árbol de archivos, diff, cambios git, commit, terminal)

## Alcance soportado

### 1) Ingesta de sesiones (sesiones históricas)

| Agente | Ruta de variable de entorno | Ruta por defecto en macOS / Linux | Ruta por defecto en Windows |
| --- | --- | --- | --- |
| Claude Code | `$CLAUDE_CONFIG_DIR/projects` | `~/.claude/projects` | `%USERPROFILE%\\.claude\\projects` |
| Codex CLI | `$CODEX_HOME/sessions` | `~/.codex/sessions` | `%USERPROFILE%\\.codex\\sessions` |
| OpenCode | `$XDG_DATA_HOME/opencode/opencode.db` | `~/.local/share/opencode/opencode.db` | `%USERPROFILE%\\.local\\share\\opencode\\opencode.db` |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini` | `~/.gemini` | `%USERPROFILE%\\.gemini` |
| OpenClaw | — | `~/.openclaw/agents` | `%USERPROFILE%\\.openclaw\\agents` |

> Nota: las variables de entorno tienen prioridad sobre las rutas de respaldo.

### 2) Sesiones en tiempo real con ACP

Actualmente soporta 5 agentes: Claude Code, Codex CLI, Gemini CLI, OpenCode y OpenClaw.

### 3) Configuración de Skills

- Soportados: `Claude Code / Codex / OpenCode / Gemini CLI / OpenClaw`
- Se añadirán más adaptadores progresivamente

### 4) Aplicaciones destino de MCP

Destinos de escritura actuales:

- Claude Code
- Codex
- OpenCode

## Inicio rápido

### Requisitos

- Node.js `>=22` (recomendado)
- pnpm `>=10`
- Rust stable (2021 edition)
- Dependencias de compilación de Tauri 2

Ejemplo para Linux (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Desarrollo

```bash
pnpm install

# Aplicación de escritorio completa (Tauri + Next.js)
pnpm tauri dev

# Solo frontend
pnpm dev

# Exportación estática del frontend a out/
pnpm build

# Compilación de escritorio
pnpm tauri build

# Lint
pnpm eslint .

# Verificaciones de Rust (ejecutar en src-tauri/)
cargo check
cargo clippy
cargo build
```

## Arquitectura

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

## Restricciones

- El frontend usa exportación estática (`output: "export"`)
- Sin rutas dinámicas de Next.js (`[param]`); se usan parámetros de consulta en su lugar
- Parámetros de comandos Tauri: `camelCase` en frontend, `snake_case` en Rust
- TypeScript en modo estricto

## Privacidad y seguridad

- Enfoque local por defecto para análisis, almacenamiento y operaciones de proyecto
- El acceso a la red solo ocurre mediante acciones iniciadas por el usuario
- Soporte de proxy del sistema para entornos empresariales

## Licencia

Apache-2.0. Ver `LICENSE`.
