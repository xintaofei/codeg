# Repeat Intent Detection Design

**Date:** 2026-07-15  
**Status:** Approved for implementation planning  
**Scope:** Chat composer send path + pending message queue (frontend only)  
**Approach:** Pure trailing-multiplier parser + confirm dialog + bulk enqueue

## Problem

Users often want to queue the same short follow-up many times (for example `继续` / `continue`) while an agent is working through a multi-step task. Typing or clicking send repeatedly is tedious. A common natural shorthand is:

```text
继续x10
继续 X10
continue x 5
```

Codeg already has a pending-message queue (`useMessageQueue`) and auto-flush when a turn becomes free. What is missing is recognition of the trailing `xN` / `XN` multiplier and a confirmation step that expands it into N queued drafts.

## Goals

1. Detect a trailing repeat multiplier on composer submit.
2. Support `x` and `X`, with optional spaces around the multiplier.
3. Match only a **trailing** suffix (not whole-string-only, not mid-text).
4. Show a confirmation dialog before expanding.
5. On confirm, enqueue **N identical base messages** into the existing pending queue.
6. On cancel/dismiss, keep the original composer text unchanged.
7. Cap N to a safe range: **2..50**.
8. Cover the behavior with unit tests and i18n (EN + zh-CN minimum; all locales if key parity is enforced).

## Non-goals

- Live detection while typing / inline chips.
- Direct multi-send that bypasses the queue.
- Multipliers other than `x` / `X` (`*10`, `×10`, `x10次`, etc.).
- Persisted "always expand" preference.
- Backend/Rust changes.
- Changing auto-flush timing or queue FIFO semantics.

## Decisions (from product clarification)

| Topic | Decision |
| --- | --- |
| Match scope | Trailing multiplier suffix only |
| Confirm action | Enqueue all N items into pending queue |
| Cancel action | Close dialog only; keep original text including `xN` |
| Valid N | Integer in **2..50** inclusive |
| Idle vs busy | Always enqueue N; do not special-case direct first send |

## Detection rules

### Input source

On send, parse the draft's plain `displayText` after normal draft construction.

Do **not** run detection when:

- composer is in queue-item edit mode
- `buildDraft()` fails / returns empty
- `onEnqueue` / bulk-enqueue path is unavailable (fall back to normal single send of the original draft)

### Pattern

Conceptual regex against the full display text:

```text
^(?s)(.+?)[ \t]*[xX][ \t]*(\d+)[ \t]*$
```

Rules:

1. Trim only trailing whitespace of the whole input before matching if needed; keep internal spaces inside the base text.
2. `baseText` = capture group 1, right-trimmed; must be non-empty.
3. `count` = integer from capture group 2.
4. Trigger only when `2 <= count <= 50`.
5. Multiplier must be trailing; anything after the number fails the match.

### Positive examples

| Input | baseText | count |
| --- | --- | --- |
| `继续X10` | `继续` | 10 |
| `继续x10` | `继续` | 10 |
| `继续 X10` | `继续` | 10 |
| `继续 x 10` | `继续` | 10 |
| `continue X 3` | `continue` | 3 |
| `请继续修复x10` | `请继续修复` | 10 |
| `fix this\nplease x2` | `fix this\nplease` | 2 |

### Negative examples

| Input | Reason |
| --- | --- |
| `继续x1` | below min |
| `继续x51` | above max |
| `x10` | empty base text |
| `继续X10 请` | multiplier not trailing |
| `继续X10a` | non-digit tail |
| `继续*10` | unsupported operator |
| attachment-only empty text | no textual multiplier |

### Rich drafts / attachments

- Multiplier is detected only from trailing **text**.
- On confirm, strip the multiplier from text (`displayText` and text blocks).
- Keep image/resource/reference blocks unchanged.
- If stripping leaves empty prose but attachments remain, the resulting draft is still valid (same as normal attachment-only drafts).

## Interaction flow

```text
User presses Send / Enter
        |
        v
buildDraft()
        |
        +-- queue edit mode? --> save edit (no repeat detect)
        |
        v
parseRepeatIntent(displayText)
        |
        +-- no match / out of range --> existing send or single enqueue
        |
        v
open AlertDialog (composer text kept)
        |
        +-- Cancel / Esc / dismiss --> close only
        |
        v
Confirm
  - build base draft (suffix stripped)
  - enqueueMany(baseDraft, modeId, count)
  - clear composer
  - existing queue UI + auto-flush handle delivery
```

