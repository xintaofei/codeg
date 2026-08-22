# PK Arena 已知问题清单

> 整理于 2026-08-19，基于 `feat/agent-pk-arena` 分支当前代码确认。
> 每条都经过源码验证，标注了具体文件和行号。
> 问题 0 在实跑 round 7（server 模式，5 选手）时发现。

---

## 问题 0：server 模式下选手完成后状态卡在就绪，裁判不触发（P0 根因）

**严重程度**：极高（阻断核心流程）

**实跑证据**（round 7，2026-08-19，server 模式 / 浏览器）：
- 5 个选手（claude_code, codex, open_code, deepseek, qoder）全部完成 git commit
- 后端日志全部有 `turn_complete stop_reason=end_turn`
- 但 DB pk_round status 仍然是 `running`，不是 `finished`
- 日志中 `judge` 出现 0 次，没有裁判 conversation 被创建
- 选手状态全部停在 `ready`，UI 显示 0/5

**根因分析**：

选手状态流转链：`connecting → ready → (status_changed "prompting") → running → (turn_complete) → done`

- `use-pk-round.ts:922-924`：turn_complete 到达时检查 `contestant.status === "running"`
- `use-pk-round.ts:947-950`：选手从 ready → running 依赖收到 `status_changed` 事件的 `status === "prompting"`
- 如果 `status_changed(prompting)` 事件未到达前端，选手永远停在 `ready`
- 后续 `turn_complete` 到达时 `contestant.status === "running"` 为 false，直接忽略（return）
- round 永远停在 running，settleContestant 不被调用，裁判不触发

**事件投递链路**（server 模式）：
- `acp-connections-context.tsx:4388-4398`：web/server 模式跳过全局 `acp://event` 监听
- 事件只通过 per-connection attach stream 投递（`setupAttachSubscription`）
- PK 在 `use-pk-round.ts:1041` 调 `attachDelegationChild` → `:5912` 调 `setupAttachSubscription`
- attach stream onEvent → `applyMappedEnvelope`（:4177）→ reducer dispatch → fan out 到 useAcpEvent subscribers（:4184-4186）
- 链路代码看起来完整，但实际运行时 status_changed(prompting) 事件未到达 PK handler

**待查**：是否 attach subscription 建立时机与 status_changed(prompting) 发出时机存在竞态，导致事件在 attach 完成前发出且未被 snapshot/replay 捕获。

**修复方向**：
1. 短期：startPrompt 发 prompt 后主动把选手设为 running（不依赖 status_changed 事件）
2. 长期：排查 server 模式 attach stream 是否丢失早期事件

---

## 问题 1：取消回合不触发裁判

**严重程度**：中（影响核心使用场景）

**场景**：用户启动 PK，某个 agent 耗时过长，用户想提前结束并导出报告时触发裁判打分。

**现状**：做不到。

- `cancelRound`（`use-pk-round.ts:1102-1131`）只做：markRound("canceled") + 断开未完成选手连接 + 标选手为 canceled
- 裁判的唯一触发点在 `settleContestant`（`:795-814`），条件是所有选手 settled（done/error/canceled）后 markRound("finished") + 检查 judgeAgent
- cancelRound 直接 markRound("canceled")，不走 settleContestant 分支，裁判永远不会被触发

**修复方向**：cancelRound 末尾加：如果配了 judgeAgent 且 judgeStatus === "idle"，调用 runJudge（复用 settleContestant 里的逻辑）。

---

## 问题 2：裁判评分不在导出报告里

**严重程度**：中

**现状**：`buildPkReportHtml`（`pk-report.ts:83-196`）完全没有引用 `round.judgeResult`。报告只包含：任务文本、元信息、计分板表格（选手/状态/用时/token/轮次/diff增删/文件数）、各选手 diff 详情。没有裁判评分板块。

**修复方向**：buildPkReportHtml 加上裁判评分渲染——渲染 `round.judgeResult.scores` 的排名、分数、点评、summary。

---

