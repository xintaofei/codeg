# 会话消息内容搜索设计规格

Status: 待评审，评审通过后进入实现

Date: 2026-08-14

Branch: `task/1`

## 1. 背景与问题

工作区左侧的搜索功能由 `Ctrl+K` 打开。会话页签目前只按会话标题搜索：

- 前端 [search-command-dialog.tsx](../../src/components/conversations/search-command-dialog.tsx) 调用 `list_all_conversations`，把用户输入作为 `search` 参数传给后端。
- 后端 `conversation_service::list_all` 只执行 `conversation::Column::Title.contains(s)`，因此聊天正文中的关键词不会被命中。

目标是在不降低现有搜索响应速度、不明显增加存储和 CPU 的前提下，把会话正文纳入搜索结果。正文当前不保存在 codeg 数据库中，而是在打开会话时由各 Agent 解析器从转录文件读取，所以本设计同时解决“何时解析正文”和“如何快速查询正文”两个问题。

## 2. 目标与非目标

目标：

- 支持按用户消息和助手回复的纯文本内容搜索会话。
- 保持搜索框 300ms 防抖体验，内容搜索后端查询目标 p95 不超过 45ms。
- 首次建索引在应用启动后自动后台完成，搜索路径永不触发转录文件解析。
- 尽量控制索引体积，提供扫描模式与 FTS 模式的自适应切换。
- 桌面模式和服务器模式共用同一套核心逻辑。

非目标：

- 不索引 `Thinking`、`System`、工具调用、工具结果、图片和文件内容。
- 不改动“文件”页签，该页签仍只匹配文件名与路径。
- 不做语义搜索、向量搜索或远程搜索服务。
- 不改变既有 `list_all_conversations` 的标题搜索接口行为。

## 3. 已确认决策

1. 索引范围仅限 `MessageTurn` 中 `role` 为 `User` 或 `Assistant` 的 `ContentBlock::Text` 文本。
2. 搜索范围与现在一致：选中文件夹时只搜该文件夹；无文件夹时搜全部非删除文件夹，并保留 Agent 过滤。
3. 首次建索引指升级到该功能版本后的第一次启动。启动检测到未建索引后立即后台静默执行，后续启动只做增量同步。
4. 排序：标题命中优先；内容命中按相关度，相同相关度按 `updated_at` 倒序。
5. 自适应阈值默认 40MB，当前机器实测的 29MB 可索引文本停留在扫描模式。

## 4. 现状与实测基线

在本机对可发现的 Agent 会话做抽样和全量统计，结果如下：

| 来源 | 原始会话体积 | 用户 + 助手纯文本 |
| --- | ---: | ---: |
| Codex | 4,266MB，2,314 个文件 | 约 25.0MB，95% 区间 17.5 到 35.2MB |
| Claude | 92.5MB，51 个文件 | 约 1.5MB，区间 1.0 到 2.0MB |
| OpenCode | 832MB SQLite 数据库 | 1.6MB，全量统计 |
| Pi | 15.0MB，24 个文件 | 0.8MB，全量统计 |
| 合计 | 约 5.2GB | 约 29MB，区间约 21 到 40MB |

Codex 抽样 60 个文件共 1.09GB，按文件字节加权；Claude 抽样 20 个文件；OpenCode 和 Pi 全量解析。每条文本按 8KB 截断。

SQLite FTS5 实测，样本 9.55 到 9.67MB 文本：

| 布局 | 相对可索引文本 |
| --- | ---: |
| 每条消息一行保存原文 | 1.15 倍 |
| 每个会话一行保存原文 | 1.02 倍 |
| trigram 倒排，contentless + `detail=none` | 1.34 倍 |
| 短词倒排，contentless unigram + bigram | 1.26 倍 |
| 会话行 + trigram 合计 | 2.36 倍 |

结论：会话行布局下，29MB 文本在扫描模式约占 30MB；开启 trigram 后约占 68MB，95% 区间约 50 到 95MB；若大语料下再启用短词索引，总额外约 3.6 倍。

扫描最坏耗时实测（热缓存，统一采用无匹配查询强制扫完全表；罕见命中会在此基础再增加数毫秒）：

| 文本 | p50 | p95 |
| --- | ---: | ---: |
| 10MB | 8ms | 9ms |
| 29MB | 25ms | 26ms |
| 100MB | 85ms | 87ms |
| 200MB | 178ms | 185ms |

