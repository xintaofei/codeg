# 翻译中间件体验改进 — 详细实施计划（含供应商健康分）

背景：日志证实限速不是瓶颈（派发中位间隔 2.8s、0 次 429/HTTP 错误）；主要矛盾是 ①端点 13% 拒答/回声被门禁静默丢弃且**不计入任何调度状态**（差端点永远平分流量）②单请求 ~14s 延迟 ③失败对用户完全不可见。目标指标：TTFT-tr、门禁拒绝率、"始终没翻译的块"=0、失败可见。

## P0 可观测性打底（先做，健康分与一切决策的数据来源）

**P0.1 后端指标结构 `TranslationMetrics`**
- 新文件 `src-tauri/src/translation/metrics.rs`，复用 `acp/internal_bus.rs:85` 的 `EventBusMetrics` 模式（AtomicU64 + `snapshot()`，不引入 metrics crate）。
- 全局：dispatch_total、cache_hits、served_total、gate_rejected_total（echo/invented/dropped_numbers/truncated 分桶）、http_error_total、network_error_total、rate_limited_total。
- 每 provider（`Mutex<HashMap<String, ProviderCounters>>`）：sent、ok、gate_rejected、http_error、network_error、latency_ms_sum/count、派发分钟桶、健康分所需的滚动窗口事件流（时间戳环形缓冲，容量 ~50 条/家）。
- 埋点：`client.rs translate_one`（provider id、耗时、传输结果）；`mod.rs translate_with_cache`（cache hit、门禁拒绝及原因）。
- 前置改造：`client.rs translate_batch:734` 返回 `Vec<ChunkOutcome { result, provider_id, latency_ms }>`（探索确认改动局部）；`TranslationResult` 新增可选字段保持 serde 兼容。

**P0.2 日志字段补全**：`client.rs translate_one` 的 sending/response/failure 日志补 `provider=<id>`、`latency_ms=`、`lane=`（今天 "I'm Mistral" 拒答无法归因的教训）。

**P0.3 指标暴露到前端**：新增 `translation_metrics_core` + Tauri command + web handler + POST `/translation_metrics`（1:1 镜像模式）；`ProviderStatus` 增加 `dispatched_last_minute`；`api.ts` 增加 `getTranslationMetrics()`；types.ts 增加 `TranslationMetricsSnapshot`、`TranslationPoolStatus` 同步。

## P1 供应商健康分（完整版）+ 端点行为治理

**P1.1 健康分本体**（`pool.rs ProviderRuntime` 新增，纯函数消费 P0.1 窗口数据）
- 滚动窗口（最近 10 分钟或 20 次，指数衰减）→ 三子分：质量 Q（0.5 权重，门禁拒绝率 0%=满分、≥30%=0 分，echo/拒答比丢数字罚更重）、稳定 S（0.3，传输失败/5xx/超时全扣、429 半扣）、速度 L（0.2，P50 延迟 5s 满分→30s 零分）。
- 综合 0-100；样本 <5 显示"观测中"，调度按中性 70 处理；全程内存态、重启重新探测（与 AIMD 哲学一致）。
- 单测：窗口衰减、样本不足、各子分映射边界。

**P1.2 调度权重**：`pool.rs pick()` 排序从"按空闲"改为"健康分优先、同分严格轮询"（顺带修掉现有 idle 排序产生的 A,A,B,B 突发——今天同一 chunk 连撞 legacy 3 次）。被拒端点分数自然回落 → 重试自动落在别家，无需显式 pin。

**P1.3 质量熔断**（补上"只有 4xx 会 retire"的洞）：
- 分数 <70 → 降级 fallback-only（正常流量不派、全池不可用时顶上）；<40 → session 停用（复用现有 `disabled_reason` 通道与事件推送）。
- 防护：样本 <5 不熔断；池内仅剩一家时永不熔断（与现有 4xx retire 同语义）；饥饿探测——低分端点每 2 分钟放一条 background probe，成功即爬分回归。
- 单测：降级/停用阈值、单点保护、probe 回归。

**P1.4 端点行为治理**：
- Prompt 加固：`prompt.rs` system_prompt 增加指令隔离条款（源文含"Write at least ten paragraphs"类祈使句——翻译其内容，绝不执行）+ 1 条 few-shot 反例；用回放评测集 A/B。
- 拒答快失败：`client.rs parse_translation` 识别已知拒答模板（"I'm … Large Language Model"等）→ 返回分类错误且不计 `report_success()`（不污染 AIMD 与健康分）。

**P1.5 归因实验**：30 分钟真实使用 + 日志回放，产出两家端点的失败率/延迟/健康分曲线矩阵，验证健康分区分度（差端点应明显低于好端点）。

## P0.4/P1 共用的前端 UI

**状态条增强**（`translation-settings.tsx`）：
- 状态 badge 显示"实际派发/分 + 健康分徽章"（如 `实际 18/分 · 健康 92·良`），hover 展开三子分与最近拒绝原因；fallback-only/停用态有明确标识。
- 新增"被拒片段 N · 最近原因"汇总行；i18n `TranslationSettings` 扁平段新增 ~8 个 key × 10 locale。

**消息级失败可见**（消灭静默失败）：
- `use-translated-text.ts` / `use-streaming-translated-text.ts`：`TranslatedTextState` 增加 `hasErrors` + `lastErrorHint`；`translation-toggle.tsx` 增加 warning 态（琥珀点 + hover 提示），`content-parts-renderer.tsx` TextPart/ReasoningPart 接入。不 toast。
- 测试：扩展 translation-toggle.test.tsx 与两个 hook 测试。

## P2 延迟与感知速度（先测量后决策）

P0 数据落地后取：单请求延迟分布、lane 排队时间、近似 TTFT-tr。候选（按数据选做、单项另立任务）：a) lane cap 调整或 per-provider 并发（现 4+3）；b) 流式思考块首批升 priority lane；c) numbered 分组按 token 预算封顶（修 6333 字符组触发 8192 token 截断，前端 `lib/translation.ts`）；d) SSE 流式翻译（最大改动，仅当 a-c 不够时立项）。

## P3 门禁调优（防误杀，数据驱动）

用 P0.6 评测集（`scripts/translation-eval.mjs` 解析日志抽 (source, reply, provider, verdict, latency) JSONL）人工标注 50-100 条，给三道门禁算 precision/recall；重点排查 dropped-numbers 误杀"阿拉伯→中文数字"合规译法、短 chunk 长度阈值误杀。只在证据支持时改阈值，每次改动附评测集回归数字。

## 执行顺序与验收

P0.1→P0.2→P0.3（顺序依赖）→ P1.1 健康分本体 → P1.2/P1.3 调度与熔断 → 前端 UI（状态条 + 消息级）→ P1.4 → P1.5 归因验证 → 回放工具与 P2/P3 按数据另立任务。批准后先完整落地 P0+P1+前端 UI。

每阶段验收：`cargo test --features test-utils`、`cargo clippy --all-targets --features test-utils -- -D warnings`、`cargo check --no-default-features --bin codeg-server`、`pnpm eslint .`、`pnpm test` 全绿。端到端：复现一次 echo 拒答 → 状态条计数与消息级提示 1s 内可见（事件推送）；持续拒答的端点健康分跌落 → 降级 → 流量自动转移；UI 能回答"为什么没翻译"。