# 会话消息内容搜索实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Ctrl+K 的会话搜索在标题之外还能命中用户与助手正文，同时保持 45ms 内容查询预算、低存储和后台首建索引。

**Architecture:** 新增 SQLite 文档表和两个可选的 FTS5 倒排表；解析器输出经过规范化后由后台索引器增量写入；查询服务合并标题结果与正文结果并生成摘要片段；前端改为服务端过滤并显示正文摘要。

**Tech Stack:** Rust + SeaORM/SQLite FTS5 + Tokio；Next.js 16 + React 19 + cmdk。

## Global Constraints

- 仅索引 `MessageTurn` 中 `role` 为 `User` 或 `Assistant` 的 `ContentBlock::Text`。
- 每个文本块 UTF-8 上限 8,192 字节，截断必须在合法字符边界。
- 搜索范围沿用现有语义：选中文件夹时限定文件夹，否则全部可见文件夹，保留 Agent 过滤。
- 扫描模式延迟预算 45ms；可索引文本默认阈值 40MB，短词索引与 trigram 同阈值。
- 候选召回上限 `max(1, 当前查询范围的可见会话数)`，默认不静默截断。
- 排序：标题命中优先，正文按相关度，同相关度按 `updated_at DESC`。
- 搜索路径不得读取转录文件，索引写入不得在搜索请求路径执行。
- 不改动既有 `list_all_conversations` 和“文件”页签行为。
- 遵循现有 Rust/TypeScript 代码风格；Rust 错误类型使用 `thiserror`/`DbError`。

---

## File Structure

- Create `src-tauri/src/search/mod.rs`：公开 normalizer、query、indexer 模块。
- Create `src-tauri/src/search/normalizer.rs`：纯文本提取、截断、哈希、短词分词。
- Create `src-tauri/src/search/query.rs`：查询词拆分、LIKE/FTS 表达式生成。
- Create `src-tauri/src/search/indexer.rs`：后台队列、首建、漂移和进度。
- Create `src-tauri/src/db/entities/message_search_document.rs`。
- Create `src-tauri/src/db/entities/search_index_state.rs`。
- Create `src-tauri/src/db/service/message_search_service.rs`。
- Create `src-tauri/src/db/migration/m20260814_000001_message_search.rs`。
- Create `src-tauri/src/commands/search.rs`。
- Create `src-tauri/src/web/handlers/search.rs`。
- Modify `src-tauri/src/models/conversation.rs` 或 `models/mod.rs`：搜索模型。
- Modify `src-tauri/src/lib.rs`、`db/mod.rs`、`db/entities/mod.rs`、`db/service/mod.rs`、`db/migration/mod.rs`、`web/router.rs`。
- Modify `src/lib/api.ts`、`src/lib/tauri.ts`、`src/lib/types.ts`。
- Modify `src/components/conversations/search-command-dialog.tsx`。
- Modify `src/contexts/search-dialog-context.tsx` 附近的前端状态或新建 hook。
- Modify `i18n/messages/*.json`。

## Task 1: 数据模型与迁移

**Files:**
- Create: `src-tauri/src/db/entities/message_search_document.rs`
- Create: `src-tauri/src/db/entities/search_index_state.rs`
- Create: `src-tauri/src/db/migration/m20260814_000001_message_search.rs`
- Modify: `src-tauri/src/db/entities/mod.rs`
- Modify: `src-tauri/src/db/migration/mod.rs`
- Modify: `src-tauri/src/db/entities/prelude.rs`

**Interfaces:**
- Produces entities `MessageSearchDocument` 和 `SearchIndexState`，表名 `message_search_document`、`search_index_state`。
- Produces migration name `m20260814_000001_message_search`，放在 `m20260807_000001_work_task_scheduled_at` 之后。
- `message_search_document` 列：`id`、`conversation_id`、`text`、`content_hash`、`source_ended_at`、`source_message_count`、`updated_at`；唯一索引 `idx_message_search_document_conversation`。
- `search_index_state` 单行表，主键 `id` 带 CHECK 约束，列 `schema_version`、`mode`、`threshold_mb`、`short_fts_enabled`、`short_threshold_mb`、`scan_ms_per_mb`、`indexed_conversation_count`、`last_calibration_at`、`last_backfill_at`、`user_enabled`（默认 1）、`user_mode`（默认 `auto`）。
- 迁移用 raw SQL 创建两个空 FTS5 表：

