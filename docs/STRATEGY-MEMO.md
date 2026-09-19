# Codeg 战略备忘录

> 内部决策记录。整理于一次深度代码评审 + 商业化讨论之后。
> 目的:把散落在对话里的分析、判断、案例、待办沉淀下来,避免重复讨论。
> 2026-08-16 第二次更新:性能深度扫描 + 上游 issue 需求验证(见二、三、六、九节标注)。

---

## 一、项目定位(已确认,不再动摇)

**Codeg = 多智能体编码工作台(Multi-Agent Coding Workspace)。**

- 服务对象:**开发者**(不是非开发者,不是垂直行业)
- 核心能力:**跨进程异构 agent 委托**(不是单进程换模型)
- 不做:**Cursor 平替、通用 agent 平台、垂直行业方案**

### 定位边界(加宽 vs 跳船)

| 允许 | 禁止 |
|------|------|
| 覆盖开发者的更多环节(编码/文档/审查) | 脱离开发者去做通用市场 |
| 引入更多编码 agent(Pi/GenericAgent) | 引入非编码 agent 后转去做非开发场景 |
| 加宽定位(开发者全流程) | 跳船(服务完全不同的人群) |

---

## 二、技术资产评审(代码级事实)

### 已完成且扎实的部分

| 模块 | 规模 | 评价 |
|------|------|------|
| `acp/delegation/` | ~1.4 万行 Rust,200+ 测试 | **核心护城河**,工程深度极高 |
| `acp/` 顶层 | ~2.5 万行(connection/manager/lifecycle 等) | ACP 协议完整实现 |
| `parsers/` | ~1.8 万行,10 个 agent | 会话聚合,覆盖广 |
| 双模式架构 | codeg / codeg-server / codeg-mcp | 部署形态齐全,`_core` 函数共用 |

### delegation 模块的技术亮点(护城河来源)

1. **跨进程真委托** — `delegate_to_agent` 起独立 PID,不是进程内换 prompt
2. **异步 fan-out** — 同时开多个子 agent,`get_delegation_status` 批量长轮询
3. **取消级联** — 四种取消路径全覆盖(外部/子/父连接/父轮次)
4. **Setup 窗口竞态处理** — inflight 注册 + checkpoint + atomic park,零信任时序
5. **深度限制** — `depth.rs` 防递归爆炸,带 cap 防脏数据
6. **安全边界** — 一次性 token + parent scoping + UDS/piped 传输
7. **测试覆盖** — broker 109 测试、companion 53、listener 25,覆盖全边界条件
8. **trait 解耦** — `ConnectionSpawner` 为 v3 远程 agent 预留扩展点

### 当前短板(v1 限制,2026-08-16 复测)

1. **v1 是 one-shot** — `broker.rs:2527` `disconnect` 写死,子 agent 跑完即杀。`continue_with_session`/`close_session` 仍不存在,`acp/delegation/mod.rs:29` 注释还挂着 v2 计划
2. **巨石文件三座山** — `commands/acp.rs` 17020 行、`acp/connection.rs` 16155 行、`broker.rs` 8481 行(此前记 7554,又长了 ~930;前两个比 broker 更大,之前漏记)。broker 约 4750 行是测试,外移到 `broker_tests.rs` 是零风险减半
3. **没有远程 agent** — 只能调度本机,v3 未实现
4. **结果不落库(比原记轻)** — 完整文本只在 512MB 内存 FIFO 缓存(`broker.rs:79`);但有界 `text_preview` 已持久化进父会话 tool-call meta(`meta_writer.rs:281`),淘汰后 UI 仍有预览。真缺口是"全文不落库",不是"什么都不落"
5. **前端两个上帝对象** — `acp-agent-settings.tsx` 11818 行(单个设置页)、`acp-connections-context.tsx` 5606 行(~99 个 switch/case 的事件分发层)

### 性能优化机会(2026-08-16 深度扫描,按收益排序)

