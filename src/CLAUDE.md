[根目录](../CLAUDE.md) > **src**

# src — 前端模块（Next.js 静态导出）

## 模块职责

Codeg 的全部用户界面：多智能体会话工作台、设置中心、桌宠窗口、Git 操作弹窗页。通过 Transport 抽象层同时对接三种运行环境（Tauri 桌面 / 浏览器 Web / 远程桌面），业务代码不感知后端形态。

## 入口与启动

- **根布局**：`app/layout.tsx`（挂载 `i18n-provider`、`appearance-provider`、`theme-provider`、`overlay-scrollbars-init`、`clipboard-fallback-init`）
- **主工作台**：`app/page.tsx`
- **开发**：`pnpm dev`（next dev --turbopack，`TAURI_DEV_HOST` 控制 assetPrefix）
- **构建**：`pnpm build`（`next.config.ts` 强制 `output: "export"` 静态导出；生产图片 unoptimized）
- **Tauri 集成**：`pnpm tauri:before-dev` / `pnpm tauri:before-build` 会先跑 sidecar 准备与前端构建

## 页面路由（`app/`）

| 路由 | 职责 |
|------|------|
| `/`、`/workspace` | 主工作台（会话列表 + 对话 + 文件查看器 + 终端） |
| `/settings/*` | 17 个设置子页：agents、appearance、chat-channels、experts、general、logs、mcp、model-providers、office-tools、quick-messages、science、shortcuts、skill-packs、skills、system、version-control、web-service |
| `/pet`、`/pet-panel` | 桌宠悬浮窗与面板（私有 `_components/`、`_hooks/`） |
| `/commit`、`/merge`、`/push`、`/stash` | Git 操作弹窗页 |
| `/import-sessions` | 历史会话导入 |
| `/project-boot` | 项目启动页 |
| `/login` | 服务器模式登录页 |

> 静态导出约束：不支持 `[param]` 动态路由，一律用查询参数（如 `?tab=`、`?id=`）。

## 对外接口（对本模块而言的边界层）

### Transport 抽象（`lib/transport/`）

三种实现按环境自动切换（`detect.ts` 探测）：

- `TauriTransport` — 桌面模式，走 Tauri `invoke()`（动态 require，避免 Web 模式打包 tauri 依赖）
- `WebTransport` — 浏览器模式，`fetch()` + `web-event-stream` / `ws-auth` WebSocket，`web-auth.ts` 处理 token 认证
- `RemoteDesktopTransport` — 远程桌面模式：Tauri 客户端绑定远端 codeg-server，API 调用与文件操作指向远端主机

关键 API（`transport/index.ts`）：`getTransport()`（远程优先）、`getShellTransport()`（本地单例）、`isDesktop()`、`isRemoteDesktopMode()`、`configureRemoteDesktopTransport()`、`getServerBaseUrl()`、`notifyRemoteDesktopUnauthorized()`、`__resetTransportForTests()`（仅 NODE_ENV=test）。

### 主 API 客户端

- `lib/api.ts` — 封装全部后端调用；`lib/tauri.ts` — Tauri API 封装；`lib/types.ts` — Rust `models/` 的 TypeScript 镜像（字段一一对应，改动需双向同步）

## 内部结构