```sql
CREATE VIRTUAL TABLE message_search_trigram USING fts5(
  text, content='', contentless_delete=1, detail=none, tokenize='trigram'
);
CREATE VIRTUAL TABLE message_search_short USING fts5(
  words, bigrams, content='', contentless_delete=1, detail=none,
  tokenize='unicode61 remove_diacritics 2'
);
```

- [ ] 参考 `m20260803_000001_token_usage.rs` 和 `conversation.rs` 定义两个实体，时间列使用 `DateTimeUtc`。
- [ ] 参考 `m20260803_000001_token_usage.rs` 写 `up`，用 `Table::create()` 建两张普通表；再用 `manager.get_connection().execute_unprepared(...)` 建两个虚拟表和 `search_index_state` 初始行。
- [ ] 写 `down` 删除两张虚拟表和两张普通表。
- [ ] 在 `entities/mod.rs`、`service/mod.rs` 暂时只注册实体模块，在 `migration/mod.rs` 注册迁移。
- [ ] 运行 `cargo check --features test-utils`，再运行 `cargo test --features test-utils db::` 确认迁移可执行。
- [ ] 提交：`feat(search): add message search schema and migration`。

## Task 2: 文本规范化与查询表达式

**Files:**
- Create: `src-tauri/src/search/mod.rs`
- Create: `src-tauri/src/search/normalizer.rs`
- Create: `src-tauri/src/search/query.rs`
- Modify: `src-tauri/src/lib.rs`（注册 `pub mod search;`）

**Interfaces:**
- `normalize_turns(turns: &[MessageTurn]) -> NormalizedDocument`，`NormalizedDocument { text: String, content_hash: String }`。
- `short_index_tokens(text: &str) -> ShortIndexTokens`，`ShortIndexTokens { words: String, bigrams: String }`。
- `split_terms(query: &str) -> Vec<String>`，最多 8 个非空词。
- `escape_like(term: &str) -> String`，转义 `\`、`%`、`_`。
- `like_pattern(term: &str) -> String`，返回 `%escaped%`。
- `trigram_expression(term: &str) -> Option<String>`，长度少于 3 个 Unicode 字符时返回 `None`；否则生成相邻三字符带引号短语，引号按 FTS5 规则加倍，并用 ` AND ` 连接。
- `short_query(term: &str) -> ShortTermQuery`，枚举 `CjkUnigram`、`CjkBigram { phrase: String }`、`LatinPrefix { token: String }`。

- [ ] 先写 `src-tauri/src/search/normalizer.rs` 的 `#[cfg(test)]` 测试：用户/助手文本保留、Thinking/System/Tool 丢弃、空块丢弃、8192 截断、SHA-256 稳定、中文单字/二字词和拉丁词分词。
- [ ] 运行 `cargo test --features test-utils search::normalizer`，确认先失败。
- [ ] 实现 normalizer；CJK 判定覆盖 U+3400-U+9FFF、U+F900-U+FAFF、U+20000-U+2FA1F、平假名/片假名、谚文。拉丁分词用 `char::is_alphanumeric` 手写扫描，避免额外依赖。
- [ ] 运行测试确认通过。
- [ ] 同样以 TDD 实现 `query.rs` 的转义、词拆分和三字短语生成，测试包含中文、引号、`%`、`_`、少于 3 字符的词。
- [ ] 运行 `cargo fmt`、`cargo clippy --all-targets --features test-utils -- -D warnings`。
- [ ] 提交：`feat(search): add transcript normalization and query builders`。

## Task 3: 文档与状态 Service

**Files:**
- Create: `src-tauri/src/db/service/message_search_service.rs`
- Modify: `src-tauri/src/db/service/mod.rs`
- Modify: `src-tauri/src/db/error.rs`（如需）

**Interfaces:**
- `ensure_search_state(conn) -> Result<SearchIndexStateModel, DbError>`。
- `get_search_state(conn) -> Result<SearchIndexStateModel, DbError>`。
- `set_search_mode(conn, mode: &str, short_fts_enabled: bool) -> Result<(), DbError>`。
- `upsert_document(conn, conversation_id: i32, doc: &NormalizedDocument, source_ended_at: Option<String>, source_message_count: i32, sync_trigram: bool, sync_short: bool) -> Result<i64, DbError>`，返回文档 `id`。
- `delete_document(conn, conversation_id: i32, sync_trigram: bool, sync_short: bool) -> Result<(), DbError>`。
- `total_indexed_text_bytes(conn) -> Result<i64, DbError>`。
- `visible_conversation_count(conn, folder_ids: Option<Vec<i32>>, agent_type: Option<AgentType>) -> Result<i64, DbError>`。
- `list_documents_by_conversation(conn, ids: &[i32]) -> Result<Vec<(i32, String, String)>, DbError>`。

