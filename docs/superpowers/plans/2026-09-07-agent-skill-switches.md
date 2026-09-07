# Per-Agent Skill Switches Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real per-agent availability switches to Settings > Skills without letting a shared skill toggle silently affect another Codeg-managed agent.

**Architecture:** Native skill roots remain authoritative. Disabled entries live in deterministic sibling vaults; shared entries are fanned out into agent-unique roots before the shared source is hidden. The existing list/read/save/delete surface is extended so disabled skills remain manageable, and one new command performs serialized toggles over both transports.

**Tech Stack:** Rust 2021, Tauri 2, Axum, Next.js 16, React 19, TypeScript, next-intl, Vitest.

## Global Constraints

- The switch is per agent and per scope; turning a shared skill off for one agent must preserve availability for every other Codeg-managed agent that already sees it.
- A disabled skill must not remain in any native scan root used by the selected agent.
- Read-only CLI skills cannot be toggled.
- Existing user files and unrelated worktree changes must be preserved.
- Both Tauri desktop and Axum server transports must expose the same behavior.

---

### Task 1: Backend Skill State And Private Toggle

**Files:**
- Modify: `src-tauri/src/acp/types.rs`
- Modify: `src-tauri/src/commands/acp.rs`

**Interfaces:**
- Produces: `AgentSkillItem { enabled: bool, can_toggle: bool, ... }`
- Produces: `acp_set_agent_skill_enabled(agent_type, scope, skill_id, workspace_path, enabled) -> Result<AgentSkillItem, AcpError>`

- [ ] **Step 1: Write failing filesystem tests**

Add tests that create directory and flat-file skills in temporary active roots,
then exercise the wished-for helpers:

```rust
let disabled = disabled_skill_root(&skills);
set_skill_enabled_in_roots(&peers, AgentType::Codex, AgentSkillScope::Global, "demo", false)?;
assert!(!skills.join("demo").exists());
assert!(disabled.join("demo").join("SKILL.md").is_file());
let listed = list_skills_from_roots(AgentSkillScope::Global, &[skills], kind)?;
assert!(!listed[0].enabled);
```

- [ ] **Step 2: Run the focused Rust test and verify RED**

Run:

```bash
cd src-tauri && cargo test --features test-utils skill_enabled -- --nocapture
```

Expected: compilation fails because the new state fields and helpers do not
exist.

- [ ] **Step 3: Implement disabled-root discovery and private moves**

Add deterministic path and entry helpers, active-first list merging, lookup in
both active and disabled roots, a serialized mutation lock, collision
preflight, and idempotent private enable/disable moves. Extend read/save/delete
to resolve disabled entries.

The command contract is:

```rust
pub async fn acp_set_agent_skill_enabled(
    agent_type: AgentType,
    scope: AgentSkillScope,
    skill_id: String,
    workspace_path: Option<String>,
    enabled: bool,
) -> Result<AgentSkillItem, AcpError>;
```

- [ ] **Step 4: Run focused tests and verify GREEN**

Run the command from Step 2. Expected: private directory/flat-file, lookup,
idempotency, collision, and read-only tests pass.

### Task 2: Shared Skill Fan-Out And Isolation

**Files:**
- Modify: `src-tauri/src/commands/acp.rs`
- Reuse: `src-tauri/src/commands/experts.rs`

**Interfaces:**
- Consumes: native roots from `skill_storage_spec` and `scoped_skill_dirs`
- Produces: an internal peer plan containing scan roots and one unique managed root per affected agent

- [ ] **Step 1: Write failing shared-root tests**

Use temporary roots for two peer agents and one shared root. Assert that
disabling for agent B moves the shared source to its vault, links agent A's
unique root to the canonical entry, leaves B's unique root empty, and reports A
enabled/B disabled. Add a peer-without-unique-root case that fails without any
move.

```rust
assert!(peer_a.join("demo").join("SKILL.md").is_file());
assert!(!peer_b.join("demo").exists());
assert!(shared_disabled.join("demo").join("SKILL.md").is_file());
```

- [ ] **Step 2: Run the focused Rust test and verify RED**

Run:

```bash
cd src-tauri && cargo test --features test-utils shared_skill -- --nocapture
```

Expected: the shared source disappears for both peers or the new planning API
is missing.

- [ ] **Step 3: Implement shared preflight, fan-out, and rollback**