## 问题 3：裁判评分不在分享截图里

**严重程度**：低

**现状**：`handleShare`（`pk-arena-dialog.tsx:162-180`）只截 `scoreboardRef.current`，即 PkScoreboard 组件。PkJudgePanel 在 scoreboard 的外面（`:330-339`），截图不包含裁判面板。

**修复方向**：把截图范围扩大到包含 PkJudgePanel，或给裁判面板单独加 ref。

---

## 问题 4：裁判评分不持久化

**严重程度**：高

**现状**：`judgeResult` 只存在前端 store（`pk-arena-store.ts`）的内存里。`pk_round` 表没有 judge_result 字段（见 migration `m20260819_000001_pk_round.rs`）。刷新页面或重启 server 后，裁判结果丢失。只有裁判的 conversation transcript 还在 DB 里（kind=Pk 的 conversation）。

**影响**：用户跑完 PK、关掉 arena、再打开，裁判评分就没了。只能从裁判的 conversation 记录里人肉找 JSON。

**修复方向**：pk_round 表加 judge_result JSON 列，store hydrate 时读回。

---

## 问题 5：导出报告不含裁判评分，且取消时无法触发裁判

**严重程度**：高（两个问题叠加）

这是问题 1 和问题 2 的叠加效应。用户想"提前结束 + 导出报告 + 看裁判打分"这个完整流程，当前完全做不到：
- cancel 不触发裁判（问题 1）
- 即使裁判跑过，报告也不含评分（问题 2）
- 截图也不含评分（问题 3）

---

## 问题 6：控制变量 PK 的 UI 未完成

**严重程度**：中

**现状**：数据层已完成——`PkRoundConfig.agents` 支持 `Array<{agent, label}>` 新格式，兼容旧 `string[]` 格式（store hydrate 时归一化）。但 launcher UI（`pk-launcher-dialog.tsx`）不支持在选手选择区重复添加同一 agent。每个 agent 只显示一个按钮，选中/取消是 toggle，无法添加第二次。

**影响**：用户无法在 UI 上做"Claude Code Sonnet vs Claude Code Opus"这种控制变量实验。只能通过 API 直接创建。

**修复方向**：launcher 选手选择改为"添加槽位"模式，每个槽位独立选 agent + label。

---

## 问题 7：真实工程 PK 的任务来源仅做了 commit 拉取

**严重程度**：低

**现状**：launcher 有 "From commit" 按钮拉取最近 commit 作为任务。另有 "From diff" 按钮从工作区未提交改动拉取（调 gitStatus 列出改动文件,选择后调 gitDiff 填入 task）。以下任务来源未做（架构限制/收益低）：
- 从 TODO 注释拉取 ❌（需新增后端扫描命令,收益低）
- 从 GitHub issue 拉取 ❌（依赖 gh 认证+网络,复杂度高）
- 在文件树里点选问题来 PK ❌（交互复杂,偏离 launcher 定位）

---

## 问题 8：裁判无法手动重跑

**严重程度**：低

**现状**：裁判是 one-shot 自动触发，judgeStatus 从 idle→running→done/error，没有重跑按钮。如果裁判 JSON 解析失败（judgeStatus="error"），用户无法手动重新触发裁判。

**修复方向**：arena 里加"重新评分"按钮，重置 judgeStatus 为 idle 后调用 runJudge。

---

## 问题 9：进行中的回合可以无限制新开回合

**严重程度**：低（可能是预期行为）

**现状**：arena 的 "New round" 按钮（`pk-arena-dialog.tsx:258-264`）始终可点击，不检查当前回合是否在运行。点击后打开 launcher，可以创建新回合，旧回合继续在后台跑。

**影响**：用户可以同时跑多个回合，没有互斥。可能导致资源占用过高（每个回合的选手都开独立 worktree + agent 连接）。

**评估**：这可能是有意设计（多回合并行），不是 bug。但如果需要限制，应在 launcher 的 start 校验里加检查。

---

