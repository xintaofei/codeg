# translation-iteration-2 — 实施计划（Phase 3 综合稿）

> 综合自三个 architect 分域计划（backend / frontend / P6），主控冲突裁决已内嵌。
> 基线约束：analysis.md（Phase 2 裁决）+ 一期计划 D-1..D-16。
> 复杂度：XL · 分支 feat/translation-middleware · 预估变更 ~20 文件

## 0. 主控冲突裁决记录

| # | 冲突 | 裁决 |
|---|------|------|
| C1 | `listTranslationModels` 签名：backend 计划用完整 `TranslationSettings`，frontend 计划用 `Pick<...>` | **完整 `TranslationSettings`**（transport args `{settings}` 形态一致；后端只读 baseUrl/apiKey + mask 回填） |
| C2 | P4 placeholder：backend I3 说维持 `https://api.example.com/v1`，frontend Step 7 说改 `api.example.com` | **改 `api.example.com`**（宽容输入正是 P4 的功能本体，裸 host 示例最直观传达；保存后回显归一化值不受影响） |
| C3 | `content-parts-renderer.tsx` ReasoningPart：P1（flex 行+sentinel）与 P6（hook 接线）同函数 | **合并为单一任务 FE-2 一次改完**，JSX 骨架见 §FE-2 |
| C4 | `translation.ts`/`translation.test.ts`：backend Step 6a/6d（常量+测试适配）与 P6（splitStableUnits+新测试）同文件 | **全部归 FE-1**（P6 agent）执行，backend 计划 6a/6d 内容并入 |
| C5 | `use-translated-text.ts`：P3 要加 `useTranslationEnabled`，P6 要 export 三符号 + 加 `disabled` | **全部归 FE-1**；FE-3 的 message-list-view 接线移到 FE-2（Layer 2） |

## 1. 文件归属矩阵与分层

```
Layer 1（五路并行，文件零交集）：
  BE-1  Rust 后端：src-tauri/src/translation/{settings,client,mod}.rs
        src-tauri/src/commands/translation.rs
        src-tauri/src/web/handlers/translation.rs
        src-tauri/src/web/router.rs · src-tauri/src/lib.rs（+各文件内 Rust 测试）
  FE-1  P6 核心 + 共享 hook：src/lib/translation.ts（常量 3000/256KB + splitStableUnits）
        src/lib/translation.test.ts · src/hooks/use-translated-text.ts（exports+disabled+useTranslationEnabled）
        src/hooks/use-translated-text.test.ts · src/hooks/use-streaming-translated-text.ts（新）
        src/hooks/use-streaming-translated-text.test.tsx（新）
  FE-3  P3 气泡：src/components/message/selection-action-bubble.tsx（+test.tsx）
  FE-4  P5+P4 UI + 格式下拉：src/lib/api.ts（listTranslationModels + translateTexts timeoutMs）
        src/lib/types.ts（TranslationSettings +apiFormat）
        src/components/settings/translation-settings.tsx（+test.tsx）
  FE-5  i18n：src/i18n/messages/*.json ×10（键清单 §FE-5，已定稿可并行）
Layer 2（依赖 FE-1+FE-3）：
  FE-2  渲染层合并改造：src/components/message/content-parts-renderer.tsx
        src/components/message/translation-toggle.tsx（+test.tsx）
        src/components/ai-elements/reasoning.test.tsx · message-list-view.tsx（onTranslate 接线）
Layer 3：全量验证 + 修复轮
```

---

## 2. BE-1 — Rust 后端（串行链 Step 1→2→3→4→5）

### B1. `translation/settings.rs` — P4 归一化（最先，其余依赖）

