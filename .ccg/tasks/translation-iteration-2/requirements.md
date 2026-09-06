# 翻译功能二轮迭代 — 结构化需求

> 任务：translation-iteration-2 · 分支：feat/translation-middleware
> 基线：一期计划 `D:/CLIGUI/work/.doc/20260831-codeg-翻译中间件-计划.md`（D-1..D-16）
> 生成：2026-09-02（CCG Phase 1 自增强）

## 目标

修复一期翻译功能的两处体验/正确性缺陷，并新增三项能力：

- **P1（修复）** 翻译切换按钮位置不当：现渲染于内容区 `absolute right-0 top-4`（ReasoningPart）/ `top-0`（TextPart），遮挡思考正文首行。期望移到「思考」折叠触发行的右侧（紧邻折叠 chevron），且按钮尺寸加大。
- **P2（修复）** 思考内容过长时翻译按钮不出现、翻译不发生。根因候选（待 Phase 2 定位）：
  1. `useNearViewport` 的 IntersectionObserver 挂在 `CollapsibleContent` 内的 div 上；流式结束后 Reasoning 自动折叠（AUTO_CLOSE_DELAY 1s），隐藏元素 observer 永不触发 → `shouldLoad` 恒 false → 不发请求、无按钮。
  2. 长文本被 `splitForTranslation` 切成多 chunk，后端 `translate_batch` 顺序 await，总耗时线性增长（每 chunk read timeout 60s）；任一 chunk 失败整体返回 null（`results.length !== chunks.length` / catch）。
  3. 长文本占位符（`\0CBLK<n>\0`）数量多，小模型丢/乱序占位符概率上升 → `hasSameTranslationPlaceholders` 失败 → 静默回退原文。
  4. virtua 虚拟列表下超高消息行的可见性判定。
- **P3（新增）** 划词翻译：在消息渲染内容中选中文字，选区右上角浮现翻译按钮，点击后展示译文（展示形态待定：气泡/就地替换）。
- **P4（增强）** 设置页 Base URL 兼容更多格式：现状 `validate` 强制 `http(s)://` 前缀（无 scheme 直接拒绝），`chat_completions_url()` 已处理裸 host / `/v1` / `/chat/completions` 三态。期望更宽容的归一化（自动补 scheme、去 query、各种后缀变体）。
- **P5（新增）** 模型列表获取：填好 Base URL + API key 后，可调用 OpenAI 兼容 `GET {base}/models` 拉取模型列表供选择（设置页模型字段从纯手输升级为可选可输）。
- **P6（新增，2026-09-02 用户追加）** 思考内容实时翻译：流式输出期间 thinking 增量跟随翻译。推翻一期 D-2「不翻译流式文本」决策（仅限 thinking 范围；正文维持 settled 后翻译）。

## 范围

- 前端：`content-parts-renderer.tsx`（TextPart/ReasoningPart）、`translation-toggle.tsx`、`use-near-viewport.ts`、`use-translated-text.ts`、`translation-settings.tsx`、新划词组件、`api.ts`、`types.ts`、i18n 10 语言
- 后端：`translation/settings.rs`（URL 归一化）、`translation/client.rs`（/models 端点、超时/批量语义、P6 实时翻译的节流与增量端点）、`commands/translation.rs`、`web/handlers/translation.rs`、`web/router.rs`
- 前端新增面：P6 需扩展 `use-translated-text.ts` 或新建流式增量翻译 hook；P3 扩展 `selection-action-bubble.tsx`
- 不动：一期已定的缓存结构（LRU+磁盘）、遮罩机制、并发闸语义（除非 P2 定位要求调整）

## 技术约束

- 一期 D-1..D-16 决策仍然有效；**例外：P6 推翻 D-2 的 thinking 子集**（正文 prose 仍 settled 后翻译，D-2/R1 对 TextPart 不变）；P2 修复不得引入正文流式期请求（P-5 零请求不变量按 P6 范围重新划界：TextPart 零请求不变，ReasoningPart 允许节流后的增量请求）
- 前后端模型镜像约束：`models`/DTO 改动同步 `src/lib/types.ts`
- 新增 HTTP 端点走 `_core` + Tauri command + web handler + router 四件套
- i18n 硬门槛：10 语言键集必须完全相等（`messages.test.ts`）
- 静态导出约束：无动态路由；划词浮层用 portal，不引入新依赖优先
- 失败静默回退原文的语义（D-15）保持；设置页显式报错通道保持

## 验收标准

- P1：思考块翻译按钮位于「思考 ^」同一行的右侧，不遮挡任何正文；按钮命中区域明显大于现状（icon-xs）；TextPart 按钮同步审视位置
- P2：≥50k 字符的思考内容，折叠状态下也能自动完成翻译并出现切换按钮；展开/折叠切换不重复请求（缓存命中）；流式期间仍零请求
- P3：选中消息内文本 → 选区右上浮现按钮 → 点击显示译文；点击空白/滚动后浮层消失；用户消息与 assistant 消息均可划词；翻译走同一后端与缓存
- P4：`api.host.com`、`https://host`、`https://host/v1`、`https://host/v1/`、`https://host/v1/chat/completions`、带 query 的 URL 全部保存成功且路由到正确 endpoint
- P5：填好 URL+key 后点「获取模型」→ 列表渲染可选；401/404/网络失败给出可区分错误；获取的模型名写回 model 字段
- P6：开启「翻译思考」后，长思考流式输出期间译文跟随滚动更新（增量、按段落节流，非 token 级请求风暴）；原文渲染不被翻译阻塞；流式结束后收敛为完整译文；关闭 P6 时行为回退一期语义
- 全绿：`pnpm test`、`pnpm eslint .`（新增文件子集）、`cargo test --features test-utils`、`cargo clippy --all-targets --features test-utils -- -D warnings`、`cargo check --no-default-features --bin codeg-server`

## 需求完整性评分

目标明确 3/3 · 预期结果 3/3 · 边界范围 2/2 · 约束条件 2/2 = **10/10**（≥7，进入 Phase 2）

## 待设计决策（Phase 2/3 给出方案后由用户审批）

1. P3 译文展示形态：就地替换选中区 vs 浮动气泡卡片（已有先例 `selection-action-bubble.tsx`，倾向扩展该组件）
2. P1 按钮放进 CollapsibleTrigger 内部（嵌套交互元素 a11y 风险）vs trigger 行外层 flex 兄弟
3. P2 若根因含批量耗时：是否并行化 chunk 请求（并发闸 2 已限流）或部分成功策略
4. P6 增量翻译策略：段落级稳定前缀增量翻译（推荐方向）vs 整块重翻 vs 后端流式代理；节流参数（最小间隔/最小新增字数）
5. P6 与一期 settled 整块翻译的衔接：流式结束后是否重发整块请求（内容哈希缓存下增量段已命中，成本可控）；实时开关是复用 translateThinking 还是新增独立设置
