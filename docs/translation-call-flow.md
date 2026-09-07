# 翻译功能调用过程图

> 依据 `feat/translation-middleware` 分支实际代码绘制（2026-09-07）。

## 1. 端到端调用链

```mermaid
flowchart TB
    subgraph FE["前端 (React)"]
        direction TB
        R1["content-parts-renderer.tsx<br/>TextPart (正文, priority=true)<br/>ReasoningPart (思考块, translateThinking)"]
        R2["use-streaming-translated-text (流式机)<br/>segmentsFor → splitStableUnits/tailChunksFor<br/>dispatchBatch / flushSettled / replayGaps"]
        R3["use-translated-text (共用核心)<br/>requestNumberedGroup / requestTranslationDetailed<br/>judgeChunkTranslation (前端门禁)"]
        R4["lib/api.ts translateTexts<br/>invoke() / fetch()"]
    end

    subgraph BE["后端 (Rust)"]
        direction TB
        B1["translation_translate_core<br/>commands/translation.rs + web/handlers"]
        B2["translate_with_cache (LRU 2000)<br/>mod.rs"]
        B3["pool.pick() 三门<br/>retire / fallback partition / probe"]
        B4["translate_batch → translate_one<br/>client.rs"]
        B5["Provider 池<br/>健康分 quality/stability/speed<br/>AIMD 自适应限速"]
        B6["远端端点<br/>OpenAI 兼容 /chat/completions"]
    end

    R1 -->|"块挂载 + viewport 门"| R2
    R2 -->|"numbered 组 / 单段<br/>variant 重试升级"| R3
    R3 -->|"XML envelope<br/>&lt;translate&gt; 包裹 + 参考块"| R4
    R4 -->|"Tauri invoke / HTTP fetch<br/>priority → lane 标记"| B1
    B1 --> B2
    B2 -->|"缓存未命中"| B3
    B2 -->|"缓存命中"| R3
    B3 --> B4
    B4 -->|"Priority lane 4并发 /<br/>Background lane 3并发"| B5
    B5 -->|"软超时 30s = AIMD 半速<br/>慢成功降健康分"| B6
```

## 2. 流式分段与显示链（正文/思考块共用）

```mermaid
flowchart TB
    S0["流式文本 (append-only)"] --> S1
    S1{"splitStableUnits<br/>切分"}
    S1 -->|"空行/ATX 标题封印<br/>= 永不变字节"| S2["sealed units"]
    S1 -->|"尾部按 1500 字符定宽切块<br/>open fence 处停刀"| S3["tail chunks"]

    S2 --> D{"dispatchBatch<br/>每 3s / 800 新字符"}
    S3 --> D
    D -->|"段无字母 (符号/分隔线)"| P1["identity piece 直接落地<br/>(零请求)"]
    D -->|"段已是目标语言 (中文→中文)"| P1
    D -->|其余| Q1["numbered 组请求<br/>≤3000 字符/组"]
    Q1 -->|"3/2 段全部成功"| L1["land(): piece 入链"]
    Q1 -->|"部分失败"| L2["游标回滚至首个失败段<br/>变体升级重试"]

    L1 --> C1["显示链拼接 display<br/>从 offset 0 连续走 piece"]
    L2 -->|"仍失败"| G1["recordGap 挂账<br/>4s→12s→36s 退避重试"]
    G1 -->|"3 次失败 → 放弃"| P2["原文 stitch 为 identity piece<br/>链条保持完整 (不再全块回原文)"]
    G1 -->|"成功"| L1

    C1 -->|"gap 补漏落地<br/>(gate 前先 mergePiecesIntoStore)"| C1
```

## 3. 质量门禁与重试升级