```
src/
├── app/                  # Next.js 路由（见上表）
├── lib/                  # 248 个文件：业务逻辑核心
│   ├── transport/        # 三模式 Transport 抽象
│   ├── adapters/         # AI 响应 → 渲染适配（tool-kind-classifier 等）
│   ├── pet/              # 桌宠动画/sprite/市场资源代理
│   ├── terminal/         # 终端主题、写队列
│   ├── api.ts / types.ts / tauri.ts / utils.ts
│   └── *.ts              # 会话、分支树、委托卡、上下文压缩等领域逻辑
├── components/           # 32 个顶层分区，各分区规模（实测文件数）：
│   ├── ai-elements/      # 39 文件：消息渲染 message、markdown/mermaid/katex 插件、
│   │                     #   file-tree、tool、reasoning、terminal
│   ├── chat/             # 52 文件：聊天输入、权限/提问对话框、会话/模型选择器、计划审批卡
│   ├── settings/         # 88 文件（最大分区）：17 个设置页的实现组件——
│   │                     #   各代理 config-panel（antigravity/codebuddy…）、chat-channel
│   │                     #   四 Tab（channel-list/commands/events/other）、model-provider、
│   │                     #   backup、agent-diagnostics、forge/git 账号对话框等
│   ├── message/          # 83 文件：消息气泡/树/工具调用展示
│   ├── layout/           # 62 文件：工作台骨架、侧栏、标题栏、面板布局
│   ├── tasks/            # 38 文件：work_task 看板（列、卡片、编辑器、筛选）
│   ├── conversations/    # 29 文件：会话列表、分组、移动/归档操作
│   ├── automations/      # 自动化编辑器、cron 构建器、模板库
│   ├── forge/            # 14 文件：GitHub/GitLab 集成 UI
│   ├── files/            # 11 文件：文件树与文件查看器
│   ├── workbench/        # workbench-content / workbench-page-title（工作台内容装配）
│   ├── diff/ merge/      # 差异查看（6）、合并视图（6）
│   ├── token-usage/      # 6 文件：用量统计图表
│   ├── ui/               # shadcn 基础组件
│   └── *.tsx             # appearance-provider、theme-provider、providers/(2)、
│                         #   connection/(2)、terminal/(3)、workspace/(3)、
│                         #   import-sessions/(3)、project-boot/(3)、shared/(10) 等顶层组件
├── hooks/                # 60+ hooks：use-connection、use-connection-lifecycle、use-delegated-sub-session、use-ime-guard 等
├── stores/               # Zustand：tab-store、app-workspace-store、conversation-runtime-store、backend-scoped-store-reset
├── i18n/
│   ├── messages/*.json   # 10 语言：en、zh-CN、zh-TW、ja、ko、es、de、fr、pt、ar
│   ├── request.ts        # next-intl request config
│   └── messages.ts       # 消息加载
└── test-setup.ts         # vitest 全局 setup
```

## 关键依赖

- 框架：`next@^16`、`react@^19`、`next-intl@^4`
- UI：`tailwindcss@^4`、`radix-ui`/shadcn、`lucide-react`、`motion`、`sonner`、`overlayscrollbars`
- 富内容：`streamdown`（+cjk/code/math/mermaid 子包）、`shiki`、`katex`、`react-markdown`、`@tiptap/*`（3.26 固定版本）
- 工具型组件：`@monaco-editor/react`（postinstall 复制到 `public/vs`）、`@xterm/*`、`virtua`（虚拟列表）、`react-resizable-panels`
- 状态：`zustand@^5`
- Tauri：`@tauri-apps/api@^2` + plugin-dialog/opener/process/updater/window-state

## 数据模型

`lib/types.ts` 是后端 `src-tauri/src/models/` 的 TS 镜像：conversation、folder、message、agent、automation、chat_channel、model_provider、pet、quick_message、token_usage、work_task 等。新增后端模型字段时必须同步此文件。

## 测试与质量

- vitest + jsdom，`*.test.ts(x)` 与源文件同目录；配置见根 `vitest.config.ts`（alias `@` → `./src`，coverage v8）
- 运行：`pnpm test` / `pnpm test:watch` / `pnpm test:coverage`；lint：`pnpm eslint .`
- 测试范围覆盖领域逻辑（transport、adapters、stores、hooks）与交互组件（chat、ai-elements）

## 常见问题 (FAQ)

- **为什么没有动态路由？** 静态导出（`output: "export"`）不支持，用查询参数替代
- **`public/vs` 是什么？** postinstall 从 `node_modules/monaco-editor/min/vs` 复制而来（并去除 sourceMappingURL），属 gitignore 产物，勿手工编辑
- **新增语言？** 在 `i18n/messages/` 加 JSON，并在 `next.config.ts` 的 locales 列表注册
- **桌面与 Web 行为分叉写在哪？** 统一走 `lib/transport/` 的 Transport 接口，禁止在业务组件里直接 `import { invoke } from "@tauri-apps/api"`

## 相关文件清单（高信号）

- `app/layout.tsx`、`app/page.tsx`、`app/workspace/page.tsx`
- `lib/transport/index.ts`、`lib/transport/detect.ts`、`lib/transport/web-transport.ts`、`lib/transport/remote-desktop-transport.ts`
- `lib/api.ts`、`lib/types.ts`、`lib/tauri.ts`
- `components/ai-elements/message.tsx`、`components/chat/chat-input.tsx`、`components/chat/message-input.tsx`
- `stores/tab-store.ts`、`stores/app-workspace-store.ts`
- `i18n/request.ts`、`i18n/messages/en.json`

## 变更记录 (Changelog)

- **2026-08-31 18:10:49 — 初始化架构师生成**：基于全仓扫描创建本模块文档（1055 个源文件清点；transport 三模式、路由表、lib/components 分区均经源码核实）。
- **2026-08-31 18:50 — 补扫**：components/ 全部 32 个分区逐目录清点并标注实测规模（settings 88 为最大分区、message 83、layout 62、chat 52、tasks 38），workbench 分区确认为 2 文件装配层。