### Dialog copy

i18n keys under the chat/message-input namespace:

- Title: "Queue repeated messages?" / "生成重复待发送消息？"
- Description: include `count` and a short preview of `baseText`
- Confirm: "Queue N messages" / "生成 N 条待发送"
- Cancel: reuse existing cancel wording where possible

### Enqueue semantics

- Always enqueue **N** copies of the base draft.
- Do **not** direct-send the first item even when the agent is idle.
- Prefer one atomic queue commit (`enqueueMany`) to avoid N React state updates and intermediate renders.
- Each queued item gets its own id (existing `randomUUID()` behavior).
- Mode id uses the same value the normal enqueue path would use.

If bulk enqueue plumbing is unavailable on a surface, fall back to normal single send of the **original** unsplit draft. Never silently multi-send outside the queue.

## Architecture

### Components

1. **`src/lib/repeat-intent.ts`** (pure)
   - `MIN_REPEAT_COUNT = 2`
   - `MAX_REPEAT_COUNT = 50`
   - `parseRepeatIntent(text: string): { baseText: string; count: number } | null`
   - helper(s) to strip the trailing multiplier from draft text/blocks

2. **`src/lib/repeat-intent.test.ts`**
   - spaces, case, bounds, unicode base text, newlines, non-matches

3. **`src/hooks/use-message-queue.ts`**
   - add `enqueueMany(draft, modeId, count)`
   - single `commit([...queue, ...newItems])`
   - unit coverage for atomic append + count clamp/guard (`count < 1` no-op)

4. **`src/components/chat/message-input.tsx`**
   - intercept in `handleSend` before normal send/enqueue
   - local dialog state: open flag + pending `{ draft, modeId, baseText, count }`
   - render shared `AlertDialog` on confirm/cancel

5. **Parent wiring**
   - `conversation-detail-panel` already owns `useMessageQueue`
   - pass bulk enqueue down through `chat-input` / `conversation-shell` as `onEnqueueMany` (or equivalent)
   - keep existing `onEnqueue` for single-item paths

6. **i18n**
   - `en.json`, `zh-CN.json`, and any other locales required by key-parity tests

### Data flow

```text
MessageInput.handleSend
  -> parseRepeatIntent
  -> (confirm)
  -> onEnqueueMany(baseDraft, modeId, count)
  -> useMessageQueue.enqueueMany
  -> MessageQueueDisplay + existing auto-flush
```

### Error handling

- Invalid/out-of-range count: treat as normal message, no dialog.
- Dialog open while status changes: cancel keeps text; confirm still only enqueues (safe if agent becomes idle/busy).
- No new backend error paths.

## Testing plan

1. **Parser unit tests**
   - all positive/negative examples above
   - leading/trailing spaces around `x` and digits
   - multiline base text with trailing multiplier

2. **Queue unit tests**
   - `enqueueMany` appends N items with identical draft/modeId and unique ids
   - does not mutate previous queue order
   - `count <= 0` is a no-op

3. **Composer behavior tests** (message-input or focused helper tests)
   - matching text opens dialog and does not enqueue yet
   - confirm enqueues N and clears composer
   - cancel leaves original text
   - queue-edit mode skips detection
   - out-of-range sends/enqueues once through normal path

## Implementation notes

- Prefer pure helpers over embedding regex in the React component.
- Reuse existing `AlertDialog` primitives (`src/components/ui/alert-dialog.tsx`).
- Keep the feature frontend-only; no Tauri/command changes.
- Follow Prettier/ESLint project style (no semicolons, etc.).
- Work from a fresh branch off `main`, not the current Tailscale funnel branch.

## PR expectations

- Feature branch: `feat/repeat-intent-queue` (or similar)
- PR body includes **English + 中文** summary, behavior, test plan, and screenshots/GIFs if UI is easy to capture
- Target repository: `xintaofei/codeg` (use fork remote if origin push is unavailable)

## Open questions

None remaining for v1. Future optional enhancements (out of scope now):

- remember "don't ask again" for the session
- custom max count in settings
- support for `×` / `*` multipliers