# PK Arena 测试用例

> 给测试 agent 用。基于 server 模式 + Playwright 浏览器自动化。
> 测试前先读「环境准备」一节。

---

## 环境准备

### 1. 构建 server 二进制

```bash
cd src-tauri
cargo build --no-default-features --bin codeg-server
```

### 2. 构建前端静态文件

```bash
pnpm build   # 产出 out/ 目录
```

### 3. 启动 server

```bash
CODEG_PORT=3080 \
CODEG_HOST=127.0.0.1 \
CODEG_TOKEN=test-token-123 \
CODEG_DATA_DIR=/tmp/codeg-pk-test \
CODEG_STATIC_DIR=$(pwd)/out \
./src-tauri/target/debug/codeg-server
```

### 4. 浏览器访问

```
http://127.0.0.1:3080
```

登录页输入 token：`test-token-123`

### 5. 准备测试用 git 仓库

PK 需要一个有 git 仓库的 folder。测试前创建：

```bash
mkdir -p /tmp/codeg-pk-test-repo
cd /tmp/codeg-pk-test-repo
git init
echo "# Test Repo" > README.md
git add . && git commit -m "initial commit"
echo "print('hello')" > hello.py
git add . && git commit -m "add hello.py"
```

然后在 Codeg 里「添加文件夹」指向 `/tmp/codeg-pk-test-repo`。

### 6. 安装 agent

PK 只列出已安装的 agent。至少需要 2 个已安装的 agent（如 Claude Code、Codex）。
如果测试环境没有真实 agent，可以用 mock——但 UI 自动化测试主要验证前端交互逻辑，
agent 未安装时 launcher 不显示选手按钮，这是预期行为，应作为边界用例。

---

## 选择器约定

PK 组件没有 data-testid（除 scoreboard 和 minimized-pill）。
用以下策略定位元素：

| 元素 | 选择器 |
|------|--------|
| Launcher 对话框 | `role="dialog"` 内含文本 "Agent PK" |
| 选手按钮 | 选手区域的 `button[aria-pressed]`，文本匹配 agent 名称 |
| 模板按钮 | `button[title]`，title 属性包含模板任务文本 |
| 任务文本框 | `#pk-task` |
| 权限单选 | `input[name="pk-permission"]` |
| effort 按钮 | effort 区域的 `button[aria-pressed]` |
| 裸机模式复选框 | bareMode label 内的 `input[type="checkbox"]` |
| 裁判按钮 | 裁判区域的 `button[aria-pressed]`，"No judge" 或 agent 名 |
| 开始按钮 | 对话框底部含 "Start match" 文本的 button |
| 取消按钮 | 对话框底部含 "Cancel" 文本的 button |
| 计分板 | `[data-testid="pk-scoreboard"]` |
| 最小化浮标 | `[data-testid="pk-minimized-pill"]` |
| Arena 对话框 | `role="dialog"` 内含 "Agent PK arena" |
| Tab 按钮 | 含 "Battle" 或 "Diff" 文本的 button |
| 选手状态 | 计分板内 `span` 文本 |

---

## 测试用例

### TC-01：Launcher 打开与初始状态

**前置**：已登录，已打开一个 git 仓库 folder，该 tab 处于活跃状态。

**步骤**：
1. 找到入口触发 PK launcher（左侧栏的 ⚔ 图标，或 composer 菜单里的 PK 选项）
2. 等待对话框出现

**验证**：
- [ ] 对话框可见，标题为 "Agent PK"
- [ ] 选手区域可见，显示 "Contestants (2-4)" 标签
- [ ] 已安装的 agent 以圆形按钮列出，每个带图标和名称
- [ ] 每个选手按钮 `aria-pressed="false"`（未选中状态）
- [ ] 任务文本框 `#pk-task` 存在且为空
- [ ] 任务文本框上方有模板按钮行（鹈鹕骑车、弹球、果冻 Blob、贪吃蛇、Flappy Bird、语音聊天）
- [ ] 权限区域默认选中 "default"（第一个 radio）
- [ ] effort 区域默认选中 "Default"
- [ ] 裸机模式复选框未选中
- [ ] 裁判区域可见，"No judge" 按钮处于选中状态（`aria-pressed="true"`）
- [ ] 底部 "Start match" 按钮处于 disabled 状态
- [ ] 底部计数显示 "0/8 picked (min 2)"