1. **侧边栏全量遍历** — `get_sidebar_data`/`list_folders`/`get_stats` 每次都走 `list_conversations_sync`(`commands/conversations.rs:342`),13 个 parser 全目录遍历,但文件夹树只需要 `folder_path`。前端已自己绕开(`skills-settings.tsx:713` 注释承认慢)。改 DB 文件夹索引 = 用户可感知的最大提升
2. **活跃会话整文件重解析** — 摘要缓存键 `(mtime,size)`(`summary_cache.rs:64`),流式期间每次列表刷新都对整个 JSONL 逐行反序列化(`claude.rs:846`)。`transcript_watermark` 已跟踪字节位置但从未用于增量读(`claude.rs:1866` 注释自认;codex 直接返回 `None`)。基础设施都在,只差接线
3. **patch 行号解析无缓存** — `resolve_patch_line_numbers` 每个 patch 块全量读目标文件(`parsers/mod.rs:1002`),7 个 parser 共用;同一大文件 N 个 patch = N 次全量读。一次 HashMap memo 即可
4. **会话文件定位 O(全树)** — Codex 按 id 每次 WalkDir(`codex.rs:336`),Claude 全目录 read_dir(`claude.rs:1060`);id→path 索引变 O(1)
5. **干净的部分(不用动)** — DB 索引齐全、无 N+1、WAL 配置正确;前端虚拟化(virtua)+ RAF 批处理是真做了的;启动无阻塞扫描,感知慢在首次侧边栏(冷缓存 + 上述第 1 条)

---

## 三、产品演进路线(v1 → v2 → v3)

### v1(现状):一次性委托

- 子 agent 跑完第一轮 `TurnComplete` → broker `disconnect` → 杀掉
- 父 LLM 拿到结果文本,委托结束
- 暴露 6 个工具:`delegate_to_agent` / `get_delegation_status` / `cancel_delegation` / `check_user_feedback` / `ask_user_question` / `get_session_info`

### v2:多轮子会话(Continue Session)— 短期 1-2 月

**目标**:子会话长期存活,支持多轮交互

**工具变化**:新增 `continue_with_session` / `close_session`

**为什么不难**:
- ACP 协议本身支持多轮(Codeg 已实现 `session/load` / `session/resume`,主会话在用)
- 只需删除 `finalize_delegation` 末尾的 `disconnect`,改成等显式 close
- 取消级联/深度限制/inflight 注册早已为长期会话设计,现在大材小用

**真正难点**:状态管理(TTL、并发上限、父结束时的级联关闭、中途失败语义)

**价值**:从"工具调用"升级为"Agent 团队",产品形态质变

### v3:远程 Agent(Remote Spawner)— 中期 3-6 月

**目标**:agent 跑在远程,broker 不变

**架构**:新增 `RemoteSpawner` 实现 `ConnectionSpawner` trait,broker 零改动

**难点不在 Codeg**(写 RemoteSpawner 不难),在基础设施:
- 远程执行环境(sandbox、API key 管理、资源隔离)
- 网络可靠性(远程超时处理)
- 安全(权限模型、审计)
- 认证(多租户隔离)
- 计费

**价值**:从"本地工具"升级为"云平台",商业化的钥匙

**需求验证(2026-08-16)**:上游 issue #461「以持久 Session 为成员的 Team / Chatroom」——社区独立提出了与功能 B/v2 同构的构想。这是 v2 值得做的最直接外部证据。

### v2 必须先于 v3 的理由

1. 远程 agent 冷启动成本高,只换一个 turn 的结果经济不成立 → 需要 v2 多轮
2. v2 纯本地,风险低,能快速验证产品形态

---

## 四、商业化分析

### 市场定位的真实判断

**Codeg 的差异化不是"多智能体",而是"多智能体的跨进程编排"。**

- OpenCode / Pi / DriFox 做的是"单进程多模型/多 prompt"(第一层)
- Codeg 做的是"跨进程真委托"(第二层)
- **不在一个维度上竞争**

### 四条变现路径(垂直行业已划掉)

| 路径 | 可行性 | 说明 |
|------|--------|------|
| 开发者工具变现(企业版/团队) | 🟢 最现实 | 开发者为效率付费,Codeg 主场 |
| 托管云 | 🟡 有需求 | 但要扛运维成本 |
| API 计费层 | 🟡 灰色 | 中国市场对接海外模型有法律风险 |
| ~~垂直行业~~ | ❌ 划掉 | 底座是编码 agent,做不了行业方案 |

### 为什么垂直行业划掉

- 垂直行业(律所/金融/医疗)不会以 Claude Code/Codex 为底座
- 这些 agent 是通用编码 agent,不具备行业专业性
- 行业数据不能出境(合规问题)
- 没有行业知识库

### 最诚实的商业化判断

**最大风险:在"多 agent 协作"需求真正普及前,上游平台方可能下场。**
- 窗口期 12-18 个月
- 最该做的:抢心智占有率("多智能体协作 = Codeg")

**最该先验证的问题(未回答)**:
1. 现在有没有人重度依赖 Codeg 的 delegation?
2. 你和同事自己日常用 delegation 吗?

> 这两个问题不回答,所有路径都是空中楼阁。先找 10 个真实用户问三个问题:
> 你用它干过什么?没有它你会怎么做?愿意付多少钱?

