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
  <strong>Español</strong> |
  <a href="./README.de.md">Deutsch</a> |
  <a href="./README.fr.md">Français</a> |
  <a href="./README.pt.md">Português</a> |
  <a href="./README.ar.md">العربية</a>
</p>

Codeg (Code Generation) es un espacio de trabajo de codificación multiagente. Unifica varios agentes (Claude Code, Codex CLI, OpenCode, Gemini CLI, OpenClaw, Cline, Hermes Agent, CodeBuddy, Kimi Code, Pi, Grok Build, Cursor, etc.) en un único espacio de trabajo, admite agregación de conversaciones y colaboración multiagente, y permite instalación de escritorio y despliegue en servidor/Docker.

![gallery](../images/gallery.svg)

## Patrocinadores

<table>
  <tr>
    <td align="center" width="220">
      <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg" target="_blank"><img src="../images/compshare.png" alt="Compshare" width="160" /></a><br/>
      <strong><a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">Compshare (UCloud)</a></strong>
    </td>
    <td>¡Gracias a Compshare por patrocinar este proyecto! Compshare es la plataforma de IA en la nube de UCloud, que ofrece planes Plan de agentes con modelos nacionales en suscripción mensual o por uso, desde 49 ¥/mes. También proporciona acceso estable a modelos extranjeros mediante proxy oficial. Compatible con Claude Code, Codex y llamadas a la API. Apto para empresas: alta concurrencia, soporte técnico 24/7 y facturación en autoservicio. ¡Los usuarios que se registren a través de <a href="https://www.compshare.cn/?ytag=GPU_YY_git_codeg">este enlace</a> recibirán 5 ¥ de saldo de prueba gratis!</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE" target="_blank"><img src="../images/sui-xiang.jpg" alt="随想AI中转站" width="200" /></a><br/>
      <strong><a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">随想AI中转站</a></strong>
    </td>
    <td>¡Gracias a 随想AI中转站 por patrocinar este proyecto! 随想AI中转站 es un proveedor de retransmisión de API fiable y eficiente, que ofrece servicios de retransmisión para Claude, Codex, Gemini y más. Las cuentas nuevas reciben 0,5 ¥ de crédito de prueba con cada registro de asistencia diario tras <a href="https://sui-xiang.com/register?aff=JPFCRHHBE8HE">registrarse</a>; las recargas se acreditan 1:1, sin suscripción y con pago por uso. La redundancia multilínea, la recuperación ante desastres entre regiones y la conmutación por error automática mantienen sin interrupciones las conexiones SSE de larga duración.</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://hezu.ink/sign-up?aff=0wVz" target="_blank"><img src="../images/hezu-ink.jpg" alt="合租巴士" width="200" /></a><br/>
      <strong><a href="https://hezu.ink/sign-up?aff=0wVz">合租巴士</a></strong>
    </td>
    <td>¡Gracias a 合租巴士 por patrocinar este proyecto! 合租巴士 es una plataforma de retransmisión de IA fiable y eficiente que ofrece una retransmisión de alta estabilidad para los principales modelos como Codex y Claude Code. La proporción de recarga es transparente (1:1), con subvenciones de tarifa de Codex desde tan solo 0,08. <a href="https://hezu.ink/sign-up?aff=0wVz">Únete al grupo desde el sitio web oficial para obtener 5 USD de crédito de prueba</a>.</td>
  </tr>
  <tr>
    <td align="center" width="220">
      <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta" target="_blank"><img src="../images/onehop.jpg" alt="OneHop" width="120" /></a><br/>
      <strong><a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">OneHop</a></strong>
    </td>
    <td>¡Gracias a OneHop por patrocinar este proyecto! OneHop ofrece a los usuarios de Codeg una única clave de API compatible con OpenAI para cientos de modelos líderes, incluidos GPT, Claude, Gemini, DeepSeek, Kimi y Qwen. Cambia de modelo sin gestionar varias cuentas de proveedores ni modificar tu código una y otra vez, y paga solo por lo que uses. <a href="https://onehop.ai/platform/login?ref=CODEG&utm_source=github&utm_medium=readme_sponsor&utm_campaign=codeg&utm_content=sponsor_cta">Regístrate a través de Codeg</a> para recibir 1 USD de crédito y, además, únete a la comunidad de OneHop y participa en la actividad de bienvenida para obtener 5 USD adicionales, hasta un total de 6 USD en crédito de prueba.</td>
  </tr>