### TC-02：选手选择与取消

**前置**：TC-01 通过。

**步骤**：
1. 点击第一个 agent 按钮
2. 验证该按钮 `aria-pressed="true"`
3. 点击第二个 agent 按钮
4. 验证两个按钮都 `aria-pressed="true"`
5. 点击第一个 agent 按钮取消

**验证**：
- [ ] 选中后按钮样式变化（border-primary + bg-primary/10）
- [ ] 底部计数更新为 "1/8 picked (min 2)"（选一个再取消一个后）
- [ ] 只选 1 个时显示 "Pick at least 2 agents to run a match." 提示
- [ ] 选 2 个后提示消失
- [ ] "Start match" 按钮在选满 2 个 + 有任务文本后变为 enabled

### TC-03：快捷模板填充

**前置**：TC-01 通过。

**步骤**：
1. 点击 "🦤 Pelican" 模板按钮

**验证**：
- [ ] `#pk-task` 文本框内容变为 "Generate an SVG of a pelican riding a bicycle."
- [ ] 模板按钮的 `title` 属性包含完整任务文本

**步骤**：
2. 清空文本框（手动或点另一个模板覆盖）
3. 依次点击每个模板按钮，验证文本框内容

**验证**：
- [ ] "⚽ Bouncing Ball" → "Create an HTML animation: a ball starts in the center of a triangle..."
- [ ] "🫧 Jelly Blob" → "Build a tiny browser toy: a jelly blob..."
- [ ] "🐍 Snake" → "Write a Snake game in a single HTML file with keyboard controls."
- [ ] "🐤 Flappy Bird" → "Write a Flappy Bird clone in a single HTML file with Canvas rendering."
- [ ] "🎙️ Voice Chat" → "Create a voice-enabled chatbot web app using the Web Speech API."

### TC-04：从 Git 提交拉取任务

**前置**：TC-01 通过，当前 folder 是 git 仓库且有至少 2 个 commit。

**步骤**：
1. 找到 "📋 From commit" 按钮（在模板按钮行末尾）
2. 点击它

**验证**：
- [ ] 出现一个下拉列表，显示最近的 commit
- [ ] 每个 commit 显示 hash 前 7 位 + commit message 第一行
- [ ] 如果还在加载，显示 "Loading commits…"

**步骤**：
3. 点击第一个 commit

**验证**：
- [ ] 下拉列表关闭
- [ ] `#pk-task` 文本框内容变为 "Reproduce the change from commit <hash>: <message>"
- [ ] 文本中的 hash 是完整 hash 还是短 hash 取决于 API 返回，验证前 7 位匹配

### TC-05：权限模式切换

**前置**：TC-01 通过。

**步骤**：
1. 默认状态验证 "default" radio 被选中
2. 点击 "acceptEdits" radio
3. 点击 "bypassPermissions" radio
4. 点回 "default"

**验证**：
- [ ] 每次切换后，对应 radio 的 `checked` 属性为 true
- [ ] 每个选项旁边有提示文本（如 "file edits run without asking"）
- [ ] 权限区域下方有说明文本

### TC-06：Effort 等级切换

**前置**：TC-01 通过。

**步骤**：
1. 默认状态验证 "Default" 按钮处于选中状态
2. 依次点击 Low → Medium → High → Max → Default

**验证**：
- [ ] 每次点击后，对应按钮 `aria-pressed="true"`
- [ ] 其他按钮 `aria-pressed="false"`
- [ ] 选中按钮有高亮样式