因此 150 到 200MB 阈值过晚，本设计采用 40MB 默认阈值。

## 5. 架构总览

新增模块：

- `MessageSearchNormalizer`：从已解析的 `MessageTurn` 提取规范化文本并计算哈希。
- `MessageSearchDocumentService`：维护每会话一行原文文档。
- `MessageSearchIndexer`：后台索引 worker，负责首建、增量、漂移扫描和模式切换。
- `MessageSearchService`：合并标题与正文结果，生成排序和摘要片段。
- `MessageSearchState`：保存模式、阈值、校准值和进度。

数据流：

1. 会话打开时，`get_folder_conversation_core` 已经解析出全部轮次；规范化后的文档异步投递到 worker，不重复读取转录文件。
2. 导入、live turn 完成、会话删除等事件同样投递 worker。
3. worker 在后台按会话做 diff，只写新增或变化的文档。
4. Ctrl+K 搜索只读 SQLite：标题查询走现有服务，正文查询走文档表或 trigram 表。
5. 可索引文本超过阈值时，worker 后台构建 trigram 表，完成后原子切换到 FTS 模式；短词查询持续超预算时再构建短词倒排。

## 6. 数据模型与迁移

### 6.1 文档表

```sql
CREATE TABLE message_search_document (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    source_ended_at TEXT,
    source_message_count INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_message_search_document_conversation
    ON message_search_document(conversation_id);
```

`id` 同时是 trigram 和短词表的 rowid。更新文档时保留 `id`，避免两个 FTS rowid 漂移；若实现选择删除重建，则必须先删除旧 FTS rowid。`content_hash` 是规范化文本的 SHA-256 十六进制值，用于 diff。`source_ended_at` 与 `source_message_count` 来自解析器列表摘要，用于漂移检测。

### 6.2 trigram 表

迁移时创建空表，不插入任何行：

```sql
CREATE VIRTUAL TABLE message_search_trigram USING fts5(
    text,
    content='',
    contentless_delete=1,
    detail=none,
    tokenize='trigram'
);
```

空表只有少量页开销。扫描模式下保持为空；FTS 模式下 `rowid` 与 `message_search_document.id` 一一对应。

### 6.3 短词表

短词表在迁移时创建为空表，默认不填充，仅在短词扫描持续超过延迟预算时写入：

```sql
CREATE VIRTUAL TABLE message_search_short USING fts5(
    words,
    bigrams,
    content='',
    contentless_delete=1,
    detail=none,
    tokenize='unicode61 remove_diacritics 2'
);
```

`words` 保存拉丁小写单词和中文单字，`bigrams` 保存中文连续二字组，两者都以空格分隔。`rowid` 同样与 `message_search_document.id` 一一对应。

### 6.4 状态表

```sql
CREATE TABLE search_index_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    mode TEXT NOT NULL DEFAULT 'scan',
    threshold_mb REAL NOT NULL DEFAULT 40.0,
    short_fts_enabled INTEGER NOT NULL DEFAULT 0,
    short_threshold_mb REAL NOT NULL DEFAULT 40.0,
    scan_ms_per_mb REAL,
    indexed_conversation_count INTEGER NOT NULL DEFAULT 0,
    last_calibration_at TEXT,
    last_backfill_at TEXT
);
```

首次启动时插入单行状态。schema 升级时递增 `schema_version` 并触发重扫。

### 6.5 迁移

在现有 `m20260807_000001_work_task_scheduled_at` 之后新增迁移。两个虚拟表用 raw SQL 创建。备份与恢复沿用 SQLite 全库备份，新表自动包含在内；恢复后启动校验文档数量与有 `external_id` 的可见会话数量，不一致则重新排队。

## 7. 文本规范化

对每个 `MessageTurn`：

1. 仅接受 `role` 为 `User` 或 `Assistant`。
2. 遍历 `blocks`，只保留 `ContentBlock::Text { text }`。
3. 每个文本块先 `trim`，丢弃空串，UTF-8 字节超过 8,192 时截断到合法字符边界。
4. 同一会话的所有块按原始顺序以 `\n\n` 连接。
5. 对最终文本计算 SHA-256，十六进制小写保存为 `content_hash`。