</table>

> ¿Quieres convertirte en patrocinador de Codeg? [Contáctanos por correo electrónico.](mailto:itpkcn@gmail.com)

## Interfaz principal

![Codeg Light](../images/main-light.png#gh-light-mode-only)
![Codeg Dark](../images/main-dark.png#gh-dark-mode-only)

## Colaboración Multi-Agente

![Codeg Light](../images/collaboration-light.png#gh-light-mode-only)
![Codeg Dark](../images/collaboration-dark.png#gh-dark-mode-only)

## Flujo de trabajo de Office

![Codeg Light](../images/office-light.png#gh-light-mode-only)
![Codeg Dark](../images/office-dark.png#gh-dark-mode-only)

## Puntos destacados

- **Agregación de conversaciones** — importa las sesiones de todos los agentes compatibles en un espacio de trabajo unificado
- **Colaboración multi-agente** — dentro de una misma sesión, el agente principal delega en sub-agentes de distintos tipos (p. ej. Claude Code llamando a Codex, Gemini) para completar una tarea de forma conjunta, ejecutándose cada uno como una sesión independiente
- Desarrollo paralelo con flujos integrados de `git worktree`
- **Inicio de Proyecto** — crea nuevos proyectos visualmente con vista previa en tiempo real
- **Documentos Office** — crea, analiza, revisa y edita archivos .docx / .xlsx / .pptx con el toolset officecli integrado; vista previa en tiempo real en pestaña de archivo que se actualiza mientras el agente edita
- **Investigación científica** — habilidades científicas integradas (generación de hipótesis, diseño experimental, estadística, visualización, evaluación crítica, búsqueda bibliográfica) que cualquier agente puede invocar, gestionadas por agente
- **Automatizaciones** — guarda cualquier configuración del compositor como automatización reutilizable que se ejecuta de forma desatendida según cron o bajo demanda
- **Canales de Chat** — conecta Telegram, Lark (Feishu), iLink (Weixin) y más a tus agentes de codificación para notificaciones en tiempo real, interacción completa con sesiones y control remoto de tareas
- Gestión de MCP (escaneo local + búsqueda/instalación desde registro)
- Gestión de Skills (ámbito global y por proyecto)
- Gestión de cuentas remotas de Git (GitHub y otros servidores Git)
- Modo de servicio web — accede a Codeg desde cualquier navegador para trabajo remoto
- **Despliegue como servidor independiente** — ejecuta `codeg-server` en cualquier servidor Linux/macOS, accede desde el navegador
- **Soporte Docker** — `docker compose up` o `docker run`, con token/puerto personalizables, persistencia de datos y montaje de directorios de proyecto
- Registros de ejecución — visor de registros en tiempo real integrado con filtrado y niveles de registro por módulo
- Ciclo de ingeniería integrado (árbol de archivos, diff, cambios git, commit, terminal)

## Agentes compatibles

| Agente       | Ruta de variable de entorno           | Ruta por defecto en macOS / Linux     | Ruta por defecto en Windows                           |
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
| Pi           | `$PI_CODING_AGENT_SESSION_DIR`        | `~/.pi/agent/sessions`                | `%USERPROFILE%\\.pi\\agent\\sessions`                 |
| Grok Build   | `$GROK_HOME/sessions`                 | `~/.grok/sessions`                    | `%USERPROFILE%\\.grok\\sessions`                      |
| Cursor       | `$CURSOR_CONFIG_DIR/chats`            | `~/.cursor/chats`                     | `%USERPROFILE%\\.cursor\\chats`                       |

> Nota: las variables de entorno tienen prioridad sobre las rutas de respaldo.

<details>
<summary><h2>Inicio de Proyecto</h2></summary>

Crea nuevos proyectos visualmente con una interfaz de panel dividido: configura a la izquierda, vista previa en tiempo real a la derecha.

![Project Boot Light](../images/project-boot-light.png#gh-light-mode-only)
![Project Boot Dark](../images/project-boot-dark.png#gh-dark-mode-only)

### Qué ofrece

- **Configuración visual** — selecciona estilo, tema de color, biblioteca de iconos, fuente, radio de borde y más desde menús desplegables; la vista previa se actualiza instantáneamente
- **Vista previa en vivo** — visualiza el aspecto elegido renderizado en tiempo real antes de crear nada
- **Creación con un clic** — presiona "Crear proyecto" y el launcher ejecuta `shadcn init` con tu preset, plantilla de framework (Next.js / Vite / React Router / Astro / Laravel) y gestor de paquetes (pnpm / npm / yarn / bun)
- **Detección de gestores de paquetes** — verifica automáticamente qué gestores están instalados y muestra sus versiones
- **Integración fluida** — el proyecto recién creado se abre directamente en el workspace de Codeg

Actualmente soporta scaffolding de proyectos **shadcn/ui**, con un diseño basado en pestañas preparado para más tipos de proyectos en el futuro.

</details>

<details>
<summary><h2>Canales de Chat</h2></summary>

Conecta tus aplicaciones de mensajería favoritas — Telegram, Lark (Feishu), iLink (Weixin) y más — a tus agentes de codificación IA. Crea tareas, envía mensajes de seguimiento, aprueba permisos, reanuda sesiones y monitorea la actividad directamente desde el chat — recibe respuestas del agente en tiempo real con detalles de llamadas a herramientas, solicitudes de permisos y resúmenes de finalización sin necesidad de abrir un navegador.

Los supergrupos de foro de Telegram también pueden usar [Telegram topic mode](../chat-channels/telegram-topic-mode.md) para vincular cada topic a una sesión de Codeg independiente.

### Canales soportados

| Canal          | Protocolo                   | Estado    |
| -------------- | --------------------------- | --------- |
| Telegram       | Bot API (HTTP long-polling) | Integrado |
| Lark (Feishu)  | WebSocket + REST API        | Integrado |
| iLink (Weixin) | WebSocket + REST API        | Integrado |

> Se planean más canales (Discord, Slack, DingTalk, etc.) para futuras versiones.

</details>

<details>
<summary><h2>Documentos Office</h2></summary>

Trabaja con archivos Word, Excel y PowerPoint como un flujo de trabajo de primera clase. El toolset **officecli** integrado permite a tus agentes crear, analizar, revisar y editar documentos .docx, .xlsx y .pptx — y puedes previsualizar el resultado directamente en Codeg.

### Qué ofrece

- **Crear y editar** — genera nuevos documentos o modifica .docx / .xlsx / .pptx existentes, incluyendo gráficos, tablas y formato
- **Analizar y revisar** — inspecciona la estructura del documento, detecta problemas de formato y revisa el contenido
- **Vista previa en vivo** — abre un .docx / .xlsx / .pptx en una pestaña de archivo y se renderiza en línea, actualizándose automáticamente mientras el agente edita — respaldado por un servidor `officecli watch` permanente (con proxy inverso y autenticación por capacidad para entornos web y servidor)
- **Acciones rápidas** — la página de bienvenida ofrece pestañas de Codificación, Office e Investigación científica que insertan la invocación de habilidad correspondiente y una plantilla de prompt con un solo clic; las habilidades no habilitadas muestran un badge de bloqueo y enlazan a donde puedes activarlas
- **Configuración de Office Tools** — una página de ajustes dedicada instala `officecli` y gestiona sus habilidades mediante una matriz de habilidad×agente: alterna cualquier par (habilidad, agente) y aplica cambios masivos

</details>

<details>
<summary><h2>Investigación científica</h2></summary>

Convierte cualquier agente en un asistente de investigación riguroso. Codeg incluye un conjunto curado de **habilidades de investigación científica** con licencia MIT — desde la ideación hasta el análisis y la redacción — que se instalan en el almacén central de habilidades compartido y se vinculan a los agentes que elijas, exactamente igual que los toolsets de expertos y de office.

### Qué ofrece

- **Habilidades curadas** — generación de hipótesis, diseño experimental, potencia estadística, análisis estadístico, análisis exploratorio de datos, visualización científica, evaluación crítica, revisión por pares, gestión de citas, evaluación de académicos, búsqueda de artículos y esquemas de IA
- **Acciones rápidas** — la pestaña de Investigación científica de la página de bienvenida inserta la invocación de habilidad correspondiente junto con una plantilla de prompt localizada en el compositor con un solo clic
- **Configuración de Ciencia** — una página de ajustes dedicada gestiona las habilidades mediante una matriz de habilidad×agente, con badges que señalan las habilidades que necesitan una clave de API o un entorno de Python

</details>

<details>
<summary><h2>Automatizaciones</h2></summary>

Convierte cualquier configuración del compositor — agente, modelo, prompt, directorio de trabajo y opciones — en una **Automatización** reutilizable que se ejecuta sin abrir la interfaz.

### Qué ofrece

- **Configurar una vez, reutilizar siempre** — guarda una configuración completa del compositor como automatización con nombre
- **Programada o bajo demanda** — ejecútala según un horario cron o lánzala manualmente cuando lo necesites
- **Ejecución desatendida** — las automatizaciones se ejecutan en segundo plano y crean sesiones reales que puedes abrir en el workspace en cualquier momento; tras iniciarlas, regresan automáticamente al workspace

</details>

<details>
<summary><h2>Inicio rápido</h2></summary>

### Requisitos

- Node.js `>=22` (recomendado)
- pnpm `>=10`
- Rust stable (2021 edition)
- Dependencias de compilación de Tauri 2 (solo modo escritorio)

Ejemplo para Linux (Debian/Ubuntu):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

### Binarios

Codeg distribuye tres binarios de Rust desde un único workspace:

| Binario        | Rol                                                                                                          | Compilación                                                                |
| -------------- | ------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------- |
| `codeg`        | Aplicación de escritorio Tauri (ventana, bandeja, actualizador)                                              | `pnpm tauri build` (release) / `pnpm tauri dev` (dev)                      |
| `codeg-server` | Servidor HTTP + WebSocket independiente para despliegues en navegador/sin interfaz                           | `pnpm server:build` / `pnpm server:dev`                                    |
| `codeg-mcp`    | Compañero stdio MCP por lanzamiento que expone la herramienta `delegate_to_agent` a las CLI de agentes (colaboración multi-agente) | `pnpm tauri:prepare-sidecars` (invocado automáticamente por `tauri dev` / `tauri build`) |

`codeg-mcp` debe ubicarse junto a su binario padre en tiempo de ejecución — los instaladores, la imagen Docker y el empaquetador de sidecars de Tauri lo colocan junto a `codeg` / `codeg-server`. Las compilaciones desde fuente y los diseños personalizados pueden anular la búsqueda con la variable de entorno `CODEG_MCP_BIN=/abs/path/codeg-mcp`. Si el compañero falta, la delegación se omite (se registra una única advertencia) y el resto de la sesión del agente sigue funcionando.

### Desarrollo

```bash
pnpm install

# Solo frontend (servidor de desarrollo de Next.js, sin Rust)
pnpm dev

# Exportación estática del frontend a out/
pnpm build

# Aplicación de escritorio completa (Tauri + Next.js, compila automáticamente el sidecar codeg-mcp)
pnpm tauri dev

# Compilación de escritorio de release (incluye codeg-mcp como externalBin)
pnpm tauri build

# Servidor independiente (sin Tauri/GUI necesario)
pnpm server:dev
pnpm server:build                  # binario de release en src-tauri/target/release/codeg-server

# Compilar explícitamente el compañero codeg-mcp (para el triple del host)
pnpm tauri:prepare-sidecars        # salida: src-tauri/binaries/codeg-mcp-<triple>

# Saltar la preparación del sidecar al iterar el frontend cuando no necesitas delegación
CODEG_SKIP_SIDECAR=1 pnpm tauri dev

# Lint
pnpm eslint .

# Pruebas frontend (vitest)
pnpm test
pnpm test:watch
pnpm test:coverage

# Verificaciones de Rust (ejecutar en src-tauri/)
cargo check                                                     # escritorio (features por defecto)
cargo check --no-default-features --bin codeg-server            # modo servidor
cargo check --no-default-features --bin codeg-mcp               # compañero MCP
cargo clippy --all-targets --features test-utils -- -D warnings

# Pruebas de Rust
cargo test --features test-utils                                # escritorio (incl. integración)
cargo test --no-default-features --bin codeg-server --lib       # modo servidor
cargo insta review                                              # aceptar actualizaciones de snapshots del parser
```

> Sugerencia: cuando tengas una compilación reciente de `codeg-mcp` en `src-tauri/target/release/` y quieras apuntar un `codeg-server` lanzado manualmente sin reinstalar, exporta `CODEG_MCP_BIN=$(pwd)/src-tauri/target/release/codeg-mcp`.

### Despliegue del servidor

Codeg puede ejecutarse como un servidor web independiente sin entorno de escritorio.

#### Opción 1: Instalación en una línea (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash
```

Instalar una versión específica o en un directorio personalizado:

```bash
curl -fsSL https://raw.githubusercontent.com/xintaofei/codeg/main/install.sh | bash -s -- --version v0.5.2 --dir ~/.local/bin
```

Luego ejecutar:

```bash
codeg-server
```

#### Opción 2: Instalación en una línea (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/xintaofei/codeg/main/install.ps1 | iex
```

O instalar una versión específica:

```powershell
.\install.ps1 -Version v0.5.2
```

#### Opción 3: Descargar desde GitHub Releases

Los binarios precompilados (con recursos web incluidos) están disponibles en la página de [Releases](https://github.com/xintaofei/codeg/releases):

| Plataforma  | Archivo                            |
| ----------- | ---------------------------------- |
| Linux x64   | `codeg-server-linux-x64.tar.gz`    |
| Linux arm64 | `codeg-server-linux-arm64.tar.gz`  |
| macOS x64   | `codeg-server-darwin-x64.tar.gz`   |
| macOS arm64 | `codeg-server-darwin-arm64.tar.gz` |
| Windows x64 | `codeg-server-windows-x64.zip`     |

```bash
# Ejemplo: descargar, extraer y ejecutar
tar xzf codeg-server-linux-x64.tar.gz
cd codeg-server-linux-x64
CODEG_STATIC_DIR=./web ./codeg-server
```

> Para despliegues desatendidos, inícialo con `--supervise` para que una actualización in situ fallida se revierta automáticamente — consulta [Actualizaciones in situ](#actualizaciones-in-situ).

#### Opción 4: Docker

```bash
# Usando Docker Compose (recomendado)
docker compose up -d

# O ejecutar directamente con Docker
docker run -d -p 3080:3080 -v codeg-data:/data ghcr.io/xintaofei/codeg:latest

# Con token personalizado y directorio de proyecto montado
docker run -d -p 3080:3080 \
  -v codeg-data:/data \
  -v /path/to/projects:/projects \
  -e CODEG_TOKEN=your-secret-token \
  ghcr.io/xintaofei/codeg:latest
```

La imagen Docker utiliza una compilación multi-etapa (Node.js + Rust → runtime Debian slim) e incluye `git` y `ssh` para operaciones con repositorios. Los datos se persisten en el volumen `/data`. Opcionalmente, puedes montar directorios de proyecto para acceder a repositorios locales desde el contenedor.

#### Opción 5: Compilar desde el código fuente

```bash
pnpm install && pnpm build          # compilar frontend
cd src-tauri
cargo build --release --bin codeg-server --no-default-features
cargo build --release --bin codeg-mcp --no-default-features    # compañero de delegación
CODEG_STATIC_DIR=../out ./target/release/codeg-server          # codeg-mcp se detecta como hermano
```

Si mantienes los dos binarios en directorios separados, define `CODEG_MCP_BIN=/abs/path/to/codeg-mcp` para que el runtime pueda seguir encontrando el compañero; sin esto, la delegación multi-agente se desactiva silenciosamente.

#### Actualizaciones in situ

El servidor puede actualizarse a sí mismo desde **Ajustes → Actualización de software**: descarga la versión firmada para su plataforma, reemplaza los binarios y los recursos web en disco, y se reinicia — sin necesidad de volver a desplegar manualmente. Esto es solo para Linux/macOS (desactivado en Windows). La versión anterior se conserva como copia de seguridad, por lo que la misma pantalla ofrece una acción **Revertir** para volver a ella.

**Ejecuta bajo el supervisor para la reversión automática.** Inicia el servidor independiente con `--supervise` para que un proceso recién actualizado que no arranque dentro de la ventana de prueba se revierta automáticamente a la versión anterior:

```bash
CODEG_STATIC_DIR=./web ./codeg-server --supervise
```

Sin `--supervise` el servidor sigue actualizándose in situ (se vuelve a ejecutar a sí mismo), pero la actualización es de mejor esfuerzo: no hay ningún supervisor que revierta automáticamente una versión que no puede arrancar. La imagen Docker ya se ejecuta bajo el supervisor.

**Las actualizaciones en Docker cambian el contenedor, no la imagen.** Una actualización in situ reescribe los binarios y los recursos web dentro de la capa de escritura del contenedor en ejecución, por lo que solo existen en ese contenedor. El volumen `/data` persiste, pero los archivos actualizados **no**: recrear el contenedor — `docker compose up --force-recreate`, un nuevo `docker run`, o recrearlo tras un `docker pull` — vuelve a partir de la imagen y descarta la actualización in situ. (Un `docker pull` por sí solo únicamente actualiza la imagen local; nada se revierte hasta que se recrea el contenedor.) Para que una actualización sea permanente, compila o descarga una imagen con la nueva versión y recrea el contenedor a partir de ella.

#### Configuración

Variables de entorno:

| Variable                       | Valor por defecto      | Descripción                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------------------------ | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `CODEG_PORT`                   | `3080`                 | Puerto HTTP                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `CODEG_HOST`                   | `0.0.0.0`              | Dirección de enlace                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `CODEG_TOKEN`                  | _(aleatorio)_          | Token de autenticación (se imprime en stderr al iniciar)                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `CODEG_DATA_DIR`               | `~/.local/share/codeg` | Directorio de la base de datos SQLite (también raíz de `uploads/`, `pets/`)                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `CODEG_STATIC_DIR`             | `./web` o `./out`      | Directorio de exportación estática de Next.js                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `CODEG_MCP_BIN`                | _(sin definir)_        | Ruta absoluta al compañero `codeg-mcp`. Anula la búsqueda por defecto de hermano-del-ejecutable + `PATH`. Úsalo para compilaciones desde fuente o diseños personalizados donde el compañero reside fuera del directorio de instalación del servidor.                                                                                                                                                                                                                                                                                  |
| `CODEG_SKIP_SIDECAR`           | _(sin definir)_        | Conveniencia solo de frontend para `pnpm tauri dev` / `pnpm tauri build` — cuando vale `1`, omite la compilación del sidecar `codeg-mcp`. La delegación queda desactivada en esa compilación; los artefactos de calidad de release deben dejarla sin definir.                                                                                                                                                                                                                                                                        |
| `CODEG_UPLOAD_MAX_TOTAL_BYTES` | _(sin definir)_        | Límite máximo de bytes totales residentes en `<data dir>/uploads/`. Conteo de bytes en decimal (p. ej. `10737418240` para 10 GiB). Si no se define, vale `0` o tiene un valor no analizable, el límite se desactiva y se imprime una línea de inicio para que la configuración sea visible. El límite se aplica dentro de un único proceso `codeg-server` — los despliegues escalados horizontalmente que comparten un mismo volumen `uploads/` requieren coordinación externa (bloqueo de archivos, Redis, cuota de proxy inverso). |
| `CODEG_UPLOAD_QUOTA_STRICT`    | _(sin definir)_        | Cuando es verdadero (`1` / `true` / `yes` / `on`), aborta el inicio con código de salida 2 si `CODEG_UPLOAD_MAX_TOTAL_BYTES` tiene un valor no analizable, en vez de continuar con un WARN. Úselo cuando su política de seguridad requiera que «la cuota configurada debe ser efectiva».                                                                                                                                                                                                                                             |

</details>

<details>
<summary><h2>Arquitectura</h2></summary>

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

## Privacidad y seguridad

- Enfoque local por defecto para análisis, almacenamiento y operaciones de proyecto
- El acceso a la red solo ocurre mediante acciones iniciadas por el usuario
- Soporte de proxy del sistema para entornos empresariales
- El modo de servicio web utiliza autenticación basada en tokens

## Comunidad

- Escanea el código QR de abajo para unirte a nuestro grupo de WeChat para discusiones, comentarios y actualizaciones

<img src="../images/weixin-light.jpg#gh-light-mode-only" alt="WeChat" width="240" />
<img src="../images/weixin-dark.jpg#gh-dark-mode-only" alt="WeChat" width="240" />

- Gracias a la comunidad de [LinuxDO](https://linux.do) por su apoyo

## Agradecimientos

- [ACP](https://agentclientprotocol.com) — el Agent Client Protocol (ACP) es la base que permite a Codeg conectarse con múltiples agentes
- [Superpowers](https://github.com/obra/superpowers) — impulsa el módulo de habilidades de expertos de Codeg
- [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) — impulsa el flujo de trabajo de documentos Office de Codeg
- [scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills) — impulsa las habilidades de Investigación científica de Codeg (subconjunto con licencia MIT)

## Licencia

Apache-2.0. Ver `LICENSE`.
