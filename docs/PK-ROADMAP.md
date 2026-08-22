# PK Arena 演进路线图

> 整理于 2026-08-19。基于当前 `feat/agent-pk-arena` 分支已实现的基础设施，
> 规划下一阶段四个增强功能，目标是把 PK 做成真正的引流点。

---

## 现状（已实现）

| 能力 | 状态 | 关键文件 |
|------|------|----------|
| 多 agent 同任务 PK（2-8 选手） | ✅ | `pk-launcher-dialog.tsx` |
| 每选手独立 git worktree 隔离 | ✅ | `use-pk-round.ts` → `gitWorktreeAdd` |
| 权限模式（default/acceptEdits/bypass） | ✅ | launcher + `applyPermissionMode` |
| 思考强度（effort）统一设定 | ✅ | launcher + `applyPreparedOptions` |
| 裸机模式（禁用 skills） | ✅ | `BARE_MODE_RULES` |
| 选手级 model/effort 选择器 | ✅ | `applyContestantSelection` |
| 实时 transcript 分屏 | ✅ | `pk-arena-dialog.tsx` |
| diff 对比 + 计分板 + 截图分享 | ✅ | `pk-diff-view.tsx` / `pk-scoreboard.tsx` |
| 回合持久化到 DB | ✅ | `pk_round` 表 + store hydrate |
| 中断恢复（interrupted） | ✅ | `dbRoundToStoreRound` |

---

## 四个新功能

### 功能 1：快捷开赛模板

**问题**：每次 PK 都要手动选 agent、写任务、调参数，门槛高。

**方案**：预设一组常见任务模板，一键填入 launcher。

- 内置模板（基于 AI 圈知名的一句话 benchmark，结果可视化、自带话题性）：

  | 模板 | 任务文本 | 测试维度 | 来源 |
  |------|----------|----------|------|
  | 鹈鹕骑车 | `Generate an SVG of a pelican riding a bicycle` | 空间推理 + SVG 编码 + 指令遵循 | Simon Willison 2024-10，AI 圈最著名的非正式 benchmark |
  | 球在三角形里弹跳 | `Create an HTML animation: a ball starts in the center of a triangle. Every time it hits a side it speeds up, and the shape gains an extra side (Triangle→Square→Pentagon→Hexagon…)` | 动画逻辑 + 物理 + 动态形状 | Instagram 病毒对比帖，Qwen Coder 胜出 |
  | 果冻 blob | `Build a tiny browser toy: a jelly blob. You poke, grab, stretch it. No scoring, no level, just a satisfying blob.` | 交互物理 + harness 能力 | DeepSeek-Reasonix 的 harness benchmark |
  | 贪吃蛇 | `Write a Snake game in a single HTML file with keyboard controls.` | 基础工程 + 游戏逻辑 | 经典编程测试 |
  | Flappy Bird | `Write a Flappy Bird clone in a single HTML file.` | 游戏逻辑 + Canvas | 常见 LLM 对比题 |
  | 语音聊天 | `Create a voice-enabled chatbot web app using the Web Speech API.` | 多功能集成 + API 调用 | YouTube LLM 对比赛 |

- 用户可自定义模板（存 localStorage 或 DB）
- 模板内容：`{ name, task, suggestedAgents?, bareMode?, effort? }`
- 入口：launcher 对话框顶部加一排模板按钮，点击即填
- 引流角度：鹈鹕骑车已是 AI 圈共识 benchmark，"用 Codeg PK 场跑鹈鹕骑车"自带搜索流量和话题认同

**工作量**：小。纯前端，不改后端。

### 功能 2：真实工程 PK

**问题**：一句话任务太玩具，不反映 agent 在真实项目里的能力。

**方案**：PK 直接在当前打开的项目里跑，选手各自在 worktree 里实现特性 / 修 bug。

- 现有基础设施**已支持**：launcher 已从 activeTab 读 `workingDir`，`gitWorktreeAdd` 已在项目下建 `.codeg-pk/<round>/<agent>/` worktree
- 差的是**任务来源体验**：
  - 目前只能手输任务文本
  - 增强：支持从 git diff / commit message / TODO 注释 / GitHub issue 拉取任务描述
  - 可选：在项目文件树里点选"就拿这个文件的问题来 PK"