## 问题 10：裁判评分维度固定，不可配置

**严重程度**：低

**现状**：裁判 prompt（`use-pk-round.ts:69-90`）硬编码 4 个评分维度：
1. Correctness — 是否完成任务
2. Code quality — 可读性、结构、边界处理
3. Completeness — 完成了多少
4. Efficiency — 代码层面效率（明确排除 token 数和耗时）

用户无法自定义评分维度或权重。

**修复方向**：launcher 加评分维度配置，传入 buildJudgePrompt。

---

## 问题 11：裁判只看 diff，不看运行结果

**严重程度**：低（设计限制）

**现状**：裁判 prompt 只传 `contestantsWithDiffs`（`use-pk-round.ts:673-693`），即每个选手的 git diff 文本。裁判不跑代码、不看截图、不看运行日志。纯静态 diff 审查。

**影响**：对于"代码能跑但 diff 看起来差"或"代码差但能跑"的情况，裁判评分可能不准。

**评估**：这是当前架构限制，要支持运行结果需要重大改造（沙箱执行 + 截图捕获）。暂时记录，不做。

---

## 问题 12：server 模式下点击文件无法打开/定位到文件夹

**严重程度**：中

**场景**：server 模式（浏览器访问），用户在消息/文件引用里点击文件想打开所在文件夹——桌面模式可以（调系统 Finder），server 模式无反应。

**现状**：

- `revealItemInDir`（`platform.ts:99-104`）和 `openPath`（`platform.ts:87-93`）在 web/server 模式下是 **no-op**——条件 `isDesktop() && getActiveRemoteConnectionId() === null` 为 false 时直接 return
- `file-reference-actions.tsx:128-130`：右键菜单的"在系统文件管理器中打开"选项在 server 模式下通过 `isLocalDesktop()` 守卫隐藏，用户看不到入口
- `reply-artifacts.tsx:111`：AI 生成文件的"打开"按钮调 `revealItemInDir`，server 模式下静默失败
- 后端有 `open_worktree_folder` HTTP 端点（`handlers/folders.rs:69`），但它的作用是把 worktree 注册到侧边栏文件夹列表，不是打开系统文件管理器
- server 模式下浏览器没有权限直接操作本地文件系统，需要后端代理

**修复方向**：

1. 后端加 `reveal_item` / `open_path` HTTP 端点，server 模式下调 `opener` crate 在服务器主机上打开 Finder/Explorer
2. 前端 `revealItemInDir` / `openPath` 在 server 模式下调 HTTP 端点而非 Tauri 插件
3. `file-reference-actions.tsx` 的 `isLocalDesktop()` 守卫改为"桌面本地 OR server 模式"都显示入口
4. 注意：server 部署在远程时，打开的是**服务器主机**的文件管理器，不是客户端的——这个限制需要在 UI 上提示用户

---

## 问题 13：任务完成后 arena 对话框只能最小化无法关闭

**严重程度**：中

**场景**：用户跑完 PK 后想关掉 arena 对话框，但只能最小化，没有关闭按钮。

**现状**：

三层问题叠加：

1. **正常进行中的回合**（`pk-arena-dialog.tsx:64-68`）：
   - `liveRef = round.status === "ready" || round.status === "running"`
   - ESC 被阻止（`:199-201` onEscapeKeyDown preventDefault）
   - 点遮罩被阻止（`:202-204` onPointerDownOutside preventDefault）
   - 没有关闭按钮（`:197` showCloseButton={false}）
   - 只有"最小化"按钮（`:265-275` setArenaOpen(false)，不真正关闭/清理）
   - 注释（`:64-65`）说"只有 X 按钮能显式关闭"，但 X 按钮被 showCloseButton={false} 去掉了——设计意图与实现矛盾

2. **因问题 #0 导致的卡住**（最常见场景）：
   - 选手完成后 round.status 仍是 running（问题 #0），liveRef 永远 true
   - 所有关闭路径被永久阻止
   - 只能最小化