- [ ] 写失败测试覆盖：插入/更新保留 id、删除同步 FTS rowid、无孤儿行、重复会话唯一索引、单行 state。
- [ ] 用 `conn.transaction()` 保证文档与 FTS 写原子；contentless 表删除用 `DELETE FROM ... WHERE rowid = ?`，插入用 `INSERT INTO ...(rowid, ...) VALUES(?, ...)`。
- [ ] 运行 `cargo test --features test-utils db::service::message_search_service`。
- [ ] 提交：`feat(search): add document and index-state services`。

## Task 4: 查询服务、模型与双模式命令

**Files:**
- Modify: `src-tauri/src/models/conversation.rs`（或新增 `models/search.rs`）
- Modify: `src-tauri/src/models/mod.rs`
- Create: `src-tauri/src/commands/search.rs`
- Create: `src-tauri/src/web/handlers/search.rs`
- Modify: `src-tauri/src/lib.rs`（invoke handler）
- Modify: `src-tauri/src/web/router.rs`

**Interfaces:**
- 模型 `SearchMatchKind`、`DbConversationSearchResult`，字段与规格一致。
- `search_conversations_core(conn, folder_ids: Option<Vec<i32>>, agent_type: Option<AgentType>, query: String, limit: u64) -> Result<Vec<DbConversationSearchResult>, AppCommandError>`。
- `get_search_index_status_core(conn) -> Result<SearchIndexStatus, AppCommandError>`。
- `#[tauri::command] search_conversations(...)`、`get_search_index_status(...)`；HTTP 参数结构 `SearchConversationsParams`。

- [ ] 查询服务先实现扫描模式：每词 `SELECT conversation_id FROM message_search_document WHERE text LIKE ? ESCAPE '\'`，Rust 求交集；标题结果复用 `conversation_service::list_all`。
- [ ] 实现 FTS 模式：状态 `mode == "fts"` 时 3 字符以上词使用 `message_search_trigram`，按 `bm25` 取候选上限 `max(1, visible_conversation_count)`；1 到 2 字符词在 `short_fts_enabled` 时使用 `message_search_short`，否则扫描文档表。
- [ ] join `conversation` 复用可见性过滤；标题优先合并；生成 snippet 时只取最终 50 条文本，用 Unicode lowercase 副本定位，返回 `snippet_prefix/match/suffix`。
- [ ] 写测试覆盖标题与正文合并、文件夹/Agent 过滤、多词交集、LIKE 转义、FTS 三字候选、空查询、50 条上限和 snippet 边界。
- [ ] 运行 `cargo test --features test-utils commands::search`、`cargo check --no-default-features --bin codeg-server`。
- [ ] 提交：`feat(search): add content search query service and endpoints`。

## Task 5: 后台索引器与事件钩子

**Files:**
- Create: `src-tauri/src/search/indexer.rs`
- Modify: `src-tauri/src/search/mod.rs`
- Modify: `src-tauri/src/app_state.rs`
- Modify: `src-tauri/src/bin/codeg_server.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/commands/conversations.rs`
- Modify: `src-tauri/src/db/service/import_service.rs`（如需）
- Modify: `src-tauri/src/acp/lifecycle.rs` 或 manager 的 TurnComplete 路径（仅投递钩子）
- Modify: `src-tauri/src/web/event_bridge.rs`（新增 `SEARCH_INDEX_PROGRESS_EVENT`）

**Interfaces:**
- `MessageSearchIndexer::spawn(conn: DatabaseConnection, emitter: EventEmitter) -> Arc<Self>`。
- `request_parse(&self, conversation_id: i32)`、`submit_turns(&self, conversation_id: i32, turns: Vec<MessageTurn>)`、`request_delete(&self, conversation_id: i32)`。
- `IndexStatus { mode, indexed_conversations, total_conversations, building, progress }`。

