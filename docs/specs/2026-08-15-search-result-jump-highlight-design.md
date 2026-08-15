# 搜索命中跳转与闪烁高亮设计

## 背景

当前 Ctrl+K 搜索已经能在会话标题和正文中找到关键词。用户点击搜索结果后进入会话时，还看不到关键词在哪条消息中，也无法快速定位。

本次新增交互目标：

1. 点击搜索结果后自动滚动到第一条命中的正文位置，并让该位置停在屏幕中部附近。
2. 命中的关键词以黄色闪烁后渐隐。
3. 如果命中的是标题，则高亮标题中的关键词并闪烁。
4. 如果正文有多条命中，提供“下一条匹配”按钮，支持循环跳转。

## 范围

- 只针对 Ctrl+K 会话搜索结果进入会话后的跳转。
- 不改变现有标题搜索、正文搜索、文件搜索和索引策略。
- 不持久保存高亮状态，高亮动画结束后恢复普通文本。
- 正文匹配只处理 User/Assistant 文本块；工具、图片、系统提示、思考内容不参与。

## 交互选择

用户已确认选择方案 B：

- 自动定位并闪烁第一条正文命中；
- 显示“下一条匹配”按钮；
- 多条命中按会话顺序循环跳转；
- 标题命中直接高亮标题，不显示“下一条”。

## 架构

### 后端

新增精确匹配位置模型，并返回给前端。

#### 1. 规范化文本块偏移表

`message_search_document` 增加一个 `block_offsets` JSON 字段。每个索引文本块保存：

```json
[
  {
    "turn_id": "turn-01",
    "block_index": 0,
    "start": 0,
    "end": 120,
    "leading_trim": 3
  }
]
```

字段含义：

- `turn_id`：后端 `MessageTurn.id`。
- `block_index`：该 turn 中的 Text block 索引。
- `start` / `end`：在规范化全文中的字符区间，半开区间。
- `leading_trim`：原始 block 文本开头被 `trim()` 去掉的字符数，用于把规范化区间映射回原始 block。

### 2. 搜索结果模型

`DbConversationSearchResult` 增加：

```rust
pub matches: Vec<SearchMatchLocation>,
pub total_match_count: u32,
```

`SearchMatchLocation`：

```rust
pub enum SearchMatchLocationKind {
    Title,
    Content,
}

pub struct SearchMatchLocation {
    pub kind: SearchMatchLocationKind,
    pub turn_id: Option<String>,
    pub block_index: Option<usize>,
    pub char_start: usize,
    pub char_end: usize,
}
```

排序规则：

- 标题命中排在最前。
- 正文命中按 `turn_id` 在会话中的顺序排列；同一 turn 中按 `block_index` 和字符位置排序。
- 结果最多返回 200 条匹配位置，避免极端大文档导致响应膨胀。
- 标题命中只包含 `kind=title`，不包含 `turn_id`/`block_index`。

### 3. 查询路径

- 搜索服务读取最终结果对应的 `message_search_document`，使用 `block_offsets` 和规范化文本计算所有命中位置。
- 查询路径不读取原始转录文件，保持既有约束。
- `snippet_prefix/match/suffix` 继续由第一条内容命中生成。
- `content_match_count` 改为实际统计值，但显示和返回仍可受上限保护。

### 4. 索引与迁移

- 在现有迁移之后新增一个小迁移，给 `message_search_document` 增加 `block_offsets` 字段。
- 将 `search_index_state.schema_version` 提升到 2，触发已有索引重新构建。
- 索引器在 `normalize_turns` 时同时生成 block offset 清单，并在 upsert 文档时写入。
- 旧文档如果没有 `block_offsets`，搜索时降级为“只返回 snippet，不返回精确跳转”，前端只显示无跳转结果，避免破坏搜索。

### 前端

#### 1. 搜索焦点状态

新增一个轻量级搜索焦点状态，保存从搜索结果到会话页的传递信息：

