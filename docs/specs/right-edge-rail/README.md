# 右侧图标栏 + 右栏分区改造 — 总览

参考 openchamber 的右侧竖排图标栏形态，对 codeg 工作区做两项改造：

> 参考效果图（openchamber 右栏 + 最右图标栏）：[openchamber-reference.png](./openchamber-reference.png)
> — 右栏自上而下为：会话（上下文 %）、项目（agents）、轮次统计、**子代理**（可折叠、
> 右侧计数、每行"名称 + 状态"）、**任务**（"已完成/总数"进度 + 每行任务）；
> 最右缘为全高竖排图标栏。

1. **最右侧竖排图标栏**：取代现在钉在窗口右上角的横排按钮（终端 / 右栏开关 / 设置），
   变成贴窗口右缘、全高、占位的独立竖条；右栏各 tab 直达。
2. **右栏"会话详情"tab 内新增分区**：把消息区左上角浮动的"智能体计划"任务列表与
   子代理列表，以分节形式并入右栏会话详情（浮层本身保留）。

## 分期

| 期 | 内容 | spec |
|---|---|---|
| s1 | 最右侧竖排图标栏（新组件 + 挂载 + 退役旧右上角簇） | [s1-right-edge-rail.md](./s1-right-edge-rail.md) |
| s2 | 清理右缘避让机制（window-chrome 常量、aux tab 条塌陷逻辑） | [s2-reserve-cleanup.md](./s2-reserve-cleanup.md) |
| s3 | 会话详情 tab 新增"任务"与"子代理"分区 | [s3-session-details-sections.md](./s3-session-details-sections.md) |
| s4 | i18n 十语言文案 + 测试与验收 | [s4-i18n-acceptance.md](./s4-i18n-acceptance.md) |

## 已确认的决策（用户拍板）

- 任务列表以**会话详情 tab 内分区**呈现（不另开 tab、不合并成新 tab）。
- 消息区左上角的浮动计划卡 / 子代理浮层**保留**，与右栏分区数据同源。
- 图标栏内容：**右栏各 tab + 终端 + 设置**；左栏切换仍留在左上角 `LeftEdgeChrome`。
- 图标栏实现：**占位的独立竖条**（布局中真实的右缘列，非 fixed 悬浮），
  Windows/Linux 原生标题按钮保持 fixed 于右上角，图标栏顶部预留 `h-10` 拖拽区让位。

## 关键现状（探索结论）

- 右上角横排按钮：`src/components/layout/right-edge-chrome.tsx`，
  由 `src/app/workspace/layout.tsx:1217-1231` 以 fixed overlay 钉住。
- 避让体系：`src/lib/window-chrome.ts` 的 `RIGHT_CHROME_CLUSTER=116` /
  `rightChromeReserve` / `rightChromeClusterWidth`，消费方：
  `workspace/layout.tsx:328`（中列 reserve）、`conversation-detail-panel.tsx:2331`、
  `aux-panel.tsx:192`（tab 条塌陷 `shouldCollapseAuxTabs`）。
- 右栏：`src/components/layout/aux-panel.tsx`，4 tab；状态在
  `src/contexts/aux-panel-context.tsx`（`AuxPanelTab`、localStorage
  `workspace:right-sidebar`）。
- 浮动任务卡：`src/components/chat/agent-plan-overlay.tsx`；子代理浮层：
  `src/components/chat/sub-agent-overlay.tsx`；共栈容器在
  `src/components/message/message-list-view.tsx:1612-1644`。
- 子代理数据：`extractDelegationSources`（`message-list-view.tsx:409-467`）、
  `useDelegationCardModel`、`src/contexts/delegation-context.tsx`。
- 会话详情 tab：`src/components/layout/aux-panel-session-details-tab.tsx` →
  `SessionDetailsContent`（`src/components/conversations/session-details-content.tsx`）。

## 不动的东西

- 后端 / Rust 层零改动；数据全部来自既有 store/context。
- 移动端（`MobileFolderWorkspaceShell`、`folder-title-bar.tsx`、抽屉）不动。
- 快捷键语义（`toggle_aux_panel`、`toggle_terminal`、`open_settings`）不变。
- 左栏 `LeftEdgeChrome`、`WindowControls`、`StatusBar` 位置不变。
