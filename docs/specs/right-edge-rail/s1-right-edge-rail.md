# s1 — 最右侧竖排图标栏

## 目标

新建贴窗口右缘、全高、占位的竖排图标栏，取代 `RightEdgeChrome` 的右上角横排按钮。
形态对齐 openchamber：一列图标，点哪个面板开哪个，当前激活项高亮。

## 新组件

`src/components/layout/right-edge-rail.tsx` — `RightEdgeRail()`

- 结构：`<aside>` 竖排 flex 列，宽 `RIGHT_EDGE_RAIL_WIDTH`（见 s2，默认 40px），
  全高，右边框分隔，背景与右栏 strip 一致（`bg-muted` + `ws-*` 背景图规则对齐）。
- 顶部 `h-10 shrink-0` 预留区，`data-tauri-drag-region`：
  Windows/Linux 原生标题按钮（`WindowControls`，仍 fixed 右上角）压在这段上，
  macOS/其他 则该段也可拖窗。
- 图标按钮自上而下（桌面，`isConversations` 路由）：
  1. `session_details` — `ReceiptText`
  2. `file_tree` — `Folder`
  3. `changes` — `FolderPen`
  4. `git_log` — `GitCommit`
  5. 分隔线（`h-px bg-border my-1`）
  6. 终端开关 — `SquareTerminal`
  7. 设置 — `Settings`
- tab 图标点击行为：
  - 面板未开 → `openTab(tab)`；
  - 面板已开且 `activeTab === tab` → `toggle()` 收起右栏（点当前高亮项 = 关闭）；
  - 面板已开但 tab 不同 → `setActiveTab(tab)`。
  - 无文件夹 / chat 模式（`showFolderTabs === false` 条件，即
    `activeFolderId == null || isChatMode`）：只显示 `session_details` 图标，
    `file_tree/changes/git_log` 隐藏（与 `resolveAuxTabView` 同一判定）。
- 终端图标：`toggleTerminal()`，`activeFolder` 为空时禁用；激活态样式同旧
  `bg-accent`。
- 设置图标：`openSettingsWindow()`。
- 激活高亮：当前 `activeTab`（面板开）对应 tab 图标加 `bg-accent text-foreground`；
  其余 `text-foreground/70 hover:bg-foreground/10`。图标 `h-4 w-4`。
- tooltip：复用 `Folder.folderTitleBar.withShortcut` + 各 tab 标签；
  终端/设置沿用 `toggleTerminal` / `openSettings` + 快捷键提示。
  tab tooltip 用 `Folder.auxPanel.tabs.*` 与 `Folder.sessionDetails.menuLabel`。
- 整页路由（`!isConversations`）：隐藏 tab 组与终端图标，渲染
  `WorkbenchRouteChromeActions`（沿用其现有 props）+ 设置图标，居中竖排。

## 挂载改造

`src/app/workspace/layout.tsx`：

- `FolderWorkspaceShell` 的 shell `ResizablePanelGroup` 之后（同一 flex 行内、
  组外）追加 `<RightEdgeRail />`，作为**非缩放占位列**：
  shell 容器 `buildShellLayout` 用 `shellWidth`（shellContainerRef 实测）换算，
  图标栏放在 shell 组外层 flex 兄弟位，避免参与 resizable 计算。
  实现上：`FolderWorkspaceShell` 返回结构外层已是 flex row
  （`ResizablePanelGroup` + 预留常量），把 rail 作为最后一个 flex 子项、
  `shrink-0`，容器宽度即从 `shellWidth` 中扣除（由 flex 自然完成）。
- `FolderLayoutShell`（1217-1231）删除 `RightEdgeChrome` 挂载与
  `right: winLinuxControls ? WINDOW_CAPTION_WIDTH : 0` 定位 div；
  保留 `LeftEdgeChrome`、`WindowControls` 两个 overlay。
- `right-edge-chrome.tsx` 删除；`WorkbenchRouteChromeActions` import 移到 rail。
- 中列（`WorkspaceContent` 的 `convReservesRight/fileReservesRight`）：
  AuxPanel 不再被 overlay 压住，右缘 reserve 改为按"rail 实际占位"扣减——
  rail 是占位列，**中列不再需要右 reserve**；相关 reserve 清理归 s2 统一处理，
  s1 先保证视觉正确（rail 占位后 overlay 删除）。

## 验收

- [ ] 桌面三平台位：Windows/Linux 标题按钮压在图标栏顶部预留区内，不互相遮挡；
      拖该区域可移动窗口。
- [ ] 四个 tab 图标点击可开/切/关右栏，激活高亮随 `activeTab` 变化。
- [ ] chat 会话与无文件夹态只显示会话详情图标。
- [ ] 终端图标开关底部终端面板；设置图标打开设置。
- [ ] 整页路由（任务/自动化）下 tab/终端图标消失、显示路由页控件 + 设置，无错位。
- [ ] 右栏开/关、左栏开/关、缩放（zoomLevel 90/150）下图标栏不重叠、不被裁剪。
- [ ] 快捷键 `toggle_aux_panel` 行为不变。
- [ ] `pnpm eslint .` 通过；新组件有渲染单测（各态图标显隐、点击 → context 调用）。