3. **即使正常完成的回合**（status=finished/canceled）：
   - liveRef 为 false，ESC 和点遮罩能关
   - 但仍然没有 X 按钮（showCloseButton={false}），用户不知道怎么关
   - handleOpenChange（`:184-191`）能处理关闭，但入口不明显

**修复方向**：
1. 始终显示关闭按钮（showCloseButton=true），让用户有明确关闭入口
2. 进行中的回合点关闭时弹确认（"回合进行中，确定关闭？"），而非永久阻止
3. 或保持 ESC/遮罩阻止，但给一个明确的关闭按钮 + 确认对话框

---

## 问题 14：PK 会话管理 UI 粗糙——下拉框切换 + 标题无信息 + 侧边栏不可见

**严重程度**：中（体验差，但不阻断功能）

**三个子问题：**

### 14a：回合切换只有一个下拉框

**现状**（`pk-arena-dialog.tsx:225-239`）：

```tsx
<select value={round.id} onChange={...} className="max-w-40 ...">
  {rounds.map((r) => (
    <option key={r.id} value={r.id}>
      {new Date(r.createdAt).toLocaleTimeString()} · {r.contestants.length} 选手
    </option>
  ))}
</select>
```

- 只显示"时间 + 选手数"，没有任务文本、状态、分数
- max-w-40 导致长内容被截断
- 多回合时无法快速区分"哪轮做了什么"
- 没有搜索/过滤
- 没有删除单轮的入口（只有删除当前轮）

### 14b：conversation 标题被 agent 自动覆盖，丢失 PK 上下文

**现状**：

- `create_pk`（`conversation_service.rs:99-100`）创建时 `title = "PK · <taskPreview>"`，`title_locked = false`
- agent 跑完后，`refresh_auto_title`（`:240-262`）因为 `title_locked = false`，用 agent 自己取的标题覆盖了 PK 标题
- 实跑证据（round 7）：DB 里 5 个选手的 title 分别是 "Interactive jelly blob browser toy"、"Build a tiny browser toy..." 等各不相同的标题，没有一个带 "PK ·" 前缀
- 裁判会话的标题 `PK Judge · <task>` 也同样会被覆盖

**根因**：PK conversation 应该 `title_locked = true`，防止 agent auto-title 覆盖。

### 14c：PK 会话在侧边栏完全不可见

**现状**：

- `sidebar-conversation-list.tsx:1059`：`c.kind !== "pk"` 过滤掉所有 PK 会话
- `sidebar-conversation-grouping.ts:300`：`if (conv.kind === "pk") continue` 分组时也跳过
- types.ts:421 注释说 `kind === "pk"` "drives the sidebar's per-round grouping"，但实际分组逻辑根本没有实现——只有排除，没有 PK 专用分组渲染
- 用户无法在侧边栏看到/打开 PK 选手的会话记录，只能通过 arena 对话框的 battle tab 看实时流

**修复方向**：

1. **14b（最简单）**：`create_pk` 里设 `title_locked = true`
2. **14a**：回合切换改为列表/卡片视图，每轮显示：任务摘要、状态徽章、选手头像+分数、创建时间；支持搜索和删除
3. **14c**：侧边栏加 PK 分组——按 round 分组，每组显示任务摘要 + 选手会话列表，点击打开选手 transcript

---

## 问题 15：battle/diff 列固定 w-80，不按 agent 数量自适应分配空间

**严重程度**：中

**场景**：3 个选手时右侧大片留白，5 个选手时需要横向滚动。

**现状**：

- `PkBattlePane`（`pk-arena-dialog.tsx:425`）：`w-80 shrink-0` — 固定 320px，不可收缩
- `PkReadyPane`（`:464`）：同 `w-80 shrink-0`
- `PkDiffView`（`pk-diff-view.tsx:79`）：`flex h-full min-h-0 flex-col` — 没有 shrink-0，但父容器 `flex h-full gap-2 overflow-x-auto`（`:360`）不限制子元素宽度，diff 列实际也不自适应
- 容器（`:360`）：`flex h-full gap-2 overflow-x-auto p-2` — 水平滚动布局，子元素不会被压缩到容器宽度内
- 没有根据 `round.contestants.length` 计算 `flex-basis` 或 grid 列数的逻辑