```mermaid
flowchart TB
    subgraph 前端判卷["requestTranslationDetailed 返回后 (前端)"]
        J1{"echoVerbatimError<br/>逐字回显? (CJK 目标)"}
        J2{"missingTargetScript<br/>无目标语言字符? (≥30 拉丁字母)"}
        J3{"missingSourceNumbers<br/>数字丢失过半?"}
        J4{"长度门<br/>2.5×+200 = 编造?"}
        RJ["拒 → variant+1 重试"]
        SP{"splitChunkForHalfRetry<br/>≥800 字符?"}
        HS["两半分别送翻<br/>句界中点 + 占位符不跨界"]
        GIVE["gap 挂账 → 放弃 → 原文 stitch"]

        J1 -->|是| RJ
        J2 -->|是| RJ
        J3 -->|是| RJ
        J4 -->|"是 (INVENTED_CONTENT)"| SP
        SP -->|能拆| HS
        SP -->|不能拆| RJ
        RJ -->|3 次后| GIVE
    end

    subgraph 后端判卷["translate_one 返回后 (后端)"]
        K1{"strip_translate_envelope<br/>剥壳后判卷"}
        BAD["记 ProviderEvent<br/>健康分 stability 扣分"]
        OK["返回 + 记 cache"]
        RATE{"失败率超阈值?"}
        PEN["AIMD penalize 半速"]

        K1 -->|"回显/编造/缺脚本"| BAD
        K1 -->|合格| OK
        BAD --> RATE
        RATE -->|是| PEN
    end

    subgraph 慢请求["在途软超时 (30s)"]
        T1["tokio::select! biased<br/>sleep_until vs pending"]
        T2["SlowInFlight 事件<br/>provider 限速减半<br/>继续等待不放弃"]

        T1 -->|"30s 未返回"| T2
    end
```

## 4. 一条正文的完整生命周期（时序）

```mermaid
sequenceDiagram
    participant U as 用户
    participant FE as 前端 hook
    participant BE as Rust 后端
    participant P as Provider 池

    U->>FE: 消息流式到达 (正文/思考块)
    FE->>FE: 封印单元切分 + 节流门 (3s/800字符)
    FE->>FE: 预检: 符号段/已是中文段 → identity piece (不请求)
    FE->>BE: translateTexts(priority, trace=[块ID])
    BE->>BE: LRU 缓存查询 (内容寻址)
    alt 缓存未命中
        BE->>P: pool.pick() (健康分排序 + 三门)
        P->>BE: PickedProvider
        BE->>P: lane 信号量 (Priority 4 / Background 3)
        Note over BE,P: 30s 软超时看护: AIMD 半速不放弃
        P-->>BE: 译文
        BE->>BE: 剥 envelope + 门禁判卷
        alt 判卷拒绝
            BE->>P: 记事件 / penalize
            BE-->>FE: 失败原因 (error 字符串)
            FE->>FE: variant+1 升级重试 / 半拆 / gap 挂账
        else 通过
            BE-->>FE: 译文 (入 LRU)
            FE->>FE: mergePiecesIntoStore (先落库后判活)
            FE-->>U: 显示链更新 (piece 接续)
        end
    else 缓存命中
        BE-->>FE: fromCache 译文
    end
    U->>FE: 流结束 (settle)
    FE->>FE: flushSettled 补尾 (在途子段重叠则等待)
    FE->>FE: gap replay (3 次放弃 → 原文 stitch)
    FE-->>U: 完整译文 + 译/原切换按钮
```

## 关键文件对照

| 环节 | 文件 |
|---|---|
| 分段/门禁/重试判据 | `src/lib/translation.ts` |
| 流式翻译机 (dispatch/flush/replay) | `src/hooks/use-streaming-translated-text.ts` |
| 请求构造/判卷/缓存键 | `src/hooks/use-translated-text.ts` |
| Provider 池/健康分/AIMD | `src-tauri/src/translation/pool.rs`, `health.rs` |
| 请求执行/软超时/lane | `src-tauri/src/translation/client.rs` |
| 缓存与后端门禁 | `src-tauri/src/translation/mod.rs` |