系统注入和内部上下文在解析层即被排除，不依赖查询候选上限。Codex 的 `session_meta.base_instructions`、`response_item` 中 developer/system 及文本型 user 消息、`environment_context`、`turn_context`、reasoning 不进入 `MessageTurn` 的 `Text` 块；Claude、Pi、OpenCode 同样只取显式 user 或 assistant 的 `type: text` 块，忽略 thinking、attachment、tool 和 system 内容。

不主动转小写、不做分词。大小写无关由 SQLite `LIKE`（ASCII）和 trigram 折叠处理，中文无需大小写。已知边界：扫描模式的 `LIKE` 大小写折叠覆盖 ASCII；带重音的非 ASCII 字母的大小写变体仅在 FTS 模式由 trigram 的 Unicode 折叠覆盖。

## 8. 索引策略

### 8.1 两种模式

- `scan`：只维护 `message_search_document`，搜索使用参数化 `LIKE`。
- `fts`：维护 `message_search_trigram`，3 个 Unicode 字符及以上的词走 FTS；1 到 2 字符的词先走文档表扫描，短词查询持续超预算后启用 `message_search_short`。

### 8.2 自适应阈值

默认阈值 `threshold_mb = 40.0`，延迟预算 `budget_ms = 45.0`。

首次全量回填完成后做本机校准：

1. 对文档表执行 3 次无匹配 `LIKE` 查询，测量毫秒每 MB。
2. 计算 `threshold_mb = clamp(budget_ms / measured_ms_per_mb, 24.0, 64.0)`。
3. 校准失败时使用 40.0。

切换规则：

- 当前可索引文本字节数达到阈值时，后台构建 trigram 表并切到 `fts`。
- 可索引文本降到阈值的 50%（至少 16MB）以下时，删除 trigram 行并切回 `scan`，避免反复震荡。
- 升级期间继续使用扫描路径，构建完成后原子更新 `mode`。

### 8.3 运行期看门狗

索引器按滚动窗口记录最近 30 次扫描查询耗时。每 10 次计算一次 p95；连续 3 个窗口超过 `budget_ms * 1.5` 时，即使未达到字节阈值也提前升级：若当前是扫描模式则构建 trigram，若瓶颈来自 1 到 2 字符查询则再构建短词索引。覆盖冷缓存和慢磁盘。

### 8.4 短词索引

trigram 无法加速 1 到 2 字符的子串查询，所以单独用短词索引兜底：

1. 默认在可索引文本达到 `short_threshold_mb = 40.0` 时与 trigram 一起构建，保证 1 到 2 字符查询也满足 45ms 预算。
2. 短词扫描看门狗也可提前触发：最近 30 次 1 到 2 字符查询的 p95 连续 3 个窗口超过 `budget_ms * 1.5` 时构建。
3. 文本降到 `short_threshold_mb / 2` 以下且短词 p95 回落后，删除短词索引行并停用。
4. 构建期间短词查询继续走文档表扫描，完成后原子更新 `short_fts_enabled`。

## 9. 查询算法

新增 `search_conversations_core`，输入 `folder_ids`、`agent_type`、`query`、`limit`，输出 `Vec<DbConversationSearchResult>`。

### 9.1 查询归一化

- `trim` 并折叠连续空白。
- 按 Unicode 空白拆词，最多 8 个词。
- 单次查询长度最多 256 个字符。
- 空查询沿用现有最近会话列表行为。

### 9.2 候选召回

每个词独立查询，最终在 Rust 中按 `conversation_id` 求交集，允许多个词出现在同一会话的不同轮次。

扫描模式：

```sql
SELECT conversation_id
FROM message_search_document
WHERE text LIKE ? ESCAPE '\';
```

