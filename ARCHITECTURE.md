# Codeg 架构设计

> 多智能体编码工作台：把 Claude Code、Codex、OpenCode、Gemini、OpenClaw、Cline、Grok、Hermes 等多种 agent CLI 聚合到同一工作区，做会话聚合与多智能体协作。支持桌面安装与服务器/Docker 部署。

本文描述整体设计与关键机制。代码标识符、路径、协议字段保留英文。

---

## 1. 最核心的设计决策：一份业务核心，三种二进制

整个架构围绕一个决策展开——用 Cargo feature flags 让**同一份代码**编译出三种形态，而不是维护三套逻辑：

| 二进制 | feature | 形态 |
|---|---|---|
| `codeg` | `tauri-runtime`（默认） | 完整桌面应用（窗口管理、系统通知、自动更新等） |
| `codeg-server` | 无（`--no-default-features`） | 独立服务器（Axum HTTP + WebSocket + 静态服务） |
| `codeg-mcp` | 无 | per-launch stdio MCP 伴生进程（多智能体委托，见 §6.4） |

支撑这一决策的四个关键抽象：

- **`EventEmitter` 枚举**（`web/event_bridge.rs`）：`Tauri(AppHandle)` 或 `WebOnly(Arc<WebEventBroadcaster>)`。统一了"实时事件如何离开后端"这一分叉点，业务代码不感知运行模式。
- **`_core` 后缀函数**（`commands/`）：接受普通引用参数（`&AppDatabase`、`&EventEmitter`），是同一份业务逻辑，同时被 `#[tauri::command]`（桌面）与 Axum handler（服务器）调用。条件编译约定：`#[cfg(feature = "tauri-runtime")]` 隔离桌面专属代码；`#[cfg_attr(feature = "tauri-runtime", tauri::command)]` 让函数两模式可用、仅桌面标记为命令。
- **`AppState`**（`app_state.rs`）：共享状态容器——`db`、`connection_manager`、`terminal_manager`、`event_broadcaster`、`acp_event_bus`、`emitter`、`delegation_broker` 等。两种模式构造同一个 `AppState`。
- **前端 `Transport`**（`src/lib/transport/`）：用 `__TAURI_INTERNALS__` 探测环境（`detect.ts`），桌面走 `invoke()`、浏览器走 `fetch()` + WebSocket。上层只面对统一的 `Transport` 接口（`call` / `subscribe` / `attach`）。

前端构建同样服务于此：`next.config.ts` 设 `output: "export"` **纯静态导出**，不使用动态路由（`[param]`），一律用查询参数替代——这样同一份前端既能被 Tauri webview 加载，也能被 server 静态服务。

---

## 2. 后端分层（`src-tauri/src/`）

```
┌──────────────────────────────────────────────────────────────┐
│ lib.rs   Tauri 命令注册 + 窗口/生命周期（桌面入口）              │
│ web/     Axum router + handlers + WS + 认证 + 静态服务（服务器入口）│
│          main.rs / bin/                                       │
├──────────────────────────────────────────────────────────────┤
│ commands/  业务逻辑层（_core 双模式共用）                        │
│   conversations.rs · turn_window.rs · chat_* · delegation.rs  │
│   version_control.rs · work_task.rs · ...                     │
├───────────────────────────┬──────────────────────────────────┤
│ acp/  实时运行侧（≈4.8 万行）│ parsers/  历史读取侧              │
│   connection.rs (1.3w)     │   14 个解析器（每 agent 一个）      │
│   manager.rs               │   → 统一 ConversationSummary/Detail│
│   session_state.rs         │   summary_cache（mtime 指纹缓存）  │
│   background_watch.rs      │                                  │
│   event_stream / internal_bus                                 │
│   delegation/（子代理 broker）│                                 │
├───────────────────────────┴──────────────────────────────────┤
│ db/（SeaORM + SQLite）  terminal/（PTY）  process/  supervise/  │
│ models/（共享数据结构，前端 types.ts 是其 TS 镜像）              │
└──────────────────────────────────────────────────────────────┘
```