---

## 五、案例研究

### CoolVibe(coolvibe.io)— 参照系

| 维度 | 数据 |
|------|------|
| 本质 | Agent 的网页查看器(壳子) |
| 投入 | 4 个月,~3 万美元 |
| 定价 | 免费 / Pro $29.9/年 |
| 现状 | "订阅免费送"(还在买用户阶段) |
| V2EX 热度 | 1054 条回复 |
| 公开营收 | 查不到 |
| 团队 | xterminal 团队(有成功经验) |

**启示**:
1. "壳子"能赚钱,但前提是痛点明确且高频
2. 定价对齐"省下的时间",不是"技术多复杂"
3. 免费送订阅是冷启动的合理策略
4. CoolVibe 比深得多的 Codeg 先验证了"给 agent 做壳"能跑通

### DriFox(github.com/martin98-afk/DriFox)— 反面教材 + 经验库

| 维度 | 数据 |
|------|------|
| 本质 | PyQt 桌面对话助手(Cursor 平替) |
| 规模 | 15 万行 Python |
| Stars | **24**(2 个月) |
| 定位错误 | 堆 33 插件做"Cursor 平替",死路 |

**DriFox 走的错路(Codeg 绝不能走)**:
- 自己做 agent 和 Cursor 正面打(蚂蚁 vs 太阳)
- 堆功能数量拼不过飞轮(数据/生态/资本)
- "平替"定位是诅咒(永远活在正品阴影里)

**Cursor 的体量(参照)**:
- ARR $40 亿(2026.06 年化)
- 估值 $290-500 亿
- 日活 100 万+
- 融资 $23 亿 D 轮

### ACP 协议现状

- 官方 35+ 兼容 agent
- 已被 Zed / JetBrains / Google / GitHub 采纳
- 事实标准(类似 LSP 之于语言服务器)
- **zcode 不在列表**(截至 2026-07)

---

## 六、可执行的功能规划

### 🎯 功能 A:编程 PK(周末项目,优先级最高)

**是什么**:同一个任务同时分发给多个 agent,看谁做得好

**为什么做**:
1. 成本极低(底层全有,只缺对比 UI)——周末能做完
2. 硬核技术的最佳展示窗口(把不可见的委托变成可视化)
3. 赛道真空(DriFox 24 star 不构成威胁,异构 PK 无人做)
4. 自带传播(天然话题性 + 可视化 + 争议性)
5. 低成本的市场探测器(PK 没人理 → 大方向要重新考虑)

**为什么 Codeg 能做、别人不能**:
- Cursor:单进程,没法真异构并行
- DriFox:单进程,SubAgent 是换 prompt
- OpenCode/Pi:同上
- **只有 Codeg 有跨进程 ACP 委托**

**技术可行性(已确认)**:
- `delegate_to_agent` 已支持 fan-out ✅
- `get_delegation_status` 已支持批量等 ✅
- `DelegationSuccess` 已带 `duration_ms`/`token_usage`/`turn_count` ✅
- `DelegationContext` 已流到前端 ✅
- **只缺一层横向对比 UI**

**实施档位**:

| 档位 | 内容 | 工作量 |
|------|------|--------|
| A 最小可玩 | PK 触发器 + 分屏视图 + 计分板 | 周六一天 |
| B 好看+分享 | diff 对比 + 实时终端 + 分享截图 + 任务模板 | +周日上午 |
| C 评判 | 自动测试裁判 + LLM 裁判 | 周日下午(选做) |

**关键代码位置**:
- 触发器:`src/components/composer/`(加 PK 模式开关)
- 对比组件:新建 `src/components/chat/agent-pk-arena.tsx`
- 数据复用:`src/hooks/use-delegation-card-model.ts`(已解出 agent/task/status/childId)
- 渲染复用:`src/components/message/sub-agent-session-dialog.tsx`(已会渲染单个子 agent)
- 分享截图:`html-to-image`(package.json 已有依赖)

**传播策略**:
- V2EX 发帖,标题方向:"周末做了个 AI 编程 PK 场:让 Claude Code、Codex、Gemini 同时写贪吃蛇,看谁快"
- 每场 PK 都是一次传播机会

**需求验证(2026-08-16)**:上游 issue #428「缺乏派发 subagent 状态的监视」——用户独立要 delegation 可视化监控,与 PK 共用数据层(`use-delegation-card-model.ts`),做完 PK 顺手就有,算免费赠品。

### 🎯 功能 B:角色化 Agent 团队(PK 之后的第二张牌)