### TC-07：裸机模式切换

**前置**：TC-01 通过。

**步骤**：
1. 验证复选框初始未选中
2. 点击复选框

**验证**：
- [ ] 复选框变为选中状态
- [ ] 复选框旁有 "Bare mode (no skills)" 标签
- [ ] 下方有说明文本

### TC-08：裁判选择

**前置**：TC-01 通过。

**步骤**：
1. 验证裁判区域可见
2. 验证 "No judge" 按钮处于选中状态（`aria-pressed="true"`）
3. 点击一个 agent 作为裁判

**验证**：
- [ ] "No judge" 按钮变为未选中
- [ ] 选中的 agent 按钮 `aria-pressed="true"`
- [ ] 如果该 agent 已被选为选手，其裁判按钮显示 `opacity-40`（半透明，表示不建议自裁判）

**步骤**：
4. 点击 "No judge" 取消裁判

**验证**：
- [ ] "No judge" 按钮回到选中状态
- [ ] 之前选中的 agent 裁判按钮变为未选中

### TC-09：启动验证 — 缺选手

**前置**：TC-01 通过。

**步骤**：
1. 只选 1 个 agent
2. 在任务文本框输入 "test task"
3. 检查 "Start match" 按钮状态

**验证**：
- [ ] "Start match" 按钮处于 disabled 状态
- [ ] 显示 "Pick at least 2 agents to run a match." 提示

### TC-10：启动验证 — 缺任务

**前置**：TC-01 通过。

**步骤**：
1. 选 2 个 agent
2. 任务文本框留空
3. 检查 "Start match" 按钮状态

**验证**：
- [ ] "Start match" 按钮处于 disabled 状态

### TC-11：启动验证 — 非 Git 仓库

**前置**：打开一个非 git 仓库的 folder。

**步骤**：
1. 打开 PK launcher

**验证**：
- [ ] 选手区域不显示 agent 按钮（或显示"需要 git 仓库"提示）
- [ ] 显示 "This folder is not a git repository" 文本
- [ ] 有 "git init" 按钮
- [ ] "Start match" 按钮处于 disabled 状态

### TC-12：完整 PK 流程 — 启动到就绪

**前置**：已安装至少 2 个 agent，已打开一个 git 仓库 folder。

**步骤**：
1. 打开 PK launcher
2. 选 2 个 agent
3. 点击 "🦤 Pelican" 模板
4. 权限设为 "acceptEdits"
5. effort 设为 "Medium"
6. 勾选裸机模式
7. 选一个不参赛的 agent 作为裁判（如果有第 3 个 agent）
8. 点击 "Start match"

**验证**：
- [ ] Launcher 对话框关闭
- [ ] Arena 对话框打开
- [ ] Arena 顶部显示任务文本
- [ ] 状态显示 "Ready"（不是 "Live"）
- [ ] 计分板 `[data-testid="pk-scoreboard"]` 可见
- [ ] 计分板显示 2 个选手卡片
- [ ] 每个选手卡片显示 agent 图标、名称、状态点（amber/ready）
- [ ] 有 "Start match" 按钮在 arena 内（准备态的启动按钮）
- [ ] 如果选了裁判，裁判面板可见
- [ ] Battle tab 默认选中
- [ ] 每个选手面板显示 "Ready" 标签 + 模型/effort 选择器（如果 agent 通告了选项）

### TC-13：Arena — 模型/effort 选择器（准备态）

**前置**：TC-12 通过，选手处于 ready 状态且 agent 通告了 configOptions。

**步骤**：
1. 在第一个选手面板里，如果模型选择器存在，选择一个不同的模型
2. 如果 effort 选择器存在，选择一个不同的 effort

**验证**：
- [ ] 选择器是 `<select>` 元素或按钮组
- [ ] 选择后 UI 更新（选择器的值改变）
- [ ] 如果 agent 没有通告 configOptions，面板显示诊断信息（"no selectors"）