- 选手的 diff 已经对基准分支做（`gitDiffWithBranch`），真实工程的改动能正确捕获

**工作量**：中。主要是前端任务输入增强 + 可选的 issue 拉取（需 GitHub API）。

### 功能 3：主裁判自动打分

**问题**：PK 结果靠人肉看 diff，没有量化评分，不够系统，也不便传播。

**方案**：指定一个 agent 当裁判，读所有选手 diff 后打分排座次。

- 回合结束后（所有选手 `done`），自动启动裁判 agent
- 裁判 prompt 包含：任务描述 + 每个选手的 diff + 评分维度（正确性 / 代码质量 / 效率 / 完成度）
- 裁判输出结构化评分（JSON：每选手每维度分数 + 总分 + 排名 + 点评）
- 评分结果展示在计分板下方，可随截图一起分享
- 裁判可以是任意已安装的 agent（甚至可以加入一个"不参赛只裁判"的 agent）
- 可选：多裁判投票制（2-3 个裁判各自打分取平均）

**工作量**：中。需要：
- 后端：`pk_round` 表加 `judge_agent` + `judge_result` 字段
- 前端：launcher 加裁判选择器，arena 加评分展示区
- 编排：`use-pk-round.ts` 在回合结束后触发裁判连接

### 功能 4：控制变量 PK（同 agent 不同配置）

**问题**：想做"同一 agent 跑不同 model / effort"的对比，但当前一个 agent 只能选一个槽位。

**方案**：选手身份从 `agentType` 升级为 `(agentType, slotLabel)`，允许同一 agent 出现多次。

- 核心改动：contestant 的唯一键从 `agentType` 改为 `contestantId`（`agentType + slot 索引`）
- 影响面：
  - `contestantBranchName` / `contestantContextKey` 加 slot 后缀
  - `PkRoundConfig.agents` 从 `string[]` 改为 `Array<{ agent: string; label?: string }>`
  - DB `pk_round.config` JSON 结构升级（需兼容旧数据）
  - 前端 launcher 支持重复添加同一 agent，每个槽位单独设 model/effort
- 典型场景：
  - Claude Code × Sonnet vs Claude Code × Opus（同 agent 不同 model）
  - Codex × medium vs Codex × high（同 agent 不同 effort）
  - Codex 裸机 vs Codex 带 skills（同 agent 不同 bareMode）
  - 同 agent 不同 system prompt / MCP 配置（更远期）

**工作量**：中偏大。改动横跨前后端 + DB schema + 编排逻辑，是四个功能里最重的。

---

## 实施优先级

| 顺序 | 功能 | 理由 | 预估工作量 |
|------|------|------|-----------|
| 1 | 快捷开赛模板 | 改动最小、体验提升最直接、立刻可用 | 半天 |
| 2 | 主裁判自动打分 | PK 结果可量化 = 引流核心素材，差异化最强 | 1-2 天 |
| 3 | 控制变量 PK | 直击用户真实疑问（Opus 值不值 / high 强多少），天然话题 | 2-3 天 |
| 4 | 真实工程 PK | 基础设施已就绪，增强任务来源即可，但不急于做 issue 拉取 | 1 天（基础）/ 2-3 天（含 issue） |

**建议**：1 → 2 → 3 → 4 顺序做。1 和 2 做完就能产生第一批传播素材；3 做完 PK 的"控制变量"叙事就完整了；4 是锦上添花。

---

## 技术约束与风险

- **功能 3（裁判）**：裁判也是 one-shot 委托，裁判 agent 的 diff 不参与排名，只输出评分。注意裁判 token 也算成本。
- **功能 4（控制变量）**：DB schema 变更需写迁移，旧 `config.agents: string[]` 要兼容读。建议用 discriminated union：`agents` 既接受旧 `string[]` 也接受新 `Array<{agent, label}>`，读取时统一归一化。
- **所有功能**：保持双模式（Tauri + Axum）兼容，`_core` 函数共用，前端 transport 自动检测。
- **测试**：每个功能完成后跑 `pnpm test` + `cargo test --features test-utils`，功能 4 需新增 DB 迁移测试。
