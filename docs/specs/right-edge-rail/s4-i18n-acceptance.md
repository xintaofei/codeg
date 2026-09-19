# s4 — i18n 十语言文案 + 最终验收

## 新增文案

命名空间 `Folder.sessionDetails`，新增 2 个 key（10 个语言文件同步）：

| key | en | zh-CN |
|---|---|---|
| `tasksHeading` | Tasks | 任务 |
| `subAgentsHeading` | Sub-agents | 子代理 |

译文约定（对齐各语言既有同概念文案，不自造词）：
- zh-TW：任務 / 子代理；ja：タスク / サブエージェント；
  ko：작업 / 하위 에이전트；参照各文件既有关键词。
- es/fr/pt/ar/de：`es.json` "Tareas"/"Subagentes"、`fr.json` "Tâches"/
  "Sous-agents"、`pt.json` "Tarefas"/"Subagentes"、`de.json` "Aufgaben"/
  "Unter-Agenten"、ar 参考既有 `Folder.chat.subAgentOverlay.title` 译法。

图标栏 tooltip 全部复用现有 key（`Folder.folderTitleBar.*`、
`Folder.auxPanel.tabs.*`、`Folder.sessionDetails.menuLabel`），不新增。
`Folder.chat.agentPlanOverlay.*` / `Folder.chat.delegation.*` 复用，不新增。

各语言文件的插入位置：对应 `Folder.sessionDetails` 对象内、按 en.json 的键序
就近插入（tokensHeading/timestampsHeading 之后）。

## 测试与验收命令

```bash
pnpm eslint .
pnpm test            # vitest 全量
pnpm build           # 静态导出
```

## 总体验收清单

视觉（桌面 Windows，桌面实跑截图对照 openchamber 参考图）：

- [ ] 窗口右缘出现全高竖排图标栏：4 个右栏 tab 图标 + 分隔线 + 终端 + 设置；
      顶部 h-10 与原生标题按钮共存，可拖窗。
- [ ] 原右上角横排按钮簇消失；左缘切换按钮不变。
- [ ] 打开右栏 → 图标栏上当前 tab 高亮；点高亮项收起右栏。
- [ ] 右栏会话详情 tab 内：标识/token/时间戳之后出现"子代理"与"任务"分区
      （有数据才显示），条目与消息区浮层一致，点击子代理行打开子会话。
- [ ] 消息区左上角浮动计划卡/子代理浮层仍在，功能不变。
- [ ] chat 会话、无文件夹、整页路由（任务看板/自动化）、zoom 90/150、
      右栏 200px 最窄 各态无布局破损。
- [ ] 移动端窗口（<768px）与 server/web 模式正常（rail 仅桌面渲染）。

代码：

- [ ] s1/s2/s3 各自验收项全过。
- [ ] `rg rightChromeReserve|RIGHT_CHROME_CLUSTER|rightChromeClusterWidth` 零命中。
- [ ] `rg right-edge-chrome` 零命中（组件删除后无残留 import）。
- [ ] eslint / vitest / build 三件套全绿；新增测试覆盖：rail 渲染与点击、
      shouldCollapseAuxTabs 新签名、分区数据合并、extractDelegationSources
      迁移后回归。