```ts
interface SearchFocus {
  conversationId: number
  query: string
  matches: SearchMatchLocation[]
  activeMatchIndex: number
}
```

搜索对话框选择会话时：

1. 把 `SearchFocus` 写入状态；
2. 照常调用 `openTab`；
3. 对话框关闭。

状态不写入持久化存储。

#### 2. 会话详情加载与跳转

`ConversationDetailPanel` / `MessageListView` 读取当前会话的 `SearchFocus`：

- 等待 detail 加载完成。
- 根据 `turn_id` 找到对应 timeline item 的索引。
- 如果目标 turn 不在当前加载窗口：
  - 调用 `loadOlderTurns`；
  - 最多尝试固定次数；
  - 超过上限则停止并清除搜索焦点。
- 使用虚拟列表的 `scrollApiRef.scrollToIndex(index, { align: "center" })` 或等价接口滚动。
- 滚动完成后再触发高亮。

#### 3. 正文高亮

- 只对当前激活匹配对应的 turn/block 做高亮。
- 高亮操作在渲染后的 DOM 文本节点中进行：
  - 找到目标 block 的文本范围；
  - 用 `TreeWalker` / 文本节点扫描定位关键词；
  - 把命中的文本节点拆开并包成 `<mark data-search-flash>`。
- 不修改 Markdown 解析器和内容数据，避免影响复制、导出、编辑和后续流式更新。
- 动画结束或切换到下一条时移除旧 `<mark>`。

#### 4. 下一条匹配

- 仅当正文匹配数量大于 1 时显示按钮。
- 按钮放在消息列表右侧或滚动按钮附近。
- 点击后：
  - `activeMatchIndex = (activeMatchIndex + 1) % matches.length`；
  - 滚动到新位置；
  - 重新闪烁。
- 按钮显示当前序号和总匹配数，例如 `2 / 8`。

#### 5. 标题命中

- 标题命中 `turn_id` 为空。
- `ConversationDetailHeader` 根据 `char_start` / `char_end` 将标题子串包成 `<mark data-search-flash>`。
- 使用与正文相同的闪烁动画。
- 标题命中不触发消息列表滚动，也不显示“下一条”。

#### 6. 动画

CSS keyframes：

```css
@keyframes search-flash {
  0% { background-color: #fde047; }
  100% { background-color: transparent; }
}

[data-search-flash] {
  animation: search-flash 1.8s ease-out forwards;
}
```

动画结束后：

- 移除 `<mark>` 或将其样式恢复为透明；
- 不残留黄色背景。

## 错误处理

- 会话已删除：清除 `SearchFocus`，不阻止会话打开。
- 索引状态落后或匹配位置不可用：只滚动到对应消息，降级为不高亮。
- 找不到对应 DOM 文本：不清除会话状态，只跳过闪烁。
- 多条匹配中存在已被流式内容替换的位置：重新按当前 DOM 扫描一次；失败则跳到下一条。

## 兼容性

- 没有 `block_offsets` 的旧文档仍可被搜索，只是不能精确跳转。
- 关闭正文搜索时不会产生正文匹配。
- 不改变文件搜索、空查询、文件夹和 Agent 过滤行为。
- Web 客户端和桌面客户端共用同一套数据结构和前端状态。

## 测试计划

### Rust

- `block_offsets` JSON 序列化/反序列化。
- Unicode 字符区间、`leading_trim` 映射。
- 标题命中和正文命中排序。
- 同一 turn 多个 block、同一 block 多次命中。
- 旧文档缺少 offset 时的降级。

### 前端

- 选择搜索结果后写入 `SearchFocus`。
- 等待 detail 后调用滚动接口。
- “下一条”按钮循环和计数。
- 正文 `mark` 动画类添加和移除。
- 标题命中的 `mark` 渲染。
- 找不到目标时的降级行为。

## 不做什么

- 不做跨会话的全局高亮。
- 不做关键词在多个消息中的“全部标黄”。
- 不做持久化搜索焦点。
- 不改变搜索排序和 50 条上限。
