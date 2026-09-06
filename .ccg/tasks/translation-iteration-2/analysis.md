# translation-iteration-2 — Phase 2 三方分析综合

> 来源：backend analyzer、frontend/UX analyzer、P6 专项 analyzer（均为独立上下文只读分析）
> 主控裁决记录在下，原始报告见各 agent 输出（要点已全部吸收进本文）。

## 一、根因结论（P2 长思考不翻译）

**推翻初始假设**：「折叠后 IntersectionObserver 不触发」不成立——ref div 是 trigger 兄弟、折叠后仍挂载（零面积元素在 rootMargin 内仍 intersect），virtua bufferSize 800 使 observer 几乎必然立即命中。

真实根因（按可能性，多因叠加）：
1. **单 chunk 撞超时**：8000 字符/chunk × 慢端点（30-130s/chunk）> 后端 read timeout 60s → 重试耗尽 → `translate_batch` 顺序执行 `?` 短路 → 整批失败、零缓存、按钮永不出现。
2. **Web 模式 transport 60s 超时**：`web-transport.ts:19` WEB_CALL_TIMEOUT_MS=60_000，长批量顺序执行必超；`translateTexts` 未传 `timeoutMs`。
3. **CJK 确定性命中字节上限**：`MAX_PARSE_BYTES=128KB`——5 万中文字符 ≈150KB → `splitForTranslation` 直接 null，零请求。后端无镜像守卫（悬空缺口）。
4. **占位符全量否决放大**：代码密集长思考任一 chunk 丢占位符 → 整体回退原文。

**修复组合（已裁决采纳）**：
- 后端 `translate_batch` 并行化（`futures::join_all` + 现有 Semaphore(2)+pace，`futures` 已在依赖）
- `translate_one` 按输入大小缩放 per-request 超时（仅 >2000 字符启用缩放；test_connection 保持快速失败）
- 前端 `MAX_TRANSLATION_CHARS` 8000→3000；`MAX_PARSE_BYTES` 128KB→256KB
- `translateTexts` 传 `timeoutMs`（300s）
- 后端 `translate_with_cache` 入口加单文本长度守卫（20k 字符，镜像前端上限，同时护住 P3 划词路径）
- observer sentinel 加固（移出折叠依赖区）并入 P1 的 ReasoningPart 重构

## 二、P4 Base URL 归一化（已裁决：保存时归一化）

`normalize_base_url()` 在 validate/save 内执行、存归一化值（单一事实源、provider_id 稳定）：
- trim；无 scheme 补 https://（localhost/127.0.0.1/[::1]/*.local/私网段补 http://，兼容 ollama/llama.cpp）
- 显式 scheme 仅 http/https，其余可区分报错；`reqwest::Url::parse`（re-export，零新依赖）校验 host
- 丢 query/fragment；去尾斜杠；`/chat/completions` 后缀剥离得 base；其余路径（/v1、/api/v1、/openai）原样保留
- 派生函数保留兜底归一化（存量值 load 后仍正确路由）；归一化后再过 MAX_BASE_URL_LEN
- 一次性成本：存量 provider_id 变化 → 旧缓存 miss 一次，可接受

## 三、P5 模型列表（已裁决：后端四件套 + 前端 datalist）

- 后端 `translation_list_models`：GET {base}/models（models_url() 与 chat_completions_url() 对偶推导）；bearer 鉴权；per-request 10s 超时；解析容错三形状（`{data:[{id}]}`/`{models:[{id|name}]}`/顶层数组），上限 500 条、1MB 响应上限；错误分类 401→authentication_failed、404→configuration_invalid（文案引导回手输）；mask 回填抽共用 helper `resolve_candidate_settings`（注意：不强制 model 非空——拉列表时 model 正是待填项，显式校验 base_url/api_key 非空即可）
- 前端沿用 `kimi-code-config-panel.tsx` 先例：「获取模型」按钮 + `<datalist>` 联想（可选可输）；probe 失效机制（baseUrl/apiKey 变更即丢弃列表）；三态 loading/toast 错误/内联空列表 hint；apiKey 掩码透传（后端回填，同 test_connection 契约）
- `api.ts` 加 `listTranslationModels()`；`types.ts` 返回 `string[]` 无需新类型

## 四、P1 按钮移位（已裁决：flex 兄弟行 + icon-sm）

- ReasoningPart：`<Reasoning className="group">` 内包 `<div className="flex items-center gap-1">`，`ReasoningTrigger className="min-w-0 flex-1"` 与 TranslationToggle 成 flex 兄弟；toggle 去 absolute。Radix Collapsible context 驱动，trigger 不必是直接 DOM 子节点。button 嵌 button（字面"紧邻 chevron"）非法，行右端为合法等价物。
- TextPart：保留 absolute right-0 top-0，改带背景 pill（`rounded-full border bg-popover/90 shadow-xs px-1 backdrop-blur`）——不推翻「不留占位行」既有决策，hover 显形时遮挡有清晰边界。
- 尺寸：icon-xs(24px)→icon-sm(32px)，命中区翻倍，零 i18n 成本；hover-reveal/focus-visible 保留。