- [ ] 先写队列去重和 diff 逻辑测试：同一会话重复投递只入队一次；内容哈希不变时不写；哈希变化时 upsert 并同步已启用的 FTS。
- [ ] worker 内解析在 `spawn_blocking` 执行，解析器选择复用 `get_folder_conversation_core` 的 `match agent_type` 逻辑；默认最多 2 个并发，每个会话处理后 `tokio::task::yield_now().await`。
- [ ] 启动时 `ensure_search_state`；状态缺失或 `schema_version` 落后时按 `updated_at DESC` 排队可见会话；每 10 分钟做一次漂移比较。
- [ ] 在 `get_folder_conversation_with_live_core` 解析成功后投递已解析 turns；在导入新增/更新、TurnComplete 后投递 parse；删除路径先删文档再删 FTS。
- [ ] 进度写回 `search_index_state` 并通过 `SEARCH_INDEX_PROGRESS_EVENT` 广播。
- [ ] 桌面与服务器启动路径都持有同一个 `Arc<MessageSearchIndexer>`；Tauri 使用 `.manage()`，HTTP AppState 增加字段。
- [ ] 运行 `cargo test --features test-utils search::indexer`、`cargo check --features test-utils`。
- [ ] 提交：`feat(search): add background message indexer and hooks`。

## Task 6: 前端搜索接入与索引进度

**Files:**
- Modify: `src/lib/api.ts`
- Modify: `src/lib/tauri.ts`
- Modify: `src/lib/types.ts`
- Modify: `src/components/conversations/search-command-dialog.tsx`
- Modify: `src/contexts/search-dialog-context.tsx` 或新建 `src/hooks/use-search-index-status.ts`

**Interfaces:**
- `searchConversations(params): Promise<DbConversationSearchResult[]>`。
- `getSearchIndexStatus(): Promise<SearchIndexStatus>`。
- 对话框会话页签调用新接口，`CommandDialog shouldFilter={false}`；正文结果显示 `snippet_prefix + snippet_match + snippet_suffix`。

- [ ] 给对话框测试 mock `searchConversations`，先写“正文命中但标题不含关键词仍显示”的失败测试。
- [ ] 实现接口与渲染，保持文件页签不变。
- [ ] 新增进度 hook，打开搜索框时请求状态，订阅或 5 秒轮询，显示“索引中 x%”。
- [ ] 运行 `pnpm test src/components/conversations/search-command-dialog.test.tsx` 和 `pnpm eslint .`。
- [ ] 提交：`feat(search): show content matches and index progress in search dialog`。

## Task 7: 设置与国际化

**Files:**
- Modify: `src-tauri/src/commands/search.rs`（`get_search_settings_core`、`set_search_settings_core`，读写 `search_index_state.user_enabled/user_mode`）
- Modify: `src-tauri/src/web/handlers/search.rs` 与路由
- Modify: `src/lib/api.ts`、`src/lib/tauri.ts`、`src/lib/types.ts`
- Modify: 设置页新增 `SearchSettingsSection`，调用上述命令，不依赖 localStorage
- Modify: `i18n/messages/en.json`、`zh-CN.json`、`zh-TW.json`、`ja.json`、`ko.json`、`es.json`、`de.json`、`fr.json`、`pt.json`、`ar.json`

**Interfaces:**
- 设置值保存在 `search_index_state.user_enabled` 与 `user_mode`，`user_mode` 取值 `auto|scan|fts`；前端设置页展示一个开关和一个三选一模式。
- 文案键 `Folder.search.indexing`、`Folder.search.contentMatch` 等。

- [ ] 先实现并测试两个设置命令，再写设置页；`user_enabled=0` 时 `search_conversations_core` 只返回标题结果，`user_mode` 覆盖自动阈值切换。
- [ ] 补齐 10 种语言文案，中文为“正在建立内容索引”“正文匹配”。
- [ ] 运行 `pnpm test` 相关设置测试和 `pnpm eslint .`。
- [ ] 提交：`feat(search): add search settings and i18n`。

## Task 8: 基准、恢复校验与最终加固

**Files:**
- Create: `src-tauri/src/search/bench.rs` 或 `src-tauri/tests/search_perf.rs`
- Modify: `src-tauri/src/db/mod.rs`（启动恢复校验钩子，如适用）

**Interfaces:**
- 无新公共接口，仅测试和启动校验。

- [ ] 构造 100k 轮次种子数据，测量扫描模式和 FTS 模式 p50/p95，记录文档表、trigram、短词表体积。
- [ ] 添加恢复校验：文档数与可见会话数不一致时标记全量待建。
- [ ] 运行 `pnpm test`、`cargo test --features test-utils`、`cargo clippy --all-targets --features test-utils -- -D warnings`、`cargo check --no-default-features --bin codeg-server`。
- [ ] 提交：`test(search): add perf benchmarks and recovery checks`。