Derive peers from `all_acp_agents()`, compare resolved scan-root paths, choose a
root used by exactly one peer, and create compatible directory/file links. Move
the shared source only after every destination has passed preflight. Track and
remove links plus restore the source if a later filesystem action fails.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run the command from Step 2. Expected: fan-out, isolation, refusal, and rollback
tests pass.

### Task 3: Expose The Toggle Over Desktop And Server Transports

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/web/handlers/acp.rs`
- Modify: `src-tauri/src/web/router.rs`
- Modify: `src/lib/api.ts`
- Modify: `src/lib/tauri.ts`
- Modify: `src/lib/types.ts`

**Interfaces:**
- Consumes: the backend command from Task 1
- Produces: `acpSetAgentSkillEnabled(params): Promise<AgentSkillItem>`

- [ ] **Step 1: Add the typed frontend call before registration**

```ts
export async function acpSetAgentSkillEnabled(params: {
  agentType: AgentType
  scope: AgentSkillScope
  skillId: string
  workspacePath?: string | null
  enabled: boolean
}): Promise<AgentSkillItem>
```

- [ ] **Step 2: Run TypeScript checking and verify RED**

Run `pnpm exec tsc --noEmit`. Expected: the transport call or the new DTO fields
are unavailable until all mirrors are updated.

- [ ] **Step 3: Register both transport paths**

Add the Tauri invoke handler, the Axum request DTO/handler, and
`/acp_set_agent_skill_enabled`; mirror `enabled` and `can_toggle` in TypeScript
and send camelCase request keys through the shared transport.

- [ ] **Step 4: Run TypeScript and Rust checks**

Run:

```bash
pnpm exec tsc --noEmit
cd src-tauri && cargo check
cd src-tauri && cargo check --no-default-features --bin codeg-server
```

Expected: all commands exit 0.

### Task 4: Skills Settings Switch And Autocomplete Filtering

**Files:**
- Create: `src/components/settings/skills-settings.test.tsx`
- Modify: `src/components/settings/skills-settings.tsx`
- Create: `src/hooks/use-agent-skills.test.tsx`
- Modify: `src/hooks/use-agent-skills.ts`

**Interfaces:**
- Consumes: `AgentSkillItem.enabled`, `AgentSkillItem.can_toggle`, and `acpSetAgentSkillEnabled`
- Produces: a row switch whose accessible name identifies the skill and selected agent

- [ ] **Step 1: Write failing component and hook tests**

Mock the existing API module, render one enabled skill, click its switch, and
assert the exact request plus authoritative reload:

```ts
expect(acpSetAgentSkillEnabled).toHaveBeenCalledWith({
  agentType: "codex",
  scope: "global",
  skillId: "demo",
  workspacePath: null,
  enabled: false,
})
expect(acpListAgentSkills).toHaveBeenCalledTimes(3)
```

Also assert that `useAgentSkills` excludes `{ enabled: false }` and that
read-only/non-toggleable switches are disabled.

- [ ] **Step 2: Run focused Vitest and verify RED**

Run:

```bash
pnpm test -- src/components/settings/skills-settings.test.tsx src/hooks/use-agent-skills.test.tsx
```

Expected: switch queries and disabled filtering fail because neither behavior
exists.

- [ ] **Step 3: Implement the switch behavior**

Add a stable switch to each list row, stop its click from changing row
selection, track one in-flight skill ID, call the new API, invalidate the cache,
reload the authoritative list, and show localized success/error feedback. Filter
disabled items in `useAgentSkills` before caching.

- [ ] **Step 4: Run focused Vitest and verify GREEN**

Run the command from Step 2. Expected: all focused tests pass.

### Task 5: Localization And Final Verification

**Files:**
- Modify: `src/i18n/messages/ar.json`
- Modify: `src/i18n/messages/de.json`
- Modify: `src/i18n/messages/en.json`
- Modify: `src/i18n/messages/es.json`
- Modify: `src/i18n/messages/fr.json`
- Modify: `src/i18n/messages/ja.json`
- Modify: `src/i18n/messages/ko.json`
- Modify: `src/i18n/messages/pt.json`
- Modify: `src/i18n/messages/zh-CN.json`
- Modify: `src/i18n/messages/zh-TW.json`

**Interfaces:**
- Produces: matching keys for switch labels, enabled/disabled success, failure, and unavailable hints in every locale

- [ ] **Step 1: Add the same message-key shape to all locales**

Add `availability.enabled`, `availability.disabled`,
`availability.toggleAria`, `availability.readOnly`,
`availability.cannotIsolate`, and the corresponding toggle toast keys.

- [ ] **Step 2: Run final frontend verification**

```bash
pnpm eslint .
pnpm test
pnpm build
```

Expected: each command exits 0 with no failing tests.

- [ ] **Step 3: Run final shared-backend verification**

```bash
cd src-tauri && cargo test --features test-utils
cd src-tauri && cargo check --no-default-features --bin codeg-server
cd src-tauri && cargo test --no-default-features --bin codeg-server --lib
```

Expected: each command exits 0 with no failing tests.

- [ ] **Step 4: Review the diff and commit**

Run `git diff --check`, inspect `git diff --stat` and `git status --short`, then
commit only the task files on `task/1` without merging, rebasing, or pushing
`main`.

### Task 6: Review Fixes For Incoming Links And Cursor Vaults

**Files:**
- Modify: `src-tauri/src/commands/acp.rs`

**Interfaces:**
- Produces: a preflight that enumerates every supported entry in peer active
  roots and disabled vaults, then rejects a canonical move if an alias or
  same-name entry directly links to that canonical Skill
- Produces: a distinct deterministic vault only for Cursor's exact builtin
  `~/.cursor/skills-cursor` root, while preserving the legacy vault for
  `~/.cursor/skills` and custom roots that happen to use the same basename
- Preserves: `AgentSkillItem.can_toggle` and toggle execution report the same
  feasibility

- [x] **Step 1: Write three failing filesystem regressions**

Add tests for a shared canonical Skill with an incoming link from a peer that
does not scan the shared root, a private canonical Skill with an incoming peer
link, and Cursor's global writable Skill root:

```rust
assert!(result.is_err(), "the canonical move must be refused");
assert!(canonical.join("SKILL.md").is_file());
assert!(!listed.can_toggle);