### TC-14：Arena — 启动比赛

**前置**：TC-12 通过，选手处于 ready 状态。

**步骤**：
1. 点击 arena 内的 "Start match" 按钮

**验证**：
- [ ] 状态从 "Ready" 变为 "Live"
- [ ] 每个选手的状态点从 amber 变为 emerald + animate-pulse
- [ ] 计分板开始显示计时（如果 startedAt 已设置）
- [ ] Battle tab 的选手面板从 ready 面板切换为 transcript 视图
- [ ] 如果 agent 在运行，transcript 内容逐步出现
- [ ] 取消按钮（"Cancel"）可见

### TC-15：Arena — 计分板实时更新

**前置**：TC-14 通过，比赛正在运行。

**步骤**：
1. 观察计分板 30 秒

**验证**：
- [ ] 选手状态从 "running" 可能变为 "done"（绿色实心圆点）
- [ ] 完成后显示耗时（如 "12s"）
- [ ] 完成后显示 token 数（如 "1.5k tok"）
- [ ] 完成后显示轮次数（如 "3 turns"）
- [ ] 计时在运行中每秒更新

### TC-16：Arena — 选手完成后结算

**前置**：TC-14 通过，至少一个选手完成。

**步骤**：
1. 等待所有选手完成

**验证**：
- [ ] 所有选手状态变为 "done" 或 "error"
- [ ] 回合状态从 "Live" 变为 "Finished"
- [ ] 每个选手的连接断开（侧边栏不再转圈）
- [ ] 如果配置了裁判，裁判自动启动（judgeStatus 从 idle 变为 running）

### TC-17：Arena — 裁判自动评分

**前置**：TC-16 通过，且在 TC-12 中选了裁判 agent。

**步骤**：
1. 所有选手完成后，等待裁判完成

**验证**：
- [ ] 裁判面板（PkJudgePanel）可见，位于计分板下方
- [ ] 裁判运行时显示 "Evaluating…" + 脉冲点
- [ ] 裁判完成后显示评分卡片：每个选手显示排名徽章（🥇🥈🥉）、分数（0-100）、点评
- [ ] 分数有颜色编码：≥80 绿色、≥60 黄色、≥40 橙色、<40 红色
- [ ] 评分按排名排序（rank 1 在前）
- [ ] 裁判面板下方有总体总结文本

### TC-18：Arena — 裁判无 JSON 输出时的容错

**前置**：裁判 agent 返回了非 JSON 文本（可通过断开裁判连接或 mock 模拟）。

**验证**：
- [ ] 裁判状态变为 "error" 或显示 "Judge response could not be parsed."
- [ ] 原始文本保留在 rawText 字段（可在报告导出中验证）
- [ ] 不崩溃，其他功能正常

### TC-19：Arena — Diff Tab

**前置**：TC-16 通过，至少一个选手完成。

**步骤**：
1. 点击 "Diff" tab

**验证**：
- [ ] Diff tab 被选中（有底部边框高亮）
- [ ] 如果 diff 还没加载，显示 "Loading diff…"
- [ ] 加载完成后，每个选手显示一个 diff 面板
- [ ] diff 面板包含 git diff 文本
- [ ] 如果选手没有改动，显示 "No changes in this worktree" 或中文等价文本
- [ ] 选手面板标题显示 agent 名称

### TC-20：Arena — 取消进行中的比赛

**前置**：TC-14 通过，比赛正在运行。

**步骤**：
1. 点击 "Cancel" 按钮

**验证**：
- [ ] 回合状态变为 "Canceled"
- [ ] 所有未完成的选手状态变为 "canceled"
- [ ] 选手连接断开
- [ ] 计分板停止计时
- [ ] ESC 键不能关闭 arena（运行中禁止误触关闭）

### TC-21：Arena — 最小化与恢复

**前置**：TC-14 通过，比赛正在运行。

