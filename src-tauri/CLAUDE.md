[根目录](../CLAUDE.md) > **src-tauri**

# src-tauri — Rust 后端模块

## 模块职责

Codeg 全部后端能力：代理会话文件解析（19 种 CLI）、Agent Client Protocol（ACP）连接管理与多智能体异步委托、Axum HTTP/WebSocket 服务、SeaORM+SQLite 持久化、PTY 终端、聊天通道桥接、定时自动化、备份加密、原地自升级。同一份代码按 Cargo feature 编译出三种二进制。

## 入口与启动（三个二进制）

| 二进制 | 入口 | Feature 要求 | 说明 |
|--------|------|--------------|------|
| `codeg` | `src/main.rs` | `tauri-runtime`（默认） | 桌面应用：窗口管理、通知、托盘、自动更新、开机自启 |
| `codeg-server` | `src/bin/codeg_server.rs` | 无（`--no-default-features`） | Axum HTTP API + WebSocket；附属模式：`--version`、`--supervise`（Docker PID 1 进程监督，原地升级后重启 worker）、`--credential-helper`（git 凭据协议子进程） |
| `codeg-mcp` | `src/bin/codeg_mcp.rs` | 无 | per-launch stdio MCP 伴生进程；必需参数 `--parent-connection-id`、`--socket-path`、`--token`；工具：`delegate_to_agent`、`check_user_feedback`、`ask_user_question`、`get_session_info`、`create_automation`、`create_work_task`（按 `--features` 分组开关） |

本地命令：`pnpm server:dev` / `pnpm server:build`（根目录代理执行）；Tauri 开发用 `pnpm tauri:before-dev`（先跑 `scripts/prepare-sidecars.mjs`）。

## 对外接口

- **Tauri 命令**（桌面）：`commands/` 下 40+ 文件，`#[cfg_attr(feature = "tauri-runtime", tauri::command)]` 标记；`commands/mod.rs` 汇总注册
- **HTTP API**（服务器/远程桌面）：`web/router.rs` 注册，`web/handlers/` 45 个 handler 文件，统一 `Extension<Arc<AppState>>` 取状态；`web/auth.rs` token 认证，`web/compression.rs` gzip/brotli
- **WebSocket 事件**：`web/ws.rs` + `web/ws_attach.rs`；事件经 `EventEmitter::WebOnly(Arc<WebEventBroadcaster>)` 广播
- **MCP 工具**（给代理 LLM）：`codeg-mcp` stdio JSON-RPC，经 UDS 与父进程往返，重逻辑在 `acp/delegation/{companion,transport}`（`tool_schema.json` 定义 schema）

### sidecar 准备（`scripts/prepare-sidecars.mjs`）

`pnpm tauri:prepare-sidecars`（被 `tauri:before-dev` / `tauri:before-build` 调用）执行三步：解析 target triple（`--target` 参数 → `TAURI_TARGET_TRIPLE` 环境变量 → 宿主 `rustc -vV`）→ `cargo build --release --bin codeg-mcp --no-default-features` → 拷贝产物为 `src-tauri/binaries/codeg-mcp-<triple>{.exe}`，供 Tauri `externalBin` 以裸名 `codeg-mcp` 打包。纯 Node 实现（无 shell），跨平台一致；CI 交叉编译时 release.yml 传 `--target`。本地只改前端迭代时可用 `CODEG_SKIP_SIDECAR=1` 跳过。

## 内部结构（lib.rs 声明的模块）

```
src-tauri/src/
├── main.rs / lib.rs        # 桌面入口 / 模块声明
├── bin/                    # codeg_server.rs、codeg_mcp.rs
├── app_state.rs            # AppState 共享状态（db、连接管理器、终端管理器、EventEmitter）
├── parsers/                # 19 个会话解析器：claude、codex(+code_mode)、gemini、opencode、
│                           #   openclaw、cline、cursor、kimi_code、qoder、grok、hermes、pi、
│                           #   codebuddy、deepseek、antigravity、acp_native…+ summary_cache
├── acp/                    # ACP 连接管理：manager、connection、event_stream、registry、
│   └── delegation/         #   session_state、plan_approval、question、fork…；多智能体异步委托
│                           #   （broker/companion/spawner/transport，UDS 通信）
├── web/                    # Axum 服务：router、handlers/(45)、auth、ws、event_bridge、
│                           #   compression、port_probe、socket_inherit、shutdown
├── commands/               # Tauri 命令层（40+ 文件）；backup/ 子模块（AES-GCM+Argon2 加密备份）
├── db/                     # SeaORM：entities/(21)、migration/(按日期 m2026MMDD_*)、service/、test_helpers
├── models/                 # 共享数据结构（与前端 lib/types.ts 镜像）
├── automation/             # cron 自动化引擎
├── chat_channel/           # 聊天通道：backends/{telegram,lark,weixin}、scheduler、command_dispatcher、webhook、session_bridge
├── forge/                  # GitHub/GitLab 集成（auth、deliver、envelope）
├── work_task/              # 工作任务看板
├── pets/                   # 桌宠市场、codex_import；pet_sessions.rs / pet_state_mapper.rs
├── backgrounds/ office_watch/ network/ terminal/   # 后台任务、Office 监视、网络、PTY(portable-pty)
├── update/ + supervise.rs  # 原地自升级 + 进程监督
└── git_repo.rs git_credential.rs folder_links.rs workspace_transfer.rs …
```

