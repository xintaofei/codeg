# s2 — 清理右缘避让机制

## 背景

旧设计里右上角 chrome 是 fixed overlay，浮在"当前右缘列"（AuxPanel 或中列）之上，
所以各处要 `rightChromeReserve` 预留宽度。s1 把 chrome 变成**占位竖条**后，
overlay 不复存在，这套避让全部作废。本期待清干净，避免留下"幽灵留白"。

## 清理清单

`src/lib/window-chrome.ts`：

- 删除 `RIGHT_CHROME_CLUSTER`、`rightChromeReserve`、`rightChromeClusterWidth`。
- 新增 `export const RIGHT_EDGE_RAIL_WIDTH = 40`（rail 占位宽，供
  `window-controls` 对齐、测试与文档引用；rem 缩放不适用——rail 图标固定 px 尺寸
  或用常量派生，实现时若需随缩放增长，参照 `scaleCluster` 加
  `rightEdgeRailWidth(zoom)`）。
- `WINDOW_CAPTION_WIDTH`、macOS/左侧常量不动。
- `src/lib/window-chrome.test.ts`：删除 right 侧三组断言，补 rail 常量断言。

`src/components/layout/aux-panel.tsx`：

- `shouldCollapseAuxTabs` 删除 `rightReserve` 形参（仅剩 panel 宽 vs 分段控件宽），
  或整体保留函数但传 0——**选择删除形参**，同步 `aux-panel.test.tsx`：
  合并 `WIN_LINUX_RESERVE` 用例（不再有 win/linux 差异），保留
  "宽面板不塌陷 / 窄面板塌陷 / 未测量不塌陷"。
- 删除 `rightChromeReserve` import 与 `rightReserve` 计算；tab 条右缘拖拽区不变。
- 顶部注释（66-77 行说明文字）同步改写：塌陷只由面板自身宽度决定。

`src/app/workspace/layout.tsx`：

- `WorkspaceContent`：删除 `rightReserve`、`convReservesRight`、`fileReservesRight`
  及把 reserve 传给列头的逻辑（列头不再为右缘留白）。
  `leftReserve` 保留（左 overlay 仍在）。
- `FolderLayoutShell`：确认已无 `RightEdgeChrome`（s1 完成），
  `WINDOW_CAPTION_WIDTH` 仅 `WindowControls` 用。

`src/components/conversations/conversation-detail-panel.tsx:2331`：

- 该处 `rightChromeReserve` 用于会话面板头部避让右 overlay——
  overlay 没了，删除该 reserve；`leftChromeReserve` 保留。

`src/components/tasks/tasks-chrome-actions.tsx:23`：

- 注释提到 `RIGHT_CHROME_CLUSTER`，改为指向 rail（s1 的
  `WorkbenchRouteChromeActions` 新落点）。

`src/contexts/aux-panel-context.tsx`：

- `resolveAuxMinWidth`：Windows/Linux 的 260 下限起因是"overlay + caption 不溢出到
  中列"；rail 占位后右栏不再被 overlay 压，但 caption 按钮仍 fixed 在右上且
  Win/Linux 下宽 138px > rail 40px——caption 只压 rail 顶部预留区（40px 内），
  **不**再需要为 caption 撑宽右栏。minWidth 统一回 `MIN_WIDTH=200`；
  删除 `resolveAuxMinWidth`/`detectPlatform` 相关分支（若平台 hook 因此无他用以
  一并清理 import，注意 `noUnusedLocals`）。

## 验收

- [ ] 全库 `rg rightChromeReserve|RIGHT_CHROME_CLUSTER|rightChromeClusterWidth` 零命中。
- [ ] 右栏最窄拖到 200px：tab 条塌陷为下拉 picker（mac/win 行为一致），
      内容不溢出、不被 rail 遮挡。
- [ ] 右栏关闭时中列（会话/文件列）头部不再多出 116/138px 幽灵留白。
- [ ] zoom 90/150 无回归（左 overlay 仍正常避让）。
- [ ] `pnpm eslint .`、`pnpm test` 通过；`window-chrome.test.ts`、`aux-panel.test.tsx`
      更新后全绿。
