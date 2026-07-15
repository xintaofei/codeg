# Auto Reply Design

**Date:** 2026-07-15  
**Status:** Approved for implementation planning  
**Scope:** Chat composer + conversation shell + settings (frontend only)  
**Approach:** Frontend-only rule engine watching error/status signals; per-conversation enable; normal user-send path

## Problem

Unattended agent tasks often stop on recoverable interruptions such as:

- `429 Too Many Requests`
- `503 Service Unavailable`
- Claude API retry / connection error surfaces that leave the session idle until a human types something like `继续`

If those interruptions are auto-handled with a delayed, explicit user-visible reply, unattended success rate improves without hiding what the client is about to send.

## Goals

1. Support configurable auto-reply **rules** (match condition, reply text, delay).
2. Ship built-in rules for **HTTP 429** and **HTTP 503** that reply with **`继续`**.
3. Support per-rule **delay** before send.
4. Allow enabling auto-reply **manually from the composer `+` menu**, scoped **per conversation**.
5. Show a clear **pre-send banner above the input** before any automatic message is sent.
6. Prevent infinite loops via **cooldown + max fires per interruption burst**.
7. Reuse the normal composer send path (no special system message type).
8. Cover matching, lifecycle, loop protection, and UI smoke with tests.
9. Deliver a PR with bilingual (EN + ZH) description.

## Non-goals

- Backend/Rust-owned auto-send.
- Matching assistant chat body text.
- Global enable (or global default + per-conversation override).
- Cross-device rule sync via backend settings API.
- Auto-answering permission / free-text question / ask-question dialogs.
- Inventing a separate message role or silent resume protocol.

## Decisions (from product clarification)

| Topic | Decision |
| --- | --- |
| Enable scope | Per conversation only |
| Rule management | Settings page |
| Match source | Error / status signals only |
| Cancel during countdown | Banner cancel + user manual send |
| Loop protection | Cooldown + max per error burst |
| Architecture | Frontend-only engine (Approach A) |

## Architecture

```
Settings (rules CRUD)
        |
        v
auto-reply settings store (rules + defaults, app-level)
        |
MessageInput + menu --> per-conversation enabled flag
        |
useAutoReplyEngine(error / claudeApiRetry / status)
        |
        |- match first enabled rule
        |- schedule countdown (rule.delayMs)
        |- banner above composer
        +- on fire -> onSend(rule.replyText)  // normal user-send path
```

### Key integration points

| Area | File(s) | Role |
| --- | --- | --- |
| Retry / error state | `src/contexts/acp-connections-context.tsx` | Source of `ClaudeApiRetryState` (`errorStatus`, `error`) and connection `error` / `status` |
| Composer dock | `src/components/chat/conversation-shell.tsx` | Host countdown banner above input |
| `+` menu toggle | `src/components/chat/message-input.tsx` | Per-conversation enable |
| Send path | existing `onSend` / queue plumbing | Auto reply sends as a normal user draft |
| Settings shell | `src/components/settings/settings-shell.tsx` + new settings page | Rule CRUD |
| i18n | `src/i18n/messages/*.json` | EN/ZH (and locale key parity as required by repo) |

## Data model

```ts
type AutoReplyMatchKind =
  | "http_status" // exact HTTP status from ClaudeApiRetryState.errorStatus
  | "error_text" // substring match against retry.error / connection error

interface AutoReplyRule {
  id: string
  name: string
  enabled: boolean
  matchKind: AutoReplyMatchKind
  matchValue: string // "429" | "503" | "Too Many Requests" | ...
  replyText: string // default "继续"
  delayMs: number
  cooldownMs: number
  maxPerBurst: number
  builtin?: boolean // shipped rules: editable, not deletable
}

interface AutoReplySettings {
  version: 1
  rules: AutoReplyRule[]
}

// Per conversation:
// conversationId -> enabled: boolean
```

### Built-in seed rules

| Name | Match | Reply | delayMs | cooldownMs | maxPerBurst |
| --- | --- | --- | --- | --- | --- |
| HTTP 429 | `http_status=429` | `继续` | 3000 | 15000 | 3 |
| HTTP 503 | `http_status=503` | `继续` | 3000 | 15000 | 3 |

Users may edit delay/reply/cooldown/max and disable builtins, but cannot delete them.

### Persistence

- **Rules:** app-level `localStorage` under a versioned key, e.g. `codeg:auto-reply:settings:v1`
- **Enable flag:** per conversation id (and draft/storage key for unsaved drafts if a stable key already exists for that composer instance)
- No backend schema change in v1

## Trigger flow

### When evaluation runs

Re-evaluate when any of these change:

- per-conversation Auto Reply enabled
- `claudeApiRetry`
- connection `error`
- connection `status`
- rules list from settings
- pending permission / question / ask-question state (safety gates)

### Match algorithm

1. Skip if Auto Reply is **off** for this conversation.
2. Skip if a countdown is already scheduled for this conversation.
3. Skip if connection is not safe to send:
   - status is `prompting`, `connecting`, `disconnected`, or `error` in a non-recoverable way that cannot accept a prompt
   - pending permission, free-text question, or ask-question dialog is open
