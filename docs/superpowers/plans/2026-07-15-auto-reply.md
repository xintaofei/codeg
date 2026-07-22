# Auto Reply Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a frontend-only Auto Reply engine that recovers unattended agent tasks from 429/503-style interruptions by auto-sending a configured reply (default `继续`) after a visible countdown, with per-conversation enable, settings-managed rules, and loop protection.

**Architecture:** Pure match/storage helpers under `src/lib/auto-reply/` feed a conversation-scoped `useAutoReplyEngine` hook. The engine watches error/status signals, schedules a countdown banner above the composer, and fires through the normal `onSend(PromptDraft)` path. Rules live in app-level localStorage; enable flags are per conversation/draft key.

**Tech Stack:** React, TypeScript, next-intl, Vitest, localStorage, existing shadcn UI (`DropdownMenuCheckboxItem`, `Switch`, `Button`, `Input`, `Select`).

## Global Constraints

- Frontend-only; no backend/Rust protocol changes.
- Match **error/status signals only** (not assistant body text).
- Enable scope is **per conversation only** (composer `+` menu).
- Rule CRUD lives on a **Settings** page.
- Built-ins: HTTP 429 and HTTP 503 -> reply `继续`; `delayMs=3000`, `cooldownMs=15000`, `maxPerBurst=3`.
- Built-ins editable, **not deletable**.
- Pre-send banner above input with live countdown + cancel.
- Manual user send cancels pending auto-reply.
- Safe send only when connected-safe and no permission/question dialogs; never while `prompting`.
- Do not clobber composer draft; send `replyText` as a standalone draft.
- Prettier: no semicolons, trailing commas `es5`, 2-space indent.
- i18n key parity across all 10 locales (`en` is source of truth).
- `docs/superpowers/**` is gitignored — always `git add -f` for plan/spec commits.

---

## File map

| Path | Responsibility |
| --- | --- |
| `src/lib/auto-reply/types.ts` | Shared types + builtin seed factory |
| `src/lib/auto-reply/match.ts` | Pure matching, safety, burst helpers |
| `src/lib/auto-reply/match.test.ts` | Unit tests for match helpers |
| `src/lib/auto-reply/storage.ts` | localStorage load/save for settings + enable map |
| `src/lib/auto-reply/storage.test.ts` | Storage tests |
| `src/lib/auto-reply/settings-store.ts` | In-memory settings cache + subscribers |
| `src/hooks/use-auto-reply-engine.ts` | Countdown lifecycle hook |
| `src/hooks/use-auto-reply-engine.test.ts` | Fake-timer lifecycle tests |
| `src/components/chat/auto-reply-banner.tsx` | Countdown + stop-notice UI |
| `src/components/settings/auto-reply-settings.tsx` | Rules CRUD UI |
| `src/app/settings/auto-reply/page.tsx` | Settings route |
| `src/components/settings/settings-shell.tsx` | Nav entry |
| `src/components/chat/conversation-shell.tsx` | Host engine + banner |
| `src/components/chat/message-input.tsx` | `+` menu toggle |
| `src/components/chat/chat-input.tsx` | Pass enable props |
| `src/i18n/messages/*.json` | Strings (10 locales) |

Storage keys:
- `codeg:auto-reply:settings:v1`
- `codeg:auto-reply:enabled:v1`

Enable key: prefer `draftStorageKey` when present, else a stable conversation/virtual id.

---

### Task 1: Pure types + match helpers

**Files:**
- Create: `src/lib/auto-reply/types.ts`
- Create: `src/lib/auto-reply/match.ts`
- Test: `src/lib/auto-reply/match.test.ts`

**Interfaces:**
- Produces: `AutoReplyRule`, `AutoReplySettings`, `AutoReplyMatchKind`, `AutoReplySignal`, `AutoReplySafetyInput`, `createBuiltinRules()`, `normalizeErrorText()`, `buildBurstKey()`, `findMatchingRule()`, `canScheduleAutoReply()`, `isSafeToAutoReply()`, `signalFromSources()`, `buildAutoReplyDraft()`