assert_eq!(
    disabled_skill_root(&home.join(".cursor/skills")),
    home.join(".cursor/.skills.codeg-disabled")
);
assert_ne!(
    disabled_skill_root(&home.join(".cursor/skills")),
    disabled_skill_root(&home.join(".cursor/skills-cursor"))
);
```

- [x] **Step 2: Run focused tests and verify RED**

Run:

```bash
cd src-tauri
cargo test --features test-utils incoming_peer_link -- --nocapture
cargo test --features test-utils cursor_global_skill_round_trip -- --nocapture
```

Expected: the incoming-link toggles mutate the canonical entry instead of
refusing, and Cursor advertises a toggle that execution rejects because the two
sibling roots resolve to one vault.

- [x] **Step 3: Implement the minimal preflight and vault compatibility fix**

Add an entry-level preflight that scans every peer's supported entries in both
active roots and disabled vaults, then uses `skill_link_targets` to identify
direct incoming links. Allow only the exact same-name active links that the
shared restore plan will remove itself; reject aliases, case variants, and
links in a peer's other roots before moving the canonical entry. Invoke the
same preflight from capability calculation and command execution.

Keep `.cursor/skills` and all unrelated custom roots on their existing
`.skills.codeg-disabled` vaults so disabled entries remain discoverable. Map
only the read-only `.cursor/skills-cursor` root to
`.cursor/.skills-cursor.codeg-disabled` so it no longer claims the writable
root's vault.

- [x] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
cd src-tauri
cargo test --features test-utils incoming_peer_link -- --nocapture
cargo test --features test-utils cursor_global_skill_round_trip -- --nocapture
cargo test --features test-utils skill_capability -- --nocapture
cargo test --features test-utils shared_skill -- --nocapture
```

Expected: all focused regressions and existing Skill isolation tests pass.

- [x] **Step 5: Run the complete repository verification matrix and review**

Run the frontend checks and every desktop, server, and `codeg-mcp` Rust command
listed in `AGENTS.md`, followed by `git diff --check`. Request an independent
read-only review of the final commit range, resolve all Critical or Important
findings, and commit the fixes on `task/1`.

The first review found alias/case-variant links outside the exact Skill ID and
custom roots named `skills-cursor` were still unsafe. Those cases now have
RED/GREEN regressions, and the follow-up review reported no remaining Critical
or Important findings.