**步骤**：
1. 点击 "Minimize" 按钮

**验证**：
- [ ] Arena 对话框关闭
- [ ] 屏幕右下角出现 `[data-testid="pk-minimized-pill"]` 浮标
- [ ] 浮标显示 "PK in progress" 文本

**步骤**：
2. 点击浮标

**验证**：
- [ ] Arena 对话框重新打开
- [ ] 比赛状态保持不变（仍在运行）

### TC-22：Arena — ESC 键防护

**前置**：TC-14 通过，比赛正在运行。

**步骤**：
1. 按 ESC 键

**验证**：
- [ ] Arena 对话框不关闭（被阻止）
- [ ] 比赛继续运行

**步骤**：
2. 比赛结束后（Finished 状态），按 ESC 键

**验证**：
- [ ] Arena 对话框正常关闭

### TC-23：Arena — 点遮罩防护

**前置**：TC-14 通过，比赛正在运行。

**步骤**：
1. 点击对话框外的遮罩区域

**验证**：
- [ ] Arena 对话框不关闭（被阻止）

### TC-24：Arena — 回合切换

**前置**：有 2 个以上的 PK 回合（已完成或进行中）。

**步骤**：
1. 打开 arena
2. 如果有多个回合，验证回合选择器（`<select>`）可见
3. 切换到另一个回合

**验证**：
- [ ] 回合选择器的每个 option 显示时间 + 选手数
- [ ] 切换后，arena 显示选中回合的任务、选手、状态
- [ ] 已完成回合的 live transcript 不可见（连接已断开），但持久化的会话内容仍可渲染

### TC-25：Arena — 清理 Worktree

**前置**：TC-16 通过，回合已完成。

**步骤**：
1. 找到 "Clean worktrees" 按钮
2. 点击它

**验证**：
- [ ] 选手的 worktree 被移除
- [ ] 分支保留（keepBranches=true）
- [ ] 按钮消失或变为不可用

### TC-26：Arena — 删除回合

**前置**：TC-16 通过。

**步骤**：
1. 找到 "Delete" 按钮
2. 点击它
3. 在确认对话框点确认

**验证**：
- [ ] 回合从列表中移除
- [ ] 如果还有其他回合，自动切换到第一个
- [ ] 如果没有回合了，arena 显示 "No round selected"

### TC-27：Arena — 分享截图

**前置**：TC-16 通过，计分板有数据。

**步骤**：
1. 点击 "Share" 按钮

**验证**：
- [ ] 按钮文本变为 "Exporting…"
- [ ] 导出完成后（按钮恢复 "Share"），浏览器触发文件下载
- [ ] 下载的文件名匹配 `codeg-pk-<round-id>.png`

### TC-28：Arena — 导出报告

**前置**：TC-16 通过。

**步骤**：
1. 点击 "Export report" 按钮

**验证**：
- [ ] 按钮文本变为 "Building…"
- [ ] 导出完成后浏览器触发文件下载
- [ ] 下载的文件名匹配 `codeg-pk-<round-id>.html`
- [ ] 打开 HTML 文件，验证包含任务文本、选手信息、diff 内容
- [ ] 如果有裁判结果，报告包含裁判评分

### TC-29：控制变量 PK — 同 agent 多 slot（数据层）

**前置**：已安装至少 1 个 agent。

**说明**：当前 UI 不支持直接在 launcher 里添加同一 agent 两次。
此用例通过 API 直接验证数据层。

**步骤**：
1. 通过 HTTP API 创建一个带重复 agent 的 round：

```bash
curl -X POST http://127.0.0.1:3080/pk_round_create \
  -H "Authorization: Bearer test-token-123" \
  -H "Content-Type: application/json" \
  -d '{
    "folder_id": 1,
    "task": "test control variable",
    "config": {
      "agents": [
        {"agent": "claude_code", "label": "Sonnet"},
        {"agent": "claude_code", "label": "Opus"}
      ],
      "permission_mode": "default",
      "bare_mode": false,
      "effort": "default"
    }
  }'
```