**是什么**:预设 leader/build/review 等角色,一键发起"Claude 当 leader 分任务给 Codex(build)和 Gemini(review)"

**为什么做**:
- 比 PK 更接近"真实有用的工作"(协作 > 竞技)
- 和 PK 共用同一套 delegation 底层,只换 prompt 和 UI
- 让 Codeg 从"对比工具"延伸为"团队编排平台"

**来源**:DriFox 的 `plugins/system/agents/*.md`(角色 prompt 设计精良,可借鉴)

**与 v2 的关系**:角色化团队天然需要多轮子会话(v2),但可以先做 one-shot 版本

---

## 七、从 DriFox 可借鉴的具体设计

### 值得偷(按价值排序)

#### 1. 角色化 agent prompt 模板
- 来源:`plugins/system/agents/{build,leader,review,explore,plan}.md`
- 用途:Codeg 的角色化团队功能
- 关键原则(直接抄):
  - "不做超出需求的功能。一次性代码不做抽象。"
  - "不要顺手改进相邻代码。不要重构没坏的东西。"
  - "每一行变更都应该能直接追溯到用户的请求。"
  - "使用 question 工具提问,优先提供选项"

#### 2. 工具安全分类(三分法)
- 来源:`app/tools/command_safety.py` + `app/tools/tool_classifier.py`
- 用途:Codeg 加一层自己的安全护栏(企业版刚需)
- 规则:
  - 危险(write/edit/bash/mouse)→ 审批
  - 安全(read/grep/glob/webfetch)→ 放行
  - 命令三分法:无元字符直接跑 / 有管道重定向需确认 / 黑名单拒绝

#### 3. "关键文档"记忆机制
- 来源:`app/core/memory_manager.py` 的 `KeyDocumentsRepository`
- 用途:解决子 agent 冷启动不懂项目上下文的痛点
- 做法:用户标记项目重要文件(README/架构文档/API 约定),委托时自动注入 task prompt

#### 4. 任务邮件分发(轻量跨窗口协作)
- 来源:`app/core/team_manager.py`(文件邮箱)
- 用途:v3 远程 agent 的备选轻量方案
- 优先级:低(记住有这个方案)

#### 5. AGENTS.md 作为项目笔记
- DriFox 印证了这是事实标准,Codeg 已在用,继续走

### 明确不要偷

| 不偷 | 理由 |
|------|------|
| PyQt 桌面浮动窗 | 架构不同,非核心价值 |
| 33 个插件 | 功能堆砌是它 24 star 的原因 |
| 进程内 SubAgent DAG | Codeg 跨进程委托更强 |
| ECharts 力导向图 | PK 用并排对比更直观 |
| LSP 集成(11 种语言) | Cursor 的战场,别送 |

---

## 八、关于接入 zcode 的结论

**结论:暂不接入。**

理由:
- zcode 不开源,改不了,只能向 Z.ai 提需求
- zcode 不支持 ACP(不在官方 35+ 兼容列表)
- 不支持 ACP → 只能做 L1(会话导入 parser),不能做 L2(完整调度)
- L1 单独价值低(装饰性,用户在 zcode 自己的 app 里已能看会话)
- zcode 加 ACP 是 Z.ai 的战略决策,Codeg 这边无法推动

**zcode 数据格式(已摸清,备用)**:
- `~/.zcode/cli/db/db.sqlite` — 主库(session/message/part/todo 表)
- `~/.zcode/v2/tasks-index.sqlite` — 任务索引
- `~/.zcode/cli/rollout/*.jsonl` — 原始模型 IO 流
- `~/.zcode/cli/agents/sess_*/agent_*/` — 子 agent 会话

如果未来 zcode 支持 ACP,接入成本分两档(2026-08-16 修正,依据:上游 DSH 集成案例):

**L1(会话导入,半天,4 处)**:
1. `models/agent.rs` — `AgentType` 枚举
2. `acp/registry.rs` — `AcpAgentMeta`
3. `parsers/mod.rs` — 注册
4. `parsers/zcode.rs` — 会话导入(可选)

**L2(完整内置 agent,六层 ~25 处,量级 3-5 天)**:
完整参考:上游提交 `3845e9df`「feat(deepseek): integrate DeepSeek Harness as a built-in agent」,+3271/−50 行,改动约 25 个文件。六层清单:

| 层 | 改动点 | 示例(DSH 提交) |
|----|--------|----------------|
| 1. 身份层 | `AgentType` 枚举 + wire 名 + 防自定义 shadow | `models/agent.rs`(+12) |
| 2. 启动层 | 注册表元数据:npm 包/版本/node 要求/认证方式 | `acp/registry.rs`(+44) |
| 3. 历史层 | 原生会话解析器(会话进列表/统计/导入,工作量最大) | `parsers/deepseek.rs`(+1291) |
| 4. 运行时层 | 沙箱根目录 / env 键 / skills 存储 / 只读路径 | `file_system_runtime.rs`、`commands/acp.rs`(+237) |
| 5. 协议层 | ACP capabilities / MCP 接线 / 传输限制 | `connection.rs`、`commands/mcp.rs`(+398) |
| 6. 防御层 | 自定义注册表冲突校验 + 文档 | `custom_registry.rs`(+52) |

> **教训**:曾以为"加一个 agent = 改 4 处",实际"内置 agent"是六层全通——只做 1/2 层的 PR 会被上游以"不够一等公民"拒绝(2026-08-16 DSH PR 实测)。以后评估任何"接新 agent"的工作量,按此清单逐层核对。

---

## 九、关键待办与未决问题

### 必须先回答的问题(阻塞所有商业化决策)

- [ ] **现在有没有人重度依赖 Codeg 的 delegation?**(找 10 个真实用户)
- [ ] **你和同事自己日常用 delegation 吗?**(如果造的人都不用,别指望别人用)

### 功能待办(按优先级)

- [ ] **编程 PK**(周末项目) — 最低成本验证 + 传播
- [ ] **角色化 Agent 团队**(PK 之后) — 第二个场景
- [ ] **v2 多轮子会话** — 产品形态质变,1-2 月
- [ ] **v3 远程 agent** — 商业化钥匙,3-6 月

### 工程待办(2026-08-16 新增,按收益排序)

- [ ] **侧边栏 DB 文件夹索引** — 干掉 13-parser 全量遍历(性能机会 1)
- [ ] **增量会话解析** — 接通已有但闲置的 `transcript_watermark`(性能机会 2)
- [ ] **broker.rs 测试外移** — ~4750 行拆到 `broker_tests.rs`,零风险减半
- [ ] **patch 行号解析加 memo** — 一次 HashMap 的事(性能机会 3)

### 上游影响力机会(刷存在感,非主线)

- [ ] **#391/#387 流式重复文本** — 维护者自己未定位根因(在等用户补 seq 数据);修了就是硬通货
- [ ] **#396 preferred-config 过滤** — 0xlinn 已给出精确根因:通用路径不按 advertised options 过滤,867 次/23天的重复报错;修法是照抄 grok 路径已有守卫(`connection.rs:1947`)+ `mode` 白名单
- [x] **#408 接入 qoder cli**(2026-08-17 已完成,待提 PR)— 六层全通:身份/注册表(`qoder-cli`,npx `@qoder-ai/qodercli@1.1.23`,`--acp`)/解析器(Claude 信封格式,`~/.qoder/projects/<encoded-cwd>/<uuid>.jsonl`,state.json 标题加密故走 transcript 明文)/沙箱(`QODER_CONFIG_DIR` RootSlot,实测 `~/.qoder` 可写)/协议(原生 ACP,loadSession+list/resume/fork 全有,MCP 走 settings.json 合并写入+转发跳过)/防御(`qoder-cli` 冲突校验)。规避了既往坑:#396 serde 重命名(测试钉死)、过期 env 误报(important keys 置空)、#468 标题问题(明文派生)。活体验证:server 模式 spawn→握手→prompt→turn_complete→transcript 解析全通。qoder 会话历史与 DSH 等宽(比 DSH 便宜在原生 ACP,贵在 MCP 转发语义相反:qoder 读自己的 settings.json,进跳过名单)

### 不做清单(防止分心)

- [x] ~~接入 zcode~~(暂缓,等 ACP)
- [x] ~~垂直行业方案~~(底座不匹配)
- [x] ~~做 Cursor 平替~~(蚂蚁 vs 太阳)
- [x] ~~堆功能数量~~(学 DriFox 的教训)
- [x] ~~脱离开发者市场~~(丢弃所有资产)

---

## 十、一句话总结

> **Codeg 的硬核技术(跨进程异构委托)是真的,但"硬核"本身不产生收入,产生收入的是"有一群人离不开它"。**
>
> **当下的优先级:先用最低成本(PK)验证"多 agent 并行"对开发者有没有吸引力 → 如果有,做 v2/v3 深化 → 如果没有,重新评估方向。**
>
> **别再纠结"能不能打过 Cursor"——那不是 Codeg 该问的问题。该问的是:Cursor 用户有什么事想做但 Cursor 架构上做不到?**