## 关键依赖与配置

- **ACP 栈**：`sacp` / `sacp-tokio` 11.0（`sacp-tokio` 被 `[patch.crates-io]` 指向 `vendor/sacp-tokio` 本地补丁）、`agent-client-protocol-schema` 0.11（启用多个 unstable feature：usage/fork/resume/elicitation/boolean_config）
- **Web**：`axum` 0.8（ws+multipart）、`tower-http`（fs/cors/compression）、`reqwest` 0.12（gzip/brotli 透传）
- **存储**：`sea-orm` 1.1（sqlx-sqlite）、`sea-orm-migration`、`rusqlite` 0.32（同步只读访问 Cursor 的 store.db；**libsqlite3-sys 版本已与 sqlx 对齐**，勿单独升级）
- **桌面**：`tauri` 2（可选，macos-private-api + tray-icon）+ 7 个 tauri-plugin
- **格式解析**：`toml`/`toml_edit`（保格式 TOML 手术式合并）、`serde_yaml`、`zstd`（DeepSeek `session.jsonl.zstd`）、`tar`/`zip`/`async_zip`/`flate2`/`bzip2`
- **安全**：`aes-gcm`（流式）、`argon2`、`keyring`（可选，桌面凭据存储）、`minisign-verify`、`sha2`
- **其他**：`tokio`（process/io-util/net…）、`portable-pty`、`kill_tree`、`notify`、`prost`、`qrcode`、`tracing` + `tracing-appender`
- **资源内嵌**：`include_dir` 打包 `experts/`（专家技能）与 `science/`（科研技能）、`resources/codex`、`resources/opencode` 目录
- **Tauri 配置**：`tauri.conf.json`；权限在 `capabilities/{default,desktop}.json`

## 数据模型

- `db/entities/` 21 个实体：conversation、folder、folder_link、folder_command、agent_setting、custom_agent、model_provider、automation(+run)、chat_channel(+message_log/sender_context/thread_binding)、opened_tab、quick_message、remote_workspace_connection、token_usage_(turn/sync)、work_task(+event/settings/template)、app_metadata
- 迁移按日期命名（`m20260211_000001_init.rs` 起），新增变更在 `db/migration/` 建新文件并登记到 `migration/mod.rs`
- `models/` 为 API/前端共享 DTO；`db/service/` 为各实体 CRUD 服务层

## 条件编译约定

- `#[cfg(feature = "tauri-runtime")]` — 仅桌面编译（窗口、通知、`tauri::State` 参数）
- `#[cfg_attr(feature = "tauri-runtime", tauri::command)]` — 函数始终编译，桌面模式额外注册为命令
- `#[cfg(feature = "test-utils")]` — 测试脚手架（`AppState::new_for_test`、`EventEmitter::test_web_only`、parser `with_base_dir`、`db::test_helpers`），release 物理不编译
- `_core` 后缀函数 — 接受 `&AppDatabase`/`&EventEmitter` 普通引用，供 Web handler 与 Tauri 命令共用

## 测试与质量

- 单元测试：各模块 `#[cfg(test)]`；运行 `cargo test --features test-utils`
- 集成测试 `tests/`（13 个）：api_integration、backup_api、parsers_snapshot（insta 快照）、codex_corpus_differential、delegation_columns、delegation_e2e_uds、delegation_e2e_windows、ws_attach、antigravity_trajectory、credential_helper_subprocess、office_watch_proxy、log_file_budget、sanity
- dev-dependencies：`insta`（JSON+redactions）、`axum-test`（含 ws）、`temp-env`、`tempfile`
- Lint：`cargo clippy --all-targets --features test-utils -- -D warnings`；服务器模式用 `--no-default-features --bin codeg-server`
- 快照更新：`cargo insta review` 或 `INSTA_UPDATE=auto`