新增自由函数 `pub fn normalize_base_url(raw: &str) -> Result<String, AppCommandError>`，按序：
1. `trim()`；空 → `Ok("")`（草稿路径，保持现 validate 语义 settings.rs:133）
2. 长度闸：`chars().count() > MAX_BASE_URL_LEN` → Err "too long"（保持现错误优先级 :111-115）
3. scheme：不含 `://` → 取主机段（首个 `/`或`:` 前）小写，命中 `localhost`/`127.0.0.1`/`::1`/`*.local`/`10.*`/`192.168.*`/`172.16-31.*` → 补 `http://`，否则补 `https://`；含 `://` → scheme 小写后必须 ∈ {http,https}，否则 `configuration_invalid("Translation base URL scheme must be http:// or https://").with_detail(scheme)`（判据先例 network/proxy.rs:30-76）
4. `reqwest::Url::parse`（re-export，零新依赖，先例 chat_channel/webhook.rs:107）→ 失败 `invalid_input("...not a valid URL")`
5. `host_str()` 空/None → `configuration_invalid("...must include a host")`
6. `set_query(None)` + `set_fragment(None)`
7. path：大小写不敏感剥 `/chat/completions` 后缀 → `trim_end_matches('/')` → `set_path`
8. `to_string()` 后 while 去尾斜杠（`https://host/` → `https://host`）

`validate`（:100-170）：`base_url` 行改 `normalize_base_url(&settings.base_url)?`；删原 scheme 块（:133-140）与原长度块（:111-115）。其余不动。

端点派生重构（:73-82）：
```rust
fn normalized_base(&self) -> String { normalize_base_url(&self.base_url).unwrap_or_else(|_| self.base_url.trim().trim_end_matches('/').to_string()) }
fn endpoint_url(&self, suffix: &str) -> String {
    let mut base = self.normalized_base();
    if let Some(s) = base.strip_suffix("/chat/completions") { base = s.to_string(); } // 存量兜底
    let has_path = base.split_once("://").and_then(|(_, r)| r.find('/')).is_some();
    if has_path { format!("{base}/{suffix}") } else { format!("{base}/v1/{suffix}") }
}
pub fn chat_completions_url(&self) -> String { self.endpoint_url("chat/completions") }
pub fn models_url(&self) -> String { self.endpoint_url("models") }
```
`provider_id()` 不改（输入已归一化）。

### B2. `translation/client.rs` — P2 并行化 + 超时缩放 + list_models

- import `futures::future::join_all`（futures 已在 Cargo.toml:81）
- 新常量（READ_TIMEOUT :25 后）：`SCALING_TIMEOUT_THRESHOLD_CHARS=2000`、`SCALING_TIMEOUT_BASE=30s`、`SCALING_TIMEOUT_PER_CHAR=20ms`、`MODELS_TIMEOUT=10s`、`MAX_MODEL_LIST=500`
- `fn request_timeout(text_chars: usize) -> Duration`：≤2000 → READ_TIMEOUT；否则 `30s + 20ms×chars`（3000→90s）
- `translate_one`（:206-210）请求链追加 `.timeout(request_timeout(text.chars().count()))`；RETRY_BACKOFF 不动（单 chunk 最坏 3×90s+4s）
- `translate_batch`（:285-295）改：`join_all(texts.iter().map(|t| translate_one(...))).await.into_iter().collect()`——保序、跑完全部、传播第一个错误，整批失败语义不变；gate/pace 共用，D-8 不变。**裁决：join_all，不用 try_join_all**
- 新增 `pub async fn list_models(&settings) -> Result<Vec<String>, AppCommandError>`：GET `settings.models_url()` + bearer + `.timeout(MODELS_TIMEOUT)`；非 2xx：404 → `configuration_invalid("This endpoint does not expose a model list — enter the model name manually")`（detail 截 500），其余复用 `classify()`；body >1MB（MAX_RESPONSE_BYTES）报错；→ `parse_models`
- `fn parse_models(bytes) -> Result<Vec<String>, _>`：`serde_json::Value`；entries = `data` 数组 → `models` 数组 → 顶层数组，全无 → Err("...no model list")；每项 `id`→`name` 回退、trim、去空、去重、截 500；空数组合法

### B3. `translation/mod.rs` — 入口守卫（独立，可并行）