**验证**：
- [ ] API 返回 200，round 创建成功
- [ ] 返回的 config.agents 有 2 个条目，都是 claude_code，label 分别为 "Sonnet" 和 "Opus"

**步骤**：
2. 通过 API 列出 rounds：

```bash
curl -X POST http://127.0.0.1:3080/pk_round_list \
  -H "Authorization: Bearer test-token-123" \
  -H "Content-Type: application/json" \
  -d '{"folder_id": null}'
```

**验证**：
- [ ] 返回列表包含刚创建的 round
- [ ] config.agents 正确反序列化为 2 个 labeled 条目

**步骤**：
3. 打开 arena（如果有 UI 入口查看此 round）

**验证**：
- [ ] 计分板显示 2 个选手卡片
- [ ] 两个选手都是同一个 agent（相同图标和名称）
- [ ] 选手卡片用 slot 区分（不同的 key）

### TC-30：控制变量 PK — 旧格式兼容

**说明**：验证旧格式（agents 为纯字符串数组）仍能正确反序列化。

**步骤**：
1. 通过 API 创建一个旧格式的 round：

```bash
curl -X POST http://127.0.0.1:3080/pk_round_create \
  -H "Authorization: Bearer test-token-123" \
  -H "Content-Type: application/json" \
  -d '{
    "folder_id": 1,
    "task": "test old format",
    "config": {
      "agents": ["claude_code", "codex"],
      "permission_mode": "default",
      "bare_mode": false,
      "effort": "default"
    }
  }'
```

**验证**：
- [ ] API 返回 200
- [ ] 返回的 config.agents 是 `["claude_code", "codex"]`（旧格式保持不变）
- [ ] 列表 API 也能正确返回

### TC-31：裁判 API — 无裁判的 round

**步骤**：
1. 创建一个不带 judge_agent 的 round（默认）

**验证**：
- [ ] config 里没有 `judge_agent` 字段（`skip_serializing_if = "Option::is_none"`）
- [ ] Arena 不显示裁判面板

### TC-32：裁判 API — 带裁判的 round

**步骤**：
1. 创建一个带 judge_agent 的 round：

```bash
curl -X POST http://127.0.0.1:3080/pk_round_create \
  -H "Authorization: Bearer test-token-123" \
  -H "Content-Type: application/json" \
  -d '{
    "folder_id": 1,
    "task": "test judge",
    "config": {
      "agents": ["claude_code", "codex"],
      "permission_mode": "default",
      "bare_mode": false,
      "effort": "default",
      "judge_agent": "gemini"
    }
  }'
```

**验证**：
- [ ] 返回的 config 包含 `judge_agent: "gemini"`
- [ ] Arena 显示裁判面板（即使状态为 idle，面板也应渲染）

### TC-33：中断恢复

**前置**：有一个运行中的 PK 回合。

**步骤**：
1. 强制重启 server（kill 进程后重新启动）

**验证**：
- [ ] 重启后 arena 中该回合状态变为 "Interrupted by restart"
- [ ] 选手状态变为 "canceled"
- [ ] 不会自动重新启动比赛（drivenRef 机制防止重放）
- [ ] 已完成的回合不受影响

### TC-34：多人多语言 — 中文界面

**前置**：浏览器语言设为 zh-CN。

**步骤**：
1. 打开 PK launcher

**验证**：
- [ ] 对话框标题为 "Agent PK"（英文保持不变，或对应中文翻译）
- [ ] 模板按钮显示中文名称："鹈鹕骑车"、"弹球"、"果冻 Blob"、"贪吃蛇"
- [ ] 裁判标签显示 "裁判(可选)"
- [ ] "No judge" 按钮显示 "无裁判"

### TC-35：多人多语言 — 日文界面

**前置**：浏览器语言设为 ja。