参数是转义 `%`、`_`、`\` 后的 `%term%`。该模式返回全部命中会话 id，候选集合最多等于可见会话数；相关位置在 Rust 对交集后的集合计算，避免提前截断造成漏召回。

FTS 模式，词长度至少为 3 时：

1. 生成连续三字符片段，每段写成加双引号的 FTS 短语项。
2. 各片段用 `AND` 连接，先取 rowid 候选。
3. join 文档表后以 `text LIKE ?` 精确过滤，消除非相邻片段造成的假阳性。
4. 使用 `bm25(message_search_trigram)` 作为相关度。

每词的候选上限为 `max(1, 当前查询范围内的可见会话数)`。可见会话数按 9.3 的过滤条件计算。索引一行对应一个会话，因此任何单个词最多只能命中可见会话数；该默认值保证不会静默漏召回。若未来可见会话数极大，可显式配置更小上限，但必须把“召回被截断”作为结果元数据返回。

词长度为 1 到 2 时，若 `short_fts_enabled = 1`：

1. 中文单字查 `words` 列的精确项，二字中文按相邻二字组做短语查询。
2. 拉丁词按 `words` 列前缀查询。
3. join 文档表后以 `text LIKE ?` 精确过滤。

未启用短词索引时，短词与扫描模式相同。

### 9.3 过滤、合并与排序

候选 `conversation_id` 继续 join `conversation`，沿用现有可见性条件：

- `deleted_at IS NULL`
- `kind != 'loop'`
- `parent_id IS NULL`
- 文件夹集合与 Agent 过滤

标题结果由现有 `list_all` 查询获取。合并时按会话去重，标题命中整体排前；内容命中按相关度，相关度相同按 `updated_at DESC`。返回上限 50。

扫描模式的相关度使用首个命中位置，位置越靠前越相关；多词时取各词位置之和。FTS 模式使用 `bm25`，多词交集取最差值，保持单调可比。

### 9.4 摘要片段

仅对最终返回的 50 条生成片段：

- 在文档 `text` 中做大小写无关的首个命中定位；定位使用文档与查询词的 Unicode 小写折叠副本，只用于计算偏移，不修改存储文本。
- 向前、向后各取约 80 个字符窗口，窗口边界不切断 UTF-8 字符。
- 返回结构化字段 `snippet_prefix`、`snippet_match`、`snippet_suffix`，避免把高亮标记注入用户文本。

### 9.5 安全与注入

- `LIKE` 使用绑定参数并显式 `ESCAPE`。
- FTS `MATCH` 只由本设计生成的三字符带引号项构成，不把原始输入拼进表达式；引号按 FTS5 规则加倍。
- 所有查询有长度、词数和自适应召回上限，防止恶意超长输入。

## 10. 后台索引器

### 10.1 队列与去重

worker 使用有界队列，按 `conversation_id` 合并重复请求。解析在 `spawn_blocking` 中执行，复用与 `get_folder_conversation_core` 相同的 `AgentParser::get_conversation` 路径。

文档新增或内容哈希变化时，在同一事务内同步 upsert 已启用的 trigram 和短词 FTS rowid；未启用的表保持空。

### 10.2 首建与漂移

首次启动时：

1. 若状态表的 `schema_version` 不存在或落后，标记全量待建。
2. 按 `conversation.updated_at DESC` 排队所有满足可见性条件的会话：`external_id` 非空、`parent_id` 为空、`kind != 'loop'`、`deleted_at` 为空。
3. 每个会话处理后主动让出一次，默认最多 2 个并发解析。

每 10 分钟运行一次漂移检查：

1. 用现有解析器 `list_conversations` 得到廉价摘要。
2. 与会话行及文档行的 `(external_id, source_ended_at, source_message_count)` 比较。
3. 只有摘要变化的会话才排队重新解析。

### 10.3 事件钩子

- 会话详情加载：`get_folder_conversation_with_live_core` 解析完成后，把已有 `turns` 的规范化结果异步投递。
- 导入：`import_summaries` / `import_summaries_resilient` 产生新增或更新时投递会话 id。
- live turn 完成：ACP `TurnComplete` 之后投递会话 id，由 worker 重新解析已落盘的转录。
- 会话软删除：同步删除文档行及 trigram、短词两个 FTS 表中对应的 rowid。

### 10.4 失败与进度

- 单个解析失败记录日志，保留待重试，不中断队列。
- 重试采用退避，连续失败跳过该会话并计入状态。
- 进度通过 `EventEmitter` 发送 `search_index_progress`，桌面模式走 Tauri 事件，服务器模式走 WebSocket。
- 前台打开搜索框时显示“索引中 x%”，标题搜索始终可用。

## 11. API 与前端

### 11.1 共享模型

```rust
pub struct DbConversationSearchResult {
    pub summary: DbConversationSummary,
    pub match_kind: SearchMatchKind, // Title | Content | Both
    pub snippet_prefix: Option<String>,
    pub snippet_match: Option<String>,
    pub snippet_suffix: Option<String>,
    pub content_match_count: u32,
}
```

新增 `search_conversations` 命令和 Axum handler，两者共享 `search_conversations_core`。原 `list_all_conversations` 保持不变。

### 11.2 前端改动

- `search-command-dialog.tsx` 会话页签改用 `searchConversations`。
- `CommandDialog` 在会话页签设置 `shouldFilter={false}`，避免 cmdk 按标题二次过滤正文命中结果。
- 每条结果显示标题、Agent、时间和一个不超过两行的正文摘要。
- 订阅或轮询索引进度，在搜索框显示构建状态。
- 新增设置：`content_search_enabled`、`content_search_mode`（`auto` | `scan` | `fts`）。
- 为 10 种语言补充“正在建立内容索引”“正文匹配”等文案。

文件页签不改变。

## 12. 性能与存储验收

| 指标 | 目标 |
| --- | --- |
| 搜索路径转录文件 I/O | 0 |
| 扫描模式 p95 | 不超过 45ms，且不超过阈值文本量 |
| FTS 模式 3 字符以上 p95 | 不超过 15ms，100k 轮次 |
| FTS 模式短词索引后 1 到 2 字符 p95 | 不超过 15ms，100k 轮次 |
| 首建期间界面 | 无阻塞，后台 2 并发 |
| 文档表体积 | 可索引文本的 1.05 倍以内 |
| trigram 额外体积 | 可索引文本的 1.5 倍以内 |
| 短词索引额外体积 | 可索引文本的 1.5 倍以内 |
| 召回完整性 | 默认候选上限不低于可见会话数，不静默截断 |
| 删除一致性 | 会话删除后无孤儿索引行 |

实现阶段用种子数据跑基准，达不到扫描 p95 目标时优先收紧阈值校准系数，而不是修改 45ms 预算。

## 13. 测试计划

后端单元测试：

- 规范化只保留用户与助手文本、截断、哈希稳定。
- `LIKE` 转义与 FTS 三字符生成，含引号和中文。
- 扫描与 FTS 两种模式的召回、求交集、过滤、合并排序。
- 短词索引的中文单字、二字短语和拉丁前缀查询。
- 自适应召回上限在可见会话数变化时仍不漏命中，超大规模截断可观测。
- 片段窗口与 UTF-8 边界。
- 文档 diff、删除同步、软删除清理。
- 阈值计算、滞后区间和看门狗状态机。
- 迁移 up/down 和恢复后校验。

前端测试：

- 正文命中结果不被 cmdk 过滤。
- 摘要渲染和索引进度显示。
- Agent 过滤与文件夹范围参数正确传递。

基准测试：

- 构造 100k 轮次，分别断言扫描与 FTS 模式延迟。
- 记录文档表和 FTS 表实际膨胀比。

## 14. 实施顺序

1. 数据模型迁移、状态表和空 FTS 表。
2. 文本规范化与哈希模块，附单元测试。
3. 文档 service 的 upsert、diff、删除。
4. 后台索引器、事件钩子、进度事件。
5. 查询 service、共享模型、Tauri 命令与 HTTP handler。
6. 前端搜索框切换、摘要、进度和设置。
7. 国际化文案。
8. 性能基准、阈值校准和整体验收。
9. 4 到 5GB 现有会话的后台回填实测。

## 15. 风险与回滚

- FTS 表损坏或构建失败：保持扫描模式，记录错误，后台自动重建。
- 短词索引损坏或构建失败：短词回退到文档表扫描，自动重建。
- 首次回填的磁盘和 CPU：限并发与主动让出，进度可见，可暂停。
- 模式切换期间的查询：原子更新 `mode`，切换前后都可用。
- 数据库写入竞争：每会话小事务，搜索路径只读，WAL 保持读不阻塞。
- 回滚：新表不改变既有标题搜索，回滚该功能只删除新表和设置即可。

## 16. 被否方案

- 实时 grep 转录文件加缓存：冷查询延迟不可控。
- Tantivy 或 Meilisearch：能力强但引入外部索引文件和进程，备份与部署复杂度高。
- `words + bigrams + trigram` 双 FTS：实测膨胀到可索引文本约 10 倍，改为自适应单 trigram。
- 固定 2,000 候选上限：会造成静默漏召回且易与系统提示词排除混淆，改为按可见会话数自适应。
- 搜索时才首次建索引：会引入等待，已改为启动即后台首建。