`const MAX_SINGLE_TEXT_CHARS: usize = 20_000;`；`translate_with_cache` 在 disabled 检查后、resolve_target_lang 前：任一 text `chars().count()` 超限 → `invalid_input(format!("Translation text is too long ({N} characters; the limit is {MAX_SINGLE_TEXT_CHARS})"))`。守卫先于缓存查询。

### B4. `commands/translation.rs` — P5 core + helper 抽取

- 抽 `fn resolve_candidate_settings(stored, incoming) -> TranslationSettings`（mask→stored.api_key，现 :44-52 逻辑上提）；`translation_test_core` 改调用它，既有测试 :150-213 保持全绿
- `translation_list_models_core(conn, settings)`：load stored → resolve_candidate → `candidate.base_url = normalize_base_url(&candidate.base_url)?` → base 空 → `configuration_missing("Translation base URL is required to list models")`；api_key trim 空 → `configuration_missing("Translation API key is required to list models")` → `client::list_models(&candidate).await`。**刻意不走 validate(enabled:true)**（model 正是待填项）
- Tauri command `translation_list_models(settings, db)` 照 :102-110 模式

### B5. 接线三件

- `web/handlers/translation.rs`：`ListModelsParams { settings }` + handler（照现文件模式，`Extension<Arc<AppState>>`）
- `web/router.rs`：`/translation_test`（:696-699）后插 `.route("/translation_list_models", post(handlers::translation::translation_list_models))`
- `lib.rs`：命令注册表（:1229-1234）`translation_test,` 后插 `translation_list_models,`

### B6. Rust 测试清单（表测名照抄，断言见 backend 计划）

settings.rs：`base_urls_normalize_to_a_canonical_form`（10 例表）、`a_missing_scheme_defaults_to_https_for_public_hosts`、`a_private_host_defaults_to_http`（8 例含 172.32 上界外）、`an_explicit_scheme_wins_over_the_private_host_guess`、`unsupported_schemes_are_rejected_distinctly`、`a_schemeless_url_without_a_host_is_rejected`、`an_empty_base_url_normalizes_to_empty`、`normalize_runs_inside_validate_and_persists`、`saving_after_normalization_keeps_the_mask_roundtrip`、`oversized_base_urls_are_rejected_after_trim`、`models_url_matches_the_chat_completions_shape`（4 例）、`host_and_v1_forms_share_one_provider_id`
client.rs：`small_texts_keep_the_fast_client_timeout`、`large_texts_scale_the_deadline_with_input_size`、`models_parse_from_the_openai_data_shape`、`models_parse_from_the_models_shape`、`models_parse_from_a_bare_array`、`an_empty_model_list_is_ok_not_an_error`、`models_skip_blank_and_non_string_ids`、`models_deduplicate_and_cap_at_500`、`malformed_model_json_is_an_error`、404 文案常量断言
mod.rs：`an_overlong_single_text_is_rejected_before_anything_else`、`a_text_at_the_character_limit_passes_the_guard`
commands：`the_mask_refills_from_stored_and_a_real_key_wins`、`listing_models_requires_a_base_url_and_key`、`listing_models_rejects_an_unusable_url`

---

## 2A. BE-1 增补 — API 格式档案（P4+，用户裁决：方案 A「自动识别 + 端点档案 + 覆盖下拉」，2026-09-02 计划复审确认）

> 目标：Base URL 直接支持 Claude（Anthropic）、OpenAI、Gemini、Ollama 四家格式。
> 设计核心：只有 anthropic 一档新增原生序列化（api.anthropic.com 不提供 OpenAI 兼容端点）；
> gemini/ollama 走各家官方 OpenAI 兼容端点（`/v1beta/openai`、`/v1`），复用现有 chat 管线，序列化零改动。

### A1. `translation/settings.rs` — 新字段与档案推导