**影响**：

- 3 个 agent：3 × 320px = 960px，对话框通常 1400px+，右侧约 440px 留白
- 5 个 agent：5 × 320px = 1600px，需要横向滚动
- 8 个 agent（最大支持）：8 × 320px = 2560px，大量滚动

**修复方向**：

- 容器改 `grid` 布局，列数 = `min(contestants.length, 上限如6)`，`grid-template-columns: repeat(N, minmax(0, 1fr))`
- 或改 `flex-1 min-w-0`，让每列等分剩余空间
- 列数超过上限时再回退到固定宽度 + 横向滚动
- 需要同时改 PkBattlePane / PkReadyPane / PkDiffView 三个组件的根 div

---

## 问题 16：导出报告里输出 token 和轮次全是 "—"

**严重程度**：中

**现状**：`pk-report.ts:105-106` 里 `c.usage ? c.usage.outputTokens : "—"`——usage 为 null 时显示 "—"。实际跑完的 PK 报告里这两列全是 "—"。

**根因有三层：**

1. **问题 #0 场景**（最常见）：选手卡在 ready，settleContestant（`use-pk-round.ts:779-790`）从不执行，fetchUsage 从不被调用，usage 永远 null。

2. **重启/hydrate 场景**：即使选手曾经正常 done 且 fetchUsage 调过，重启后 hydrate 把 usage 重置为 null：
   - `dbRoundToStoreRound`（`pk-arena-store.ts:246`）：硬编码 `usage: null`
   - `createRound`（`:315`）：同 `usage: null`
   - `pk-arena-host.tsx:40-48`：hydrate 从 DB 加载 rounds 后直接 `hydrateFromDb(storeRounds)`，**没有调 fetchUsage 重新拉取**
   - store 注释（`:15`）说 "Live-only fields (connectionId, diff, usage) stay in the Zustand store — they are meaningless across restarts"——这个设计假设导致 usage 在重启后永远丢失

3. **即使 fetchUsage 被调用**（`use-pk-round.ts:471-489`）：
   - 调 `getFolderConversation` 拿 turns，遍历 assistant turns 累加 `turn.usage?.output_tokens`
   - 如果 parser 没从 agent session 文件提取到 usage（某些 agent 格式不包含 token 信息），turn.usage 为 None
   - 此时返回 `{ inputTokens: 0, outputTokens: 0, turnCount: N }`——不是 "—" 而是 0
   - DB 里 `token_usage_turn` 表 0 行，说明 token usage 同步机制也有问题

**实跑证据**（round 7）：
- `token_usage_turn` 表 0 行
- 5 个选手 status 全是 ready（问题 #0），settleContestant 从不执行
- 报告里 token 和轮次全 "—"

**修复方向**：
1. hydrate 时对已完成的选手调 fetchUsage 回填（usage 不应该被当作 live-only）
2. 或把 usage 持久化到 DB（pk_round 或 conversation 表）
3. 修复问题 #0 让选手正常走到 settleContestant

---

## 问题 17：模板任务提示词应为中文，且缺少黑洞效果模板

**严重程度**：低（增强需求）

**现状**（`pk-templates.ts:23-60`）：

6 个内置模板的 task 文本全是英文：
- "Generate an SVG of a pelican riding a bicycle."
- "Create an HTML animation: a ball starts in the center of a triangle..."
- "Build a tiny browser toy: a jelly blob..."
- "Write a Snake game in a single HTML file..."
- "Write a Flappy Bird clone..."
- "Create a voice-enabled chatbot web app..."

i18n 只翻译了模板**标签**（zh-CN.json:5013-5018 "鹈鹕骑车"/"弹球"等），但 `setTask(tpl.task)` 填进 textarea 的是英文原文，用户提交前看到的是英文 prompt，agent 收到的也是英文。

