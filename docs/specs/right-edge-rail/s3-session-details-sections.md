# s3 — 会话详情 tab 新增"任务"与"子代理"分区

## 目标

右栏"会话详情"tab（`aux-panel-session-details-tab.tsx`）在现有
`SessionDetailsContent` 之下新增两个分区，样式对齐 openchamber：
小节标题行（图标 + 名称 + 右侧计数），其下逐行列出条目。

- **任务**：当前会话最新的智能体计划（TodoWrite / plan block）。
- **子代理**：当前会话委派出去的子代理列表。

消息区左上角的浮动 `AgentPlanOverlay` / `SubAgentOverlay` 保留不动。

## 共享件抽取

1. `src/lib/delegation-sources.ts`（新）
   - 把 `message-list-view.tsx:409-467` 的 `collectDelegationSources` /
     `extractDelegationSources` 原样迁入（依赖的 `isDelegateToAgentToolName`、
     `isRefusedResume`、`parseResumeTaskId`、`normalizeToolName` 留在原模块或一并迁入）。
   - `message-list-view.tsx` 改 import，行为零变化（有既有测试兜底）。
2. 计划行共享组件
   - `agent-plan-overlay.tsx` 抽出 `StatusIcon` 与单条目行
     （状态图标 + 文案 + 状态/优先级 Badge）为 `PlanEntryRow`，
     放 `src/components/chat/plan-entry-row.tsx`；浮层与右栏分区共用。
   - 浮层现有视觉不得变化（回归看现有测试 + 快照）。

## 右栏数据接入（`aux-panel-session-details-tab.tsx`）

当前活动会话解析逻辑（tab → runtimeConversationId → `byConversationId`）已在此文件，
新分区挂在同一组件里：

- **任务分区**：
  - 实时：`liveMessage`（runtime store 已有）→ `getLatestPlanEntries`
    （从 `agent-plan-overlay.tsx` 一并导出或迁入共享模块）。
  - 历史回退：live 无 plan 时，用 `src/lib/agent-plan.ts` 从已加载 turns
    （`localTurns`）解析最近一次 plan-like 工具调用（与 `message-list-view`
    传给浮层的 `entries/planKey` 同一函数）。
  - 计数：`completed/total`。无计划时分区整体隐藏（与浮层一致：没数据不占位）。
- **子代理分区**：
  - 数据：`DelegationContext`（`delegation-context.tsx` 的绑定 map）中
    `parentConversationId === 当前会话` 的绑定，∪ 最近 assistant turn 的
    `extractDelegationSources`（turns 来自 runtime store 的会话 detail/turns，
    与浮层同源；两者按 `parentToolUseId`/taskId 去重合并）。
  - 行渲染：复用 `SubAgentOverlay` 的行视觉——把其 `SubAgentOverlayRow` 抽成
    导出组件 `DelegationRow`（`src/components/chat/delegation-row.tsx`），
    两处（浮层 + 分区）共用；点击经 `useSessionViewerHost` 打开子会话
    （aux-panel 不在 `MessageListView` 内，host 为 `null` 时走
    `SubAgentSessionDialog` 本地兜底——该组件已支持此态）。
  - 计数：条目数。列表为空时分区隐藏。
- 空态：两个分区都空时，只显示原会话详情内容，不出现空小节标题。

## 视觉规格

- 分区标题行：`text-xs font-medium uppercase tracking-wide text-muted-foreground`
  （与 `SessionDetailsContent` 的 `tokensHeading` 小节一致）+ 左侧小图标
  （任务 `ListTodoIcon`、子代理 `BotIcon`）+ 右侧计数（`text-xs text-muted-foreground`）。
- 分区间以 `border-t pt-4` 分隔；任务条目复用 `PlanEntryRow`；
  子代理条目复用 `DelegationRow`（去掉外层卡片的 `border`，改列表行）。
- 全部置于既有 `ScrollArea` 内（跟随 `SessionDetailsContent` 一个滚动容器，
  不另开滚动区）。

## i18n key（文案实体在 s4）

- `Folder.sessionDetails.tasksHeading`（"任务"）、
  `Folder.sessionDetails.subAgentsHeading`（"子代理"）。
- 行内状态/优先级/状态 Badge 复用 `Folder.chat.agentPlanOverlay.*` 与
  `Folder.chat.delegation.*` 现有 key。

## 验收

- [ ] 流式会话中智能体创建/更新计划 → 右栏任务分区实时跟随更新；
      浮层同步更新，两处计数一致。
- [ ] 打开历史会话（含 TodoWrite 计划）→ 无 live 时右栏显示回退解析的计划。
- [ ] 子代理运行中显示运行态图标，完成/失败状态与消息流内委派卡一致；
      点击行打开子会话查看器。
- [ ] 切换 tab / 切换会话时分区随活动会话切换（复用 tab store 的 activeTabId）。
- [ ] 右栏收起时中列浮层照常工作（回归）；浮层展开/折叠互不影响。
- [ ] 新单测：live→历史回退优先级、去重合并、空态隐藏、点击打开 viewer；
      `pnpm eslint .`、`pnpm test` 全绿。