- `TranslationSettings` 加 `#[serde(default)] pub api_format: String`（`"auto"|"openai"|"anthropic"|"gemini"|"ollama"`；存量 JSON 行反序列化即 "auto"，零迁移）。前端 `types.ts` 同步镜像 `apiFormat: "auto" | "openai" | "anthropic" | "gemini" | "ollama"`
- `enum ApiFormat { Openai, Anthropic, Gemini, Ollama }` + `fn resolve_format(base_url, api_format) -> ApiFormat`：显式值直接映射；"auto" 按 host 判定（大小写不敏感）——host 含 `anthropic` → Anthropic；host 含 `googleapis`/`gemini` → Gemini；端口 11434 或 host 含 `ollama` → Ollama；否则 Openai
- `provider_id()` 改 `format!("{normalized_base}|{model}|{resolved_format}")`（同 base 换格式=换端点，缓存正确隔离）
- `chat_completions_url()` / `models_url()` 按档案派生：
  - Openai：现逻辑（base 无 `/v1` 补 `/v1`）
  - Anthropic：`{base}/v1/messages`、`{base}/v1/models`
  - Gemini：base 归一到 `{origin}/v1beta/openai`（用户路径非已知后缀时保留），再 `+/chat/completions`、`+/models`
  - Ollama：`{base}/v1/chat/completions`、`{base}/v1/models`
- B1 的 normalize_base_url 第 7 步剥后缀清单扩为：`/chat/completions`、`/v1/messages`、`/v1beta/openai`、`/api/chat`、`/api/generate`（大小写不敏感）
- `validate`：`enabled && api_key 空` 仅当 resolve 后 ≠ Ollama 才报错（Ollama 本地无鉴权）；`api_format` 不在五值内 → `configuration_invalid("Unknown translation API format")`

### A2. `translation/client.rs` — anthropic 原生 chat 分支 + 鉴权头分档

- `translate_one` 按 `resolve_format` 分支：
  - Anthropic：POST `chat_completions_url()`，headers `x-api-key: {key}` + `anthropic-version: 2023-06-01`，body `{model, max_tokens, system, messages:[{role:"user",content}]}`；`max_tokens = clamp(chars×2+1024, 4096, 32768)`；解析 `content[]` 中 `type=="text"` 的 text 拼接；`stop_reason=="max_tokens"` → Err（防半截译文入缓存；占位符校验是第二道闸）
  - 其余三家：现 OpenAI 序列化不动；api_key 为空时跳过 bearer 头（Ollama）
- `list_models`：鉴权头同分档（anthropic → x-api-key+version；其余 bearer，空则跳过）。`parse_models` 三形状已覆盖四家返回（anthropic / gemini-compat / ollama-compat 均为 `{data:[{id}]}`），不改
- `classify()` 复用（anthropic 401/404/429 语义同码）

### A3. `commands/translation.rs` — list_models 的 key 豁免

- B4 的「api_key trim 空 → configuration_missing」改为仅当 `resolve_format ≠ Ollama`

### A4. 测试增补（并入 B6 清单）

- settings.rs：`formats_are_detected_from_the_host`（表测：api.anthropic.com、generativelanguage.googleapis.com、localhost:11434、192.168.1.5:11434、api.openai.com、反代域名→openai 兜底）、`an_explicit_format_wins_over_detection`、`an_unknown_format_is_rejected`、`provider_id_changes_with_the_format`、`ollama_may_be_enabled_without_a_key`、`the_four_formats_derive_their_documented_endpoints`（chat+models 各 4 例）、`known_api_suffixes_are_stripped_on_save`
- client.rs：`anthropic_requests_carry_the_versioned_key_headers`、`anthropic_text_blocks_are_joined`、`a_truncated_anthropic_output_is_an_error`、`an_empty_key_sends_no_bearer_header`

### A5. FE-4 / FE-5 联动增补

- `translation-settings.tsx`：baseUrl 行上方加「API format」Select（值 auto/openai/anthropic/gemini/ollama，显示名 Auto/OpenAI/Claude/Gemini/Ollama——品牌名不译）；model placeholder 随档案变化（anthropic→`claude-sonnet-4-5`、gemini→`gemini-2.5-flash`、ollama→`qwen2.5:14b`、默认 `gpt-4o-mini`）；probe 失效判据加 `apiFormat`（格式变更即丢弃模型列表）；FE-4 测试 +2 用例（格式切换→probe 失效；下拉值写回）
- i18n：+2 键（`TranslationSettings.formatLabel`、`formatAuto`）×10 语言
- `types.ts`：`TranslationSettings` 加 `apiFormat` 字段（镜像 A1）