**需求**：

1. task 文本改为中文（面向中文用户为主）
2. 新增黑洞效果模板——视觉效果好的引力透镜/吸积盘动画，适合 PK 股评对比

**修复方向**：

- `pk-templates.ts` 的 task 字段改为中文
- 新增 `{ id: "blackHole", emoji: "🕳️", task: "..." }` 黑洞模板
- i18n 所有语言的 `templates.blackHole` 标签

---

## 问题 18："从提交拉取"混在模板按钮里，交互不清晰

**严重程度**：中（体验差）

**现状**（`pk-launcher-dialog.tsx:290-367`）：

"从提交拉取"按钮和 6 个模板按钮并排排列（:304-333），视觉上像一个普通模板，但行为完全不同：

- 模板按钮：一键填充 textarea，即时生效
- 从提交拉取：点击后异步加载 git log，展开一个 commit 列表，再点一条 commit 才填充
- 生成的 prompt 是硬编码英文模板（:352）：`Reproduce the change from commit ${hash}: ${message}`

**问题**：

1. **分类混淆**：模板（预设创意任务）和 from-commit（真实工程复现任务）是两种完全不同的 PK 模式，混在一个按钮行里，用户不知道点了会发生什么
2. **prompt 硬编码英文**：和问题 17 一样，生成的 commit 复现 prompt 也是英文
3. **交互不可发现**：按钮点了会展开列表，但没有视觉提示这是个"可展开"操作，第一次用的用户会困惑
4. **commit 只显示 hash + message**：没有日期、作者、改动文件数，用户无法判断该选哪条
5. **只拉 5 条**：`gitLog(workingDir, 5)` 硬编码 5 条，不够时没法翻页

**修复方向**：

把 launcher 的任务来源分成两个清晰的模式区：

```
┌─ 创意 PK（模板）──────────────────────────┐
│ 🦤 🦤 ⚽ 弹球  🫧 果冻  🐍 贪吃蛇 ...      │
│ [一键填充任务文本]                          │
└────────────────────────────────────────────┘

┌─ 真实工程 PK（从提交）──────────────────────┐
│ [📋 从提交拉取]  → 展开为 commit 选择面板     │
│  • 展开后显示: hash · 日期 · 作者 · 改动文件数 │
│  • prompt 改为中文: "复现提交 {hash} 的改动:   │
│    {message}"                                │
│  • 支持加载更多 / 翻页                        │
└──────────────────────────────────────────────┘
```

具体改动：
1. 模板按钮和 from-commit 在视觉上分区（不同容器/分隔线/标题）
2. from-commit 展开后的列表增加日期、作者、改动文件数（gitLog 支持 `withFiles`）
3. 复现 prompt 改为中文
4. 支持加载更多 commit（limit 改为可配置，或加"加载更多"按钮）

---

## 问题 19：战报中的嵌入式 HTML 可能无法完整运行

**严重程度**：高（影响战报预览与快速分享）

**最新一轮实跑证据（round 5）**：

- 6 份嵌入产物与各自 worktree 原文件逐字节一致，不是导出时截断或编码损坏
- 其中 4 份页面使用了 `localStorage`
- 战报 iframe 当前使用 `sandbox="allow-scripts allow-pointer-lock"`，页面处于不透明来源
- 页面访问 Web Storage 时会触发安全异常，导致后续初始化脚本中断；Qoder 页面表现为画布空白、开始层未显示
- 仅作诊断时加入 `allow-same-origin` 后页面可以启动，证明根因是沙箱来源限制

**安全约束**：不能直接把 `allow-same-origin` 与 `allow-scripts` 长期组合使用，否则会明显削弱用户生成代码的隔离边界。

**修复方向**：