后端有**两条几乎独立的子系统**，是理解后端的关键：

### 2.1 `parsers/` —— 历史读取侧（离线解析）

每个 agent 一个解析器（`claude.rs`、`codex.rs`、`gemini.rs`、`opencode.rs`、`openclaw.rs`、`cline.rs`、`hermes.rs`、`codebuddy.rs`、`kimi_code.rs`、`pi.rs`、`grok.rs`、`cursor.rs`、`acp_native.rs`…），把散落在本地文件系统的会话文件（JSONL / 整文档 JSON / SQLite）解析成统一的 `ConversationSummary` / `ConversationDetail` / `MessageTurn` 模型（`models/`）。

- 出口模型统一，前端只面对一种数据结构。
- `summary_cache.rs`：进程级缓存，用 `(mtime, size)` 指纹做命中/失效，避免每次列表都全量重解析；带 LRU 上限。
- `turn_window.rs`：把"全量解析"与"传输窗口切片"分离（见 §6.5）。

### 2.2 `acp/` —— 实时运行侧（Agent Client Protocol）

把 agent CLI 作为子进程拉起，通过 ACP 协议双向通信，是整个系统最大、最核心的子系统：

| 模块 | 职责 |
|---|---|
| `connection.rs` | 单连接的 actor / 事件循环（读 agent stdout、发 prompt、收 session/update） |
| `manager.rs` | `ConnectionManager`：所有连接的生命周期（spawn/disconnect/idle sweep） |
| `session_state.rs` | 单会话实时状态：`live_message`、`active_tool_calls`、`pending_permission`、`to_snapshot()` |
| `background_watch.rs` | 对 transcript 做增量 tail（与详情解析复用同一个 `ClaudeRecordAccumulator`） |
| `event_stream.rs` | 每连接事件环形缓冲 + per-connection broadcast |
| `internal_bus.rs` | 进程内 ACP 事件总线（见 §6.2） |
| `delegation/` | 多智能体委托 broker（见 §6.4） |
| `process.rs` / `supervise.rs` | 子进程拉起与监督 |

---

## 3. 前端分层（`src/`）

```
components/   UI（message/ composer、sidebar、settings、tasks、terminal、merge ...）
contexts/     React Context 横切状态
              acp-connections-context（连接/流式）· conversation-runtime-context
              workspace / tab / terminal / delegation / alert ...
stores/       Zustand store（app-workspace-store · conversation-runtime-store · tab-store）
lib/
  transport/  Transport 抽象（tauri-transport vs web-transport；detect/ws-auth/web-auth）
  adapters/   AI 响应 → 组件渲染的适配器
  api.ts      主 API 客户端
  types.ts    Rust 模型的 TypeScript 镜像
i18n/         10 种语言（next-intl，i18n/messages/*.json）
```

- **状态管理**：Context（横切订阅/生命周期）+ Zustand（细粒度 store）。`conversation-runtime-store.ts` 管理每个会话的 detail/optimistic turns/live message 与窗口化加载。
- **路径别名**：`@/*` → `./src/*`。

---

## 4. 两条数据通路

### 4.1 历史读路径（查看旧会话）

```
agent CLI 写的会话文件
  → parsers/ 解析成统一模型（summary_cache 加速）
  → turn_window 按窗口切片（tail / fromIndex / page）
  → 前端 conversation-runtime-store 窗口化装载 + 倒序无限滚动
```

### 4.2 实时运行路径（正在跑的会话）

```
ConnectionManager 拉起 agent CLI 子进程
  ⇄ ACP 协议事件（session/update）
  → connection.rs 事件循环 → session_state 更新
  → EventEmitter
      桌面:  Tauri app.emit
      服务器: WebEventBroadcaster → WebSocket
  → 前端 Transport.attach / subscribe
  → acp-connections-context（16ms 合帧）→ stores → 渲染
```