---

## 3. FE-1 — P6 核心 + 共享 hook 层

### F1a. `src/lib/translation.ts`

- `:3` `MAX_TRANSLATION_CHARS = 8000` → `3000`；`:4` `MAX_PARSE_BYTES = 128*1024` → `256*1024`（**注意 composer-copy-text.ts:71 同名常量无关，不许动**）
- 新常量：`STREAM_MIN_INTERVAL_MS=2500`、`STREAM_MIN_NEW_CHARS=300`、`STREAM_FAILURE_PAUSE_LIMIT=3`
- 新增 `splitStableUnits(text): { units: string[]; unitEndOffsets: number[]; tailStart: number }`——单趟 offset 行扫描：
  - 逐行（记行起始 offset），fence 状态 `null | {ch: '`'|'~', len}`：行匹配 `/^ {0,3}(`{3,}|~{3,})\s*$/`，无 fence 开（记 ch/len），有 fence 且 ch 同、横线数 ≥ len 关
  - 分隔符 `/(?:\r?\n){2,}/`（**不可用 `\n{2,}`——CRLF**）：不在 fence 内 → 密封单元（上一密封点至分隔符末的原文切片）；在 fence 内 → 不密封
  - 文末未密封区 → tailStart。恒等式：units+分隔符+尾部逐字节还原原文
  - 跨段完整 fence 整体一个单元；未闭合围栏起至文末永不密封