- [ ] **Step 1: Write failing tests** in `match.test.ts` covering builtins, http_status, error_text first-match, burst key, cooldown/maxPerBurst, safety gates, signal source preference.
- [ ] **Step 2: Run** `pnpm exec vitest run src/lib/auto-reply/match.test.ts` (expect FAIL).
- [ ] **Step 3: Implement** types + match helpers as specified in design.
- [ ] **Step 4: Re-run tests** (expect PASS).
- [ ] **Step 5: Commit** `feat(auto-reply): add match helpers and builtin 429/503 rules`

Builtin rule ids: `builtin-http-429`, `builtin-http-503`.
`isSafeToAutoReply`: status must be `"connected"` and no pending permission/question/ask-question.
`error_text` match is case-sensitive substring.
Burst key: `` `${httpStatus ?? "none"}|${normalize(errorText)}` ``.

---

### Task 2: Storage + settings store

**Files:**
- Create: `src/lib/auto-reply/storage.ts`
- Create: `src/lib/auto-reply/settings-store.ts`
- Test: `src/lib/auto-reply/storage.test.ts`

- [ ] **Step 1: Tests** for defaults, corrupt JSON, round-trip, enable map, re-inject missing builtins.
- [ ] **Step 2: Implement** localStorage helpers + `useSyncExternalStore` settings store.
- [ ] **Step 3: Pass tests + commit** `feat(auto-reply): persist rules and per-conversation enable flags`

---

### Task 3: Engine hook

**Files:**
- Create: `src/hooks/use-auto-reply-engine.ts`
- Test: `src/hooks/use-auto-reply-engine.test.ts`

API:

```ts
useAutoReplyEngine({
  enabled, status, error, claudeApiRetry,
  pendingPermission, pendingQuestion, pendingAskQuestion,
  onSend,
}): {
  pending, stopNotice, cancelPending, notifyManualSend, dismissStopNotice
}
```

- [ ] **Step 1: Fake-timer tests** for disable, schedule/fire, cancel, manual send, unsafe cancel, signal clear, cooldown, maxPerBurst, new burst.
- [ ] **Step 2: Implement hook** with refs for timers/burst counters; re-check safety at fire.
- [ ] **Step 3: Commit** `feat(auto-reply): add countdown engine with loop protection`

---

### Task 4: i18n (all 10 locales)

- SettingsShell.nav.auto_reply
- AutoReplySettings.* (settings page strings)
- Folder.chat.messageInput.autoReply (+ hint)
- Folder.chat.autoReply.* (banner/stop notice)

- [ ] Add keys to en + zh-CN with real copy; other locales can mirror EN but must keep parity.
- [ ] Run `pnpm exec vitest run src/i18n/messages.test.ts`
- [ ] Commit `feat(auto-reply): add i18n strings for composer and settings`

---

### Task 5: Banner + shell / menu wiring

- Create `auto-reply-banner.tsx`
- Wire engine in `conversation-shell.tsx` (banner closest to input)
- Wrap composer send to call `notifyManualSend`
- Pass enable state through chat-input -> message-input
- `+` menu `DropdownMenuCheckboxItem` toggle; tint `+` when enabled

- [ ] Implement + commit `feat(auto-reply): wire countdown banner and + menu toggle`

---

### Task 6: Settings page + nav

- `src/components/settings/auto-reply-settings.tsx`
- `src/app/settings/auto-reply/page.tsx`
- Nav item near Quick Messages; builtins not deletable; delay/cooldown in seconds UI

- [ ] Implement + commit `feat(auto-reply): add settings page for rule management`

---

### Task 7: Verify + bilingual PR

```bash
pnpm exec vitest run src/lib/auto-reply src/hooks/use-auto-reply-engine.test.ts src/i18n/messages.test.ts
git add -f docs/superpowers/plans/2026-07-15-auto-reply.md
git push -u origin feat/auto-reply
gh pr create --base main --head feat/auto-reply --title "feat: auto-reply for recoverable agent interruptions (429/503)" --body "..."
```

PR body must include Chinese + English sections: Summary, Motivation, Behavior, Test plan.

---

## Self-review

1. Spec coverage: enable toggle, settings rules, 429/503 builtins, delay, banner, cancel, manual-send cancel, cooldown/maxPerBurst, normal onSend, bilingual PR — covered.
2. No intentional placeholders.
3. Shared types/names consistent across tasks.

## Execution note

User has repeatedly said 继续 — execute inline on `feat/auto-reply` after committing this plan. Do not restore the repeat-intent stash onto this branch.