---

## 5. 数据库（`db/`）

SeaORM + SQLite。`entities/`（conversation、folder、agent_setting、automation、chat_channel、delegation 等）、`migration/`（按日期命名的迁移）、`service/`（查询封装）。`models/` 与 DB 实体分离：`models/` 是面向传输/解析的共享结构，前端 `types.ts` 与其一一对应。

---

## 6. 关键深层机制

### 6.1 ACP 连接事件流

`connection.rs` 是每连接的 actor：从 agent 子进程 stdout 读 ACP `session/update`，转成内部 `AcpEvent`，更新 `SessionState`，再经 `emit_with_state` 发射。`SessionState::to_snapshot()` 产出 wire 友好的 `LiveSessionSnapshot`，供 attach 时的全量快照。

### 6.2 双事件总线（`internal_bus` vs `WebEventBroadcaster`）

后端有**两条**总线，拆分原因写在 `internal_bus.rs` 头部：

- **`InternalEventBus`**：携带 `Arc<EventEnvelope>`，服务**进程内**消费者（lifecycle、pet_state_mapper、chat-channel 订阅者）。 typed 投递，消费者无需 `serde_json::from_value` 逐事件反序列化。
- **`WebEventBroadcaster`**：携带 `Arc<serde_json::Value>`，面向 WS 客户端的 JSON 投递。`Arc` 让 N 个订阅者只增加引用计数，不拷贝可能上 MB 的 JSON 树。

拆分收益：① 后端消费者省去每事件每订阅者的 JSON 解析；② ACP 事件从全局 firehose 移除后，每连接 attach 流成为唯一通路，前端不再需要按 `connectionId` 去重。

### 6.3 Attach 协议（Subscribe-with-Snapshot，`web/ws_attach.rs`）

取代旧的"订阅全局 firehose + 单独 HTTP 拉快照"两步。客户端对某连接发 `attach`，服务器在 `SessionState` 读锁内**原子地**二选一：

- **`snapshot`**：全量快照（`since_seq=None`，或缺口太大时）；
- **`replay`**：用每连接环形缓冲增量补齐（`since_seq` 已知时）。

之后该连接的实时事件以 `event` 帧沿同一 WebSocket 推送。环形缓冲（`event_stream.rs`）有界：`RECENT_BUFFER_MAX_BYTES=128KB`、`MAX_COUNT=128`、单事件 `64KB` 上限；消费跟不上时 broadcast `Lagged` → 客户端转为重新 attach（snapshot 兜底）。每连接出站 mpsc 容量 64，靠背压自然节流慢客户端。

### 6.4 多智能体委托（`acp/delegation/`）

父 agent 的 LLM 调内建 MCP 工具 `delegate_to_agent`，即可拉起一个**可以是另一种 agent 类型**的全新 ACP 子会话，并把子 agent 首轮的最终 assistant 文本作为 MCP tool_result 返回：

```
parent LLM ─┐ ToolUse(delegate_to_agent, ...)
            ▼
parent CLI ──stdio──► codeg-mcp（per-launch 伴生进程）
                            │ UDS / named pipe（token 鉴权）
                            ▼
                    DelegationBroker
                            │ ConnectionSpawner trait
                            ▼
              ConnectionManager.spawn_agent / send_prompt_linked
                            ▼
              child ACP session ── TurnComplete ──┐
parent LLM ◄── MCP tool_result ◄── DelegationOutcome ◄┘
```

要点（`broker.rs` / `mod.rs` 文档）：