## 五、P3 划词翻译（已裁决：扩展 SelectionActionBubble，in-tree 非 portal）

- **需求更正**：requirements.md 原写「portal」与 bubble 的 in-tree 刻意决策相悖（隐藏 tab 靠 visibility:hidden 继承自动隐藏）——采纳 in-tree。
- bubble 的 `asking: boolean` 泛化为 `mode: "actions"|"asking"|"translating"`（translatingRef 同步副本，`mode !== "actions"` 单一谓词替换三处 askingRef 读取——selectionchange/frame loop/pointer handlers，漏改会复现"卡片悬在旧坐标"历史 regression）
- 新 prop `onTranslate?: (text) => Promise<string|null>`，宿主 message-list-view 注入（包 `translateTexts([text], locale)`）；选区纯文本直接送，不走 mask/split 管线
- 译文卡片在气泡内联展开（复用 asking 态的 re-clamp/dismiss/冻结机制）；>2000 字符截断且可见提示；失败内联文案（不 toast——浮层还在）
- 门控：导出 `useTranslationEnabled()`（包装既有 settings snapshot），翻译关闭时不传 handler→按钮自动缺席
- 只读表面（sub-agent dialog）也启用（翻译不依赖 composer）
- i18n：`Folder.chat.messageList` 下 ~5 新键 ×10 语言

## 六、P6 实时思考翻译（已裁决：段落级增量）

**关键前提（专项核实）**：`maskLiteralSpans` 按 match 顺序编号；流式中未闭合围栏闭合后会让全文重 mask 时占位符回跳重编号 → 整块重翻每 tick 全量缓存 miss（模型开销 5-10 遍全文）不可行。**段级独立 mask 天然稳定**（段落被 `\n\n` 固定后不再变，段内编号从 0 起，restore 段级各做各的）。

- 数据流事实：thinking delta 以 ~16ms 批次 flush（非 token 级），ReasoningPart 每 flush 重渲染
- 策略：只翻「已稳定」前缀（最后一段不翻）；`splitStableParagraphs(text)` 返回 `{paragraphs, separators, stableCount}`，含围栏奇偶扫描（奇数→该围栏起至文末全视为不稳定尾段）；分隔符按 match index 保留回填
- 节流：距上次请求 ≥2500ms 或稳定前缀新增 ≥300 字符（先到触发）；inflight 不取消（内容寻址，旧结果照常并入段落 Map）
- 展示：已翻段拼接 + 尾段原文混排；失败段显示原文 + `consecutiveFailures≥3` 暂停增量至 settled（防风暴）
- settled 收敛：立即 flush 剩余未翻段（无节流），复用增量结果、**不再发整块请求**；P6 关闭时回一期整块路径
- 开关：复用 `translateThinking`（零 DTO/i18n/设置页改动）；后端零新逻辑（段 <3000 字符天然在 60s 内）
- 渲染成本：译文更新 ≤1 次/2.5s，远低于 delta 频率；virtua 扰动与流式原生长高同类——可接受
- 数学：30k 字符/2 分钟典型场景 ≈40 段请求、0.33 req/s，远低于闸吞吐（~5 req/s）

## 七、跨任务冲突裁决

1. **P2×P6**：`MAX_TRANSLATION_CHARS` 3000 同时作用于两条路径（TextPart chunk 路径、P6 段落路径——段落通常 <3000 不再二次切分，超长的段仍会被 splitForTranslation 兜底切分，兼容）。
2. **P1×P6×observer 同文件**（content-parts-renderer.tsx ReasoningPart）：合并为一个实施任务一次改完，避免两次冲突编辑。
3. **P2 后端×P5 后端同文件**（client.rs）：并行化+超时缩放与 list_models 分属不同函数，同 agent 顺序做。
4. **P3×P6 共用后端守卫**：20k 字符入口守卫同时护住划词（前端忘截断时）与畸形输入。
5. **i18n 汇总**：P3 ~5 键 + P5 4 键 + P4 改写 1 键 + P1 零键（icon-sm）+ P6 零键（复用开关）→ 一次补齐 10 语言，messages.test.ts 键集门槛。

## 八、验收矩阵（Phase 3 计划须覆盖）

- 50k ASCII / 50k CJK / 高密度代码段三组思考样本：折叠态可翻译、按钮出现、耗时 <2 分钟
- 流式思考实时跟随：请求频率 ≤0.4 req/s、settled 收敛无整块重发、关闭开关零增量请求
- TextPart 流式零请求不变量回归测试（P-5 保持）
- P4 归一化表测（无 scheme/localhost/尾斜杠//v1//chat/completions/query/fragment/ftp 拒绝）
- P5 三形状解析 + 401/404 可区分 + datalist 失效机制
- P3 截断可见性 + mode 三态互斥 + 冻结语义三处一致
- cargo test/clippy（两 feature 组合）+ pnpm test/eslint（新增子集）全绿