4. Build signal snapshot:
   - `httpStatus = claudeApiRetry?.errorStatus`
   - `errorText = claudeApiRetry?.error ?? connection.error ?? ""`
5. Walk **enabled rules** in list order; first match wins.
   - `http_status`: `Number(matchValue) === httpStatus`
   - `error_text`: case-sensitive substring of `errorText` containing `matchValue` (document exact policy in code; keep simple)
6. Apply loop protection for that rule + conversation:
   - if now < lastSuccessfulSendAt + `cooldownMs` -> skip
   - if same interruption burst already reached `maxPerBurst` -> skip and surface a dismissible stop notice
7. Start countdown for `rule.delayMs`.

### Interruption burst identity

```ts
burstKey = `${httpStatus ?? "none"}|${normalize(errorText)}`
```

`normalize` trims and collapses internal whitespace. Same outage retries share a burst. When the signal clears and a later distinct signal appears, a new burst may fire again.

### Countdown lifecycle

```
matched
  -> pending { ruleId, replyText, fireAt, burstKey, matchedLabel }
  -> banner visible with live remaining seconds
  -> timer fires
      -> if still enabled + still safe + signal still relevant
          -> onSend(plain text draft of replyText)
          -> record lastSentAt + increment burst count
      -> else cancel without sending
```

### Cancel conditions

Cancel pending auto-send (do not send) when:

- user clicks **Cancel** on the banner
- user **manually sends** a message
- Auto Reply is toggled off
- engine unmounts / conversation context is replaced
- connection becomes unsafe (`prompting`, disconnected, pending dialogs)
- matched signal disappears before fire (avoid sending after recovery)

### Send semantics

- Auto reply is a **normal user prompt** through the existing `onSend` (or queue-when-busy) path.
- Do **not** invent a system/hidden message type.
- Do **not** clobber composer draft contents; send `replyText` directly, independent of the editor buffer.
- If the session is busy and the product already queues user sends, auto-reply should follow the same busy-send policy as a manual send of that text. Prefer not to invent a second queue policy.

## UI / UX

### Composer `+` menu

Add a menu item in the existing add-actions dropdown:

- Label: `自动回复` / `Auto Reply`
- Checked / on-off for **this conversation only**
- Click toggles enable; does not open settings
- Optional secondary entry: "Manage rules..." -> Settings -> Auto Reply (nice-to-have)

When enabled, show a low-noise indicator on the `+` control (tint or small badge) so unattended mode is visible without opening the menu.

### Pre-send countdown banner (required)

Render above the input, same width as the composer dock:

**zh-CN**

```
即将自动回复「继续」· 3s          [取消]
匹配：HTTP 429
```

**en**

```
Auto-replying "continue" in 3s     [Cancel]
Matched: HTTP 429
```

Notes:

- Live countdown
- Quote the exact reply text
- Info/warning styling, distinct from the existing Claude API retry destructive strip
- If both exist, auto-reply banner stays closest to the input (action context)

### Settings page

New settings nav item (near Quick Messages):

- Rule list (builtins first)
- Per rule: enable, name, match kind, match value, reply text, delay (seconds UI), cooldown, max per burst
- Builtin badge; builtins not deletable
- Custom rules: add / delete / reorder (first match wins)
- Help text: matches error/status signals only; per-conversation enable lives in composer `+` menu

### States

| State | UI |
| --- | --- |
| Off for conversation | No banner; menu unchecked |
| On, no match | No banner |
| On, countdown | Banner + cancel |
| Hit maxPerBurst | Dismissible notice: auto-reply stopped for this burst |

## Safety defaults

| Field | Builtin default |
| --- | --- |
| `delayMs` | 3000 |
| `cooldownMs` | 15000 |
| `maxPerBurst` | 3 |

## Testing

1. **Rule matching**
   - 429 / 503 http status
   - error_text substring
   - disabled rules ignored
   - first match wins
2. **Engine lifecycle**
   - disabled conversation never schedules
   - schedule uses `delayMs`
   - cancel / manual send prevents send
   - fire only when still safe
3. **Loop protection**
   - cooldown blocks re-fire
   - maxPerBurst stops burst
   - new burst can fire again
4. **UI smoke**
   - `+` toggle flips per-conversation state
   - banner copy + cancel
   - settings edit of builtin delay/reply

## PR delivery

- Branch from `origin/main` (do not stack on unrelated feature branches).
- Implementation plan under `docs/superpowers/plans/`.
- PR description includes **Chinese + English** sections:
  - Summary / 摘要
  - Motivation / 动机
  - Behavior / 行为
  - Test plan / 测试计划

## Success criteria

- With Auto Reply on, an unattended conversation recovering from 429/503 sends `继续` after the configured delay.
- User always sees an explicit pre-send warning and can cancel.
- Persistent rate limits do not produce an infinite auto-send loop.
- No backend protocol changes required for v1.

## Open implementation notes

- Prefer pure helpers (`matchAutoReplyRule`, `shouldScheduleAutoReply`, burst key) for unit tests without mounting the full composer.
- Prefer a small dedicated store/hook module under `src/lib/auto-reply/` or `src/hooks/use-auto-reply.ts` + `src/stores/` only if an existing local pattern fits better.
- Follow existing i18n key parity rules used by the repo for settings/composer strings.