1. 保留 iframe 沙箱隔离
2. 导出时注入按选手隔离的内存版 `localStorage` / `sessionStorage` 兼容层
3. 导出前扫描常见运行依赖并给出兼容性提示
4. 增加真实浏览器回归测试，至少覆盖 Web Storage、键盘输入、Canvas 和重新开始流程
5. 对外链依赖和多文件项目另行做资源打包，不能只靠单文件 `srcdoc`

---

## 问题 20：顶部固定区域的 PK 按钮层级过高且入口重复

**严重程度**：中（信息架构与视觉层级问题）

**现状**：`LeftEdgeChrome` 将 PK 双剑按钮与侧栏开关、远程工作区等壳层操作并列。它既可能新建竞技场，也可能打开已有竞技场，语义不稳定，视觉上还容易被理解为全局或窗口级操作。

**已有入口**：

- 输入框附加菜单：新建 / 打开竞技场
- 左侧 PK 竞技场列表：打开具体历史回合
- 缩小胶囊：恢复当前进行中的竞技场

**与 `main` 分支风格的冲突**：`main` 的顶部固定区域只保留结构性、窗口级控制；代码中还明确移除了可从其他位置触达的重复入口。PK 属于业务功能，不应占据这一层级。

**决策方向**：

1. 删除 `LeftEdgeChrome` 中的 PK 双剑按钮
2. 保留输入框入口负责创建，侧栏负责历史，缩小胶囊负责恢复
3. 如果新建入口仍不够明显，可在“PK 竞技场”分组标题悬停时显示轻量 `+`，不要重新放回全局顶栏

---

## 优先级排序

> ✅ 已修复: #0 #1 #2 #3 #4 #6 #7 #8 #10 #12 #13 #14a #14b #15 #16 #17 #18 #19 #20

| 优先级 | 问题 | 状态 | 说明 |
|--------|------|------|------|
| P0 | #0 server 模式选手状态卡 ready，裁判不触发 | ✅ | startPrompt 主动设 running |
| P0 | #4 裁判评分不持久化 | ✅ | pk_round 加 judge_result 列 |
| P0 | #1 取消不触发裁判 | ✅ | cancelRound 末尾调 runJudge |
| P1 | #2 报告不含裁判评分 | ✅ | 报告加颁奖台板块 |
| P1 | #3 截图不含裁判评分 | ✅ | 截图范围含 PkJudgePanel |
| P1 | #12 server 模式无法打开文件夹 | ✅ | 设计正确,加守卫隐藏 |
| P1 | #13 arena 对话框无法关闭 | ✅ | showCloseButton=true |
| P1 | #14b PK 标题被 agent 覆盖 | ✅ | title_locked=true |
| P1 | #15 battle/diff 列不自适应宽度 | ✅ | flex-1 min-w-80 |
| P1 | #16 报告里 token/轮次全 "—" | ✅ | hydrate 回填 usage |
| P1 | #19 战报嵌入式 HTML 无法完整运行 | ✅ | 保持沙箱隔离，注入内存 Web Storage 兼容层并补回归测试 |
| P2 | #14a 回合切换下拉框信息不足 | ✅ | option 加状态+task+冠军 |
| P2 | #14c PK 会话侧边栏不可见 | | 待修 |
| P2 | #6 控制变量 UI 未完成 | ✅ | 选手选择改槽位模式 |
| P2 | #8 裁判无法重跑 | ✅ | 重评按钮 |
| P2 | #20 顶部 PK 按钮层级过高且重复 | ✅ | 删除顶栏入口，保留创建、历史、恢复三条职责清晰的路径 |
| P3 | #10 评分维度不可配 | ✅ | launcher 加维度配置 |
| P3 | #7 任务来源不足 | ✅ | commit + 工作区改动拉取 |
| P3 | #17 模板提示词改中文 + 黑洞模板 | ✅ | 中文 + 🕳️ |
| P3 | #18 从提交拉取和模板分区 | ✅ | 分区 + commit 增强 |
| P4 | #9 无限新开回合 | | 可能预期行为 |
| P4 | #11 裁判只看 diff | | 架构限制 |