- 边界用例（全进测试）：空/纯空白；无分隔符；`\n\n\n`；CRLF；`\n \n` 不算分隔符（文档化盲区）；段>3000（下游 split 兜底）；未闭合围栏在首段；围栏跨段含空行；围栏闭合后 prose 继续；`~~~` 与 ``` 互不误关；纯代码篇；重建恒等式

### F1b. `src/hooks/use-translated-text.ts`（纯增量）

- export `useTranslationSettingsSnapshot`（:93）、`translationCacheKey`（:109）、`requestTranslation`（:135）——无逻辑改动
- `UseTranslatedTextParams` 加 `disabled?: boolean`（默认 false）；effect（:214）开头 `if (disabled) return cleanup`；`disabled` 进依赖数组
- 文件尾追加 `export function useTranslationEnabled(): boolean`（包 snapshot，返回 `settings.enabled`——划词不受 translateThinking 门控）

### F1c. `src/hooks/use-streaming-translated-text.ts`（新建）

```ts
export function useStreamingTranslatedText(params: {
  text: string; isStreaming: boolean; shouldLoad: boolean
  uiLocale: string; blockKey: string; enabled: boolean
}): TranslatedTextState  // 与 settled hook 同形
```
- state：`translatedMap: ReadonlyMap<number, string>`（unit 序号→译文，**不存 offset**）+ `remainder: {key, text} | null`
- refs：`requestedUnitsRef`、`lastDispatchAtRef`、`newCharsRef`、`timerRef`、`consecutiveFailuresRef`、`settledFlushedRef`、`wantsOriginalRef`（用户切换偏好，key 每 flush 变化下保持选择）
- dispatch：对 `units[requestedUnits..]` 逐个 `requestTranslation(unit, uiLocale, translationCacheKey({blockKey, text: unit, uiLocale, settings}))`，Promise.all 收敛；任一成功→写 Map+setState、failures=0；整批 null→failures+1；≥3→PAUSED 至 settled
- 展示：连续已译前缀（unit 0 起遇空洞即止）按原文分隔符拼接 + `text.slice(前缀末 offset)` 原文 + remainder 并入末尾；`hasTranslation = map.size>0 || remainder`
- 节流：effect 每 flush 检查 `now-lastDispatch ≥ 2500ms || newChars ≥ 300`（先到触发；≥300 立即 fire，否则装 timer `max(lastDispatch+2500, now)-now`）
- settled 收敛（每 blockKey 一次）：清 timer → R = text.slice(连续前缀末 offset)，非空 → `requestTranslation(R)`（无节流）→ remainder；冷挂载已 settled 旧消息 → 前缀空 → R=全文 → 退化为一期整块路径（行为收敛）
- blockKey 变更 → 全量重置；卸载 → 清 timer，inflight 继续写缓存，setState 由 current 标志拦截
- 状态机（S0 IDLE→S1 THROTTLED→S2 REQUESTING→S3 PAUSED→S4 SETTLING→S5 DONE）转移表照 P6 计划 §StateMachine 实现

### F1d. 测试

- `translation.test.ts`：既有 8000 边界表改 3000 同构（2999/[2999]、3000/[3000]、3001/[3000,1]）；:25-31 段落用例 source 改 `a×2500\n\nb×2000` 期望 `[2502,2000]`；:37 surrogate 期望符号化；+ splitStableUnits 12 边界用例 + 恒等式
- `use-streaming-translated-text.test.tsx`（新）：`vi.useFakeTimers()` + `vi.mock("@/lib/api")` 受控 promise + `primeTranslationSettings` 预置；12 用例：首分隔符前零请求 / 节流合并（2.5s 内 10 flush→恰 1 调用）/ 300 字符先到 / 段级请求内容逐字节 / 段级缓存命中 / 失败退避（3 批全 null→暂停→settle 恰 1 次 R）/ settled 收敛（**全文 30k 串从未出现在任何调用**）/ 混排展示 / P6 off 零调用 / settled hook 抑制（p6 on 无全文请求）/ 卸载清理 / 用户切换持久
- `use-translated-text.test.ts`：`useTranslationEnabled` true/false + prime 即时翻转；`disabled:true` 时 settled 零请求

---

## 4. FE-2 — 渲染层合并改造（Layer 2）

### F2a. `content-parts-renderer.tsx` ReasoningPart（:2929-2975，一次改完）

```tsx
const ReasoningPart = memo(function ReasoningPart({ part, blockKey = "" }) {
  const hasContent = part.content.trim().length > 0
  const expandable = hasContent || part.isStreaming
  const { ref, shouldLoad } = useNearViewport<HTMLDivElement>()
  const uiLocale = useLocale()
  const settings = useTranslationSettingsSnapshot()
  const p6Enabled = settings.enabled && settings.translateThinking
  const streaming = useStreamingTranslatedText({
    text: part.content, isStreaming: part.isStreaming,
    shouldLoad: shouldLoad && expandable, uiLocale, blockKey, enabled: p6Enabled,
  })
  const settled = useTranslatedText({
    text: part.content, isStreaming: part.isStreaming, isUser: false,
    shouldLoad: shouldLoad && expandable, uiLocale, blockKey,
    isThinking: true, disabled: p6Enabled,
  })
  const view = p6Enabled ? streaming : settled
  return (
    <Reasoning isStreaming={part.isStreaming} expandable={expandable} className="group">
      <div className="flex items-center gap-1">
        <ReasoningTrigger className="min-w-0 flex-1" />
        {view.hasTranslation && (
          <TranslationToggle isTranslated={view.isTranslated}
            onShowOriginal={view.showOriginal} onShowTranslation={view.showTranslation}
            className="shrink-0" />
        )}
      </div>
      {expandable && <ReasoningContent>{view.display}</ReasoningContent>}
      {/* P2 sentinel：零面积常挂载，observer 与折叠态解耦（rootMargin 内零面积仍 intersect） */}
      <div ref={ref} aria-hidden className="h-0 w-0 overflow-hidden" />
    </Reasoning>
  )
})
```
删除 :2961 包裹 div 与 :2963-2970 absolute toggle 块；Radix Collapsible context 驱动，trigger 非直接子节点不破坏折叠（reasoning.tsx:146-157）。

### F2b. TextPart（:2286-2295）

toggle className：`"absolute right-0 top-0 rounded-full border bg-popover/90 shadow-xs px-1 backdrop-blur"`（保留 out-of-flow 与 :2291-2292 注释决策）。

### F2c. `translation-toggle.tsx`

:32 `icon-xs`→`icon-sm`；:45 `h-3 w-3`→`size-4`（**显式**：button.tsx:31 icon-sm 不含 svg 尺寸类；不改 button.tsx）。hover/focus 逻辑（:37）不动。

### F2d. `message-list-view.tsx` P3 宿主接线

- imports：`translateTexts`、`useLocale`、`useTranslationEnabled`
- 组件内（:1211 selectionBoxRef 附近）：`handleTranslateSelection = useCallback(async (text) => { try { const r = await translateTexts([text], uiLocale); return r[0]?.text ?? null } catch { return null } }, [uiLocale])`
- :1415-1420 挂载点：`onTranslate={translateEnabled ? handleTranslateSelection : undefined}`
- 无新公开 prop、不改 conversation-detail-panel（主面板/sub-agent/canvas/live transcript 自动获得）

### F2e. 测试

- `translation-toggle.test.tsx`：class 含 `size-8`、图标 `size-4`、不含 `size-6`/`h-3`
- `reasoning.test.tsx`：新增「flex 包裹后 trigger 仍可折叠」+「toggle 非 trigger button 后代（closest("button")）」
- content-parts-renderer 相关既有测试全绿（P6 off 时行为=一期）

---

## 5. FE-3 — P3 划词翻译（selection-action-bubble.tsx）

- 新 prop：`onTranslate?: (text: string) => Promise<string | null>`（resolve null / reject = 失败）
- 常量 `MAX_SELECTION_TRANSLATE_CHARS = 2000`（GAP/EDGE 旁）
- `asking: boolean`/`askingRef` 泛化为 `mode: BubbleMode = "actions"|"asking"|"translating"` + `modeRef` 同步副本；`translation: TranslationCardState | null`（`{status:"loading"|"error"|"done", original, truncated, text?}`）；`translateSeqRef`（迟到结果守卫）
- `closeAsk`→`closeModes()`（mode 复位 + 清 question + setTranslation(null) + seq+1）；dismiss 内改调 closeModes
- **冻结谓词 `modeRef.current !== "actions"` 同 commit 替换三处**（:208-213 selectionchange / :288-292 frame loop / :240-243 pointerup setTimeout）——漏一处复现「卡片悬旧坐标」regression（既有测试 :478-509 防的就是它）
- re-clamp effect（:372-377）：判据 `mode === "actions"` return；依赖加 `translation?.status`（loading→done 变宽二次 clamp）；focus 分支仅 asking
- E2 进入 translating：seq=++translateSeqRef；`raw.length>2000` → slice(0,2000)+truncated=true；setTranslation(loading)；发射 onTranslate（不 await）；E3/E4 resolve/reject 守卫 `modeRef==="translating" && seq 匹配`；**不 dismiss 不清选区**
- E6 退出：Escape（document keydown，translating 注册）/ 卡片 X 按钮 / 外部按压 → dismiss()
- JSX：根容器 `mode==="actions" ? rounded-full : rounded-lg`；translating 时 `flex-col items-stretch`；`mode!=="actions" && "max-w-[calc(100%-1rem)]"`；按钮行顺序 Copy→**Translate**→Quote→Note→Ask（onTranslate 缺席即不渲染）；译文卡片骨架（w-72、Original 标签+truncated 琥珀提示+X、line-clamp-2 原文、loading spinner/error destructive/done whitespace-pre-wrap select-text）
- 测试（复用 mockSelection :56-87 / firePointer :104-114 / mockToolbarWidthByMode :179-198）：9 用例——handler 缺席 / 卡片替换按钮行+resolve 渲染+不清选区 / null 与 reject 双分支内联失败 / 2500 字符截断（onTranslate 收 2000 slice + truncated 提示）/ Escape·外压·X 三路径 dismiss / asking·translating 互斥 / seq 守卫迟到结果 / re-clamp 二次 / 冻结期位置不动

---

## 6. FE-4 — P5 + P4 UI（api.ts + translation-settings.tsx）

- `api.ts`：`translateTexts`（:1715-1720）第三参加 `{ timeoutMs: 300_000 }`（CallOptions 通道现成 transport/types.ts:78-90；Web 生效/Tauri 忽略/RemoteDesktop 透传）；`testTranslationSettings` 后新增 `listTranslationModels(settings: TranslationSettings): Promise<string[]>` → `translation_list_models` `{ settings }`
- `translation-settings.tsx`：
  - state：`modelProbe: {baseUrl, apiKey, kind:"ok", models} | {baseUrl, apiKey, kind:"empty"} | null` + `fetchingModels`
  - 派生失效（kimi 先例 :716-721）：`fetchedModels`/`showEmptyHint` 仅当 probe.baseUrl===settings.baseUrl && probe.apiKey===settings.apiKey 时生效
  - `handleFetchModels`：trim 空防御 return；成功 setProbe(ok/empty)；失败 `toast.error(t("fetchModelsFailed", {error: toErrorMessage(err)}))`（后端分类透传不吞）
  - model 行（:228-237）：flex 行 = Input(`list="translation-model-options"` flex-1) + Button(outline h-8，Loader2/RefreshCw，disabled=`!baseUrl.trim()||!apiKey.trim()||fetching`)；下方 empty hint 内联 + `<datalist>`（option 列表）
  - P4：:210 placeholder → `api.example.com`（裁决 C2）
- 测试：5 用例——空字段禁用 / fetch 成功 datalist option 入 DOM（不模拟点选）/ reject→toast 含后端 message / 空列表→内联 hint 无 toast / 改 baseUrl→option 消失

---

## 7. FE-5 — i18n（10 语言一次补齐）

`Folder.chat.messageList` 6 键：selectionTranslate / selectionTranslating / selectionTranslateFailed / selectionTranslateTruncated({limit}) / selectionTranslateOriginal / selectionTranslateClose
`TranslationSettings` 6 新键：fetchModels / fetchingModels / fetchModelsFailed({error}) / fetchModelsEmpty / formatLabel / formatAuto + 改写 baseUrlDescription（多格式说明）
en 基准与 zh-CN/zh-TW/ja/ko/es/de/fr/pt/ar 文案表见 frontend 计划 §i18n（ICU 占位符各语言原样保留；ar RTL 无内联代码不需隔离）。P1/P2/P6 零新键。messages.test.ts 键集门槛自动强制。

---

## 8. 验收与验证

```bash
# 后端（src-tauri/）
cargo test --features test-utils && cargo clippy --all-targets --features test-utils -- -D warnings
cargo check --no-default-features --bin codeg-server && cargo clippy --no-default-features --bin codeg-server --lib -- -D warnings
# 前端（根）
pnpm test && pnpm eslint .（改动文件子集零告警）&& pnpm build
```

手工验收矩阵（requirements.md §验收标准逐条）：50k ASCII/50k CJK/高密度代码段三组思考样本折叠态可译且 <2min；流式思考跟随（≤0.4 req/s、settled 无整块重发、关闭零增量）；TextPart 流式零请求回归；P4 六形态 URL 保存归一化回显；四格式档案验收——`api.anthropic.com`（auto→anthropic，/v1/messages + x-api-key）、`generativelanguage.googleapis.com/v1beta/openai/`（auto→gemini，兼容端点）、`localhost:11434`（auto→ollama，无 key 可启用可拉列表）、`api.openai.com`（auto→openai），且下拉显式覆盖生效；P5 三形状/401/404 可区分；P3 截断可见+三态互斥+冻结一致。

残余风险（实现者须知）：慢端点+100k 字符逼近 300s 上限时，唯一允许调整为调大 timeoutMs（前端单点），不得改后端并发闸；P6 双发窗口（settle 后 R 在飞时翻转开关）属一次性有界成本，测试注明不覆盖。