- **异步**：`delegate_to_agent`  setup 完成即返回 `task_id` ack；LLM 之后用 `get_delegation_status`（可长轮询）取结果，或 `cancel_delegation` 取消。无阻塞 oneshot——运行中的任务只是 `running` map 里的一项，终态事件把它原子迁移进 `completed` 缓存并 `result_notify` 唤醒长轮询。
- **生命周期**：`start_delegation` → 前置检查（开关、深度上限）→ `ConnectionSpawner.spawn` → 以首个 prompt 下发任务（尾部 `DelegationLink` 携带父 `tool_use_id` 与 broker 内部 `call_id`）→ 注册 `RunningTask` → 终态（`complete_call` / 各类 cancel）解析并拆除子会话。
- **v1 一次性**：子会话首个 `TurnComplete` 后即 resolve + disconnect，不复用会话。
- **结果不落库**：子输出不写入 codeg DB，broker 把完成文本缓存在 `completed`（按父作用域、FIFO 上限）。

前端侧对应 `delegation-context.tsx`：按 `parent_tool_use_id` 维护父↔子绑定，让父的 `delegate_to_agent` 工具卡内联渲染子会话（带 LRU 上限）。

### 6.5 窗口一致性协议（`commands/turn_window.rs`）

长会话只在序列化前切片传输可视窗口。`tailTurns`/`fromIndex`/翻页都基于**完整解析+完整后处理**的 turn 列表切片，保证窗口响应与全量响应对应区域**字节一致**。每个窗口响应带：

- `offset` / `total` / `assistant_before`；
- `prefix_hash`：`turns[0..offset)` 的结构指纹（FNV-1a，逐 turn 喂 `(role_tag, timestamp_millis)`）。刻意**不含 turn id**——id 在 turn 运行时会被改写，而 `(role, millis)` 在 id 改写与内容回填下不变，但插入/删除/移位（压缩/改写的真实形态）会改变它，前端据此检测前缀被改写并安全地重置窗口；
- `uncovered_prefix_max_ts`：未覆盖前缀的最大时间戳，驱动前端 overlay 退休判定。

`prefix_fingerprint` 由 Rust 与 TypeScript 双实现，并有共享测试向量锁定两端一致。

### 6.6 前端流式合帧（`acp-connections-context.tsx`）

高频 token/tool 事件不直接逐条 setState：

- 流式增量进入队列，`flushStreamingQueue` 以 **16ms** 定时批量 flush；
- `tool_call_update` 走 **RAF 合帧**（`BATCH_TOOL_CALL_UPDATES`）。

二者把"每 token 一次渲染"降为"每帧一次"，是长会话流畅度的关键。

---

## 7. 设计亮点速览

- **单一业务核心 + 特性开关分发**，桌面/服务器不漂移（`EventEmitter` / `_core` / `AppState` / `Transport`）。
- **`Arc` 负载 + 双总线**：广播零拷贝；进程内消费者免 JSON 解析；前端免去重。
- **每 agent 一个解析器**，出口模型统一。
- **Subscribe-with-Snapshot**：原子化 snapshot/replay 抉择 + 有界环形缓冲 + Lagged 自愈。
- **窗口一致性协议**：结构指纹让前端能检测压缩/改写，安全翻页。

---

## 8. 目录速查

| 路径 | 说明 |
|---|---|
| `src-tauri/src/lib.rs` | 桌面入口、Tauri 命令注册、模块声明 |
| `src-tauri/src/app_state.rs` | 共享状态 `AppState` |
| `src-tauri/src/web/` | Axum router/handlers/WS/认证/静态服务/event_bridge |
| `src-tauri/src/commands/` | 业务逻辑（`_core` 双模式共用） |
| `src-tauri/src/acp/` | ACP 实时运行侧（连接/状态/事件/委托） |
| `src-tauri/src/parsers/` | 每 agent 历史解析器 + summary_cache |
| `src-tauri/src/db/` | SeaORM 实体/迁移/服务 |
| `src-tauri/src/terminal/` `process.rs` `supervise.rs` | PTY 与子进程 |
| `src/lib/transport/` | 前端 Transport 抽象（tauri/web 自动切换） |
| `src/contexts/` `src/stores/` | 前端状态 |
| `src/lib/types.ts` | Rust 模型的 TS 镜像 |