**步骤**：
1. 打开 PK launcher

**验证**：
- [ ] 模板按钮显示日文名称："ペリカン"、"ボール"、"ゼリー"、"スネーク"
- [ ] 裁判标签显示 "審査員(任意)"
- [ ] "No judge" 按钮显示 "審査員なし"

### TC-36：边界 — 最大选手数

**步骤**：
1. 选 8 个 agent（如果安装了 8 个）

**验证**：
- [ ] 第 9 个无法选中（toggle 函数在达到 MAX_CONTESTANTS 时忽略新选择）
- [ ] 底部计数显示 "8/8 picked (min 2)"

### TC-37：边界 — 空文件夹

**前置**：打开一个空的 git 仓库（只有 initial commit，无其他文件）。

**步骤**：
1. 打开 PK launcher
2. 点击 "From commit" 按钮

**验证**：
- [ ] 只显示 initial commit
- [ ] 选择后任务文本框填充正常

### TC-38：完整流程回归 — 从模板到评分

**前置**：已安装至少 3 个 agent（2 个选手 + 1 个裁判）。

**步骤**：
1. 打开 PK launcher
2. 选 2 个 agent 作为选手
3. 点击 "🐍 Snake" 模板
4. 权限设为 "bypassPermissions"
5. effort 设为 "High"
6. 选第 3 个 agent 作为裁判
7. 点击 "Start match"
8. 在 arena 的 ready 态点击 "Start match" 启动比赛
9. 等待所有选手完成
10. 等待裁判完成评分
11. 切换到 Diff tab 查看 diff
12. 点击 "Share" 导出截图
13. 点击 "Export report" 导出报告

**验证**：
- [ ] 每一步都按预期执行
- [ ] 选手完成后裁判自动启动
- [ ] 裁判评分显示在计分板下方
- [ ] Diff tab 显示每个选手的代码变更
- [ ] 截图下载成功
- [ ] HTML 报告下载成功且内容完整

### TC-39：状态文案一致性

**步骤**：
1. 在 launcher 和 arena 中检查所有可见文本

**验证**：
- [ ] Launcher 的按钮文案与 i18n 消息文件一致
- [ ] Arena 的状态文案（Ready/Live/Finished/Canceled/Interrupted）与 i18n 一致
- [ ] 计分板的状态文案（preparing/connecting/running/done/error/canceled）与 i18n 一致
- [ ] 裁判面板的文案（Judge Verdict/Evaluating/Judge failed）与 i18n 一致

### TC-40：Launcher 复赛预填

**前置**：之前成功启动过一次 PK。

**步骤**：
1. 再次打开 PK launcher

**验证**：
- [ ] 上次的选手选择被预填（如果 agent 仍可用）
- [ ] 上次的任务文本被预填
- [ ] 上次的权限模式被预填
- [ ] 上次的 effort 被预填
- [ ] 上次的裸机模式被预填
- [ ] 上次的裁判选择被预填

---

## API 端点参考

| 端点 | 方法 | 说明 |
|------|------|------|
| `/pk_round_list` | POST | 列出所有 round（可按 folder_id 过滤） |
| `/pk_round_get` | POST | 获取单个 round |
| `/pk_round_create` | POST | 创建 round |
| `/pk_round_update_status` | POST | 更新 round 状态 |
| `/pk_round_delete` | POST | 删除 round（软删除） |
| `/git_log` | POST | 获取 git 提交历史 |
| `/git_branch` | POST | 获取当前分支 |
| `/git_worktree_add` | POST | 创建 worktree |
| `/git_remove_worktree` | POST | 移除 worktree |
| `/git_diff` | POST | 获取 diff |
| `/git_diff_with_branch` | POST | 获取对基准分支的 diff |
| `/git_init` | POST | 初始化 git 仓库 |

所有请求需要 `Authorization: Bearer <token>` 头。

请求体格式为 JSON，参数名用 snake_case。