## 常见问题 (FAQ)

- **为什么 sacp-tokio 要 vendor？** 上游 crate 需本地补丁，`[patch.crates-io]` 指向 `vendor/sacp-tokio`；升级依赖时保留该段
- **`src-tauri/binaries/` 是什么？** sidecar 二进制的按平台暂存目录（`prepare-sidecars.mjs` 生成），gitignore 产物，通过 release.yml 分发，勿提交
- **rusqlite 为什么钉在 0.32？** 其 `libsqlite3-sys`（0.30）需与 sqlx-sqlite 链接同一份 SQLite，避免符号冲突
- **新增一种代理支持？** `parsers/` 加解析器 + 更新 `parsers/mod.rs` + 补 insta 快照测试
- **HTTP 端点与桌面命令如何共存？** 业务逻辑写 `_core` 函数，`web/handlers/` 与 `commands/` 各自薄封装调用

## 资源内嵌：experts / science 技能包

随二进制内嵌（`include_dir`），运行时只读：

| 目录 | 规模 | 内容 |
|------|------|------|
| `experts/` | 14 个技能 + `experts.toml` | 编码工作流技能（brainstorming、test-driven-development、systematic-debugging、writing-plans 等 superpowers 系列） |
| `science/` | 13 个技能 + `science.toml` + `NOTICE.md` | 科研技能（experimental-design、statistical-analysis、paper-lookup、peer-review 等） |

- 注册表约定：`experts.toml` 的 `category` 必须匹配 `commands/experts.rs` 的 `ExpertCategory` 枚举；`icon` 为 lucide-react 图标名；`display_name`/`description` 按 10 语言 locale 提供（缺失回退 en），locale 集合与前端 i18n 一致。
- 同步脚本：`scripts/sync-science-skills.sh` 负责同步 science 技能资源。

## 相关文件清单（高信号）

- `Cargo.toml`（feature/二进制定义）、`tauri.conf.json`、`build.rs`、`capabilities/*.json`
- `src/lib.rs`（模块声明）、`src/app_state.rs`、`src/main.rs`、`src/bin/*.rs`
- `src/web/router.rs`、`src/web/event_bridge.rs`、`src/commands/mod.rs`
- `src/acp/mod.rs`、`src/acp/manager.rs`、`src/acp/delegation/mod.rs`
- `src/db/mod.rs`、`src/db/migration/mod.rs`、`src/models/mod.rs`
- `experts/experts.toml`、`science/science.toml`

## HTTP API 端点概览（web/router.rs 实测）

- 挂载点：全部 API 经 `.nest("/api", api)` 挂载于 `/api` 前缀下；另有 `/ws/events` WebSocket 事件流。
- 规模：约 348 条 `.route()` 注册，几乎全部为 `POST`（JSON-RPC 风格，动词式路径如 `/acp_prompt`、`/git_commit`）。
- 域分布（按 handler 模块 → 路由数）：acp 67（含 agent 下载/注册表/登录/诊断）、git 51、folders 32、work_task 31、chat_channel 22、conversations 21、pet 19、forge 17、office_tools 14、files 12、version_control 11、automation 11、custom_skills 10、web_server 8、system_settings 8、science 8、mcp 8、experts 8、backup 7、folder_links 6、folder_commands 6、workspace_files 5、terminal 5、quick_messages 5、project_boot 5、logging 5、token_usage 4、model_provider 4、app_update 4 等。
- 约定：新增端点 = `web/handlers/<域>.rs` 加 handler（`Extension<Arc<AppState>>` 取状态）→ `web/router.rs` 注册一条 `.route()`，路径保持下划线动词式命名。

## 变更记录 (Changelog)

- **2026-08-31 18:10:49 — 初始化架构师生成**：基于全仓扫描创建本模块文档（329 个 Rust 源文件清点；lib.rs mod 声明、Cargo.toml 三二进制与 feature、19 解析器、21 实体、13 集成测试均经源码核实）。
- **2026-08-31 18:50 — 补扫**：补入 experts/science 技能包结构与注册表约定、`web/router.rs` HTTP API 端点概览（348 路由实测统计）、`scripts/prepare-sidecars.mjs` sidecar 机制说明。
