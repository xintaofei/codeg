# Agent Skill Switch Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make per-Agent skill switches match each Agent's real availability state without moving shared or Codeg-managed skills.

**Architecture:** Codex keeps a declarative `skills.config` implementation and gains complete user, project, plugin, and system discovery. Other Agents may move only entries from private roots into identity-scoped vaults; shared roots and central Codeg skill-pack links are classified but never mutated. Tauri and Axum pass the resolved Codeg data directory to common core functions so project vaults remain outside repositories.

**Tech Stack:** Rust 2021, Tauri 2, Axum, TOML/toml_edit, Next.js 16, React 19, TypeScript, next-intl, Vitest.

## Global Constraints

- Do not merge into, rebase onto, or push `main`.
- Do not move canonical entries from roots consumed by more than one Agent or by external tools.
- Do not move links managed by Experts, Office, Science, or Custom Skills.
- Preserve unknown Codex TOML keys, comments, ordering, and symlinked config targets.
- Use rename-only private toggles and refuse cross-filesystem operations.
- Do not automatically migrate or delete legacy fan-out vaults because they have no ownership manifest.
- Keep the existing frontend request payload stable across Tauri and Axum.

---

### Task 1: Encode Toggle Availability Reasons

**Files:**
- Modify: `src-tauri/src/acp/types.rs`
- Modify: `src/lib/types.ts`
- Modify: `src/components/settings/skills-settings.tsx`
- Modify: `src/components/settings/skills-settings.test.tsx`
- Modify: `src/i18n/messages/*.json`

**Interfaces:**
- Produces: `AgentSkillToggleReason` serialized as snake_case.
- Produces: `AgentSkillItem.toggle_reason: Option<AgentSkillToggleReason>` and TypeScript mirror `toggle_reason: AgentSkillToggleReason | null`.

- [x] **Step 1: Add failing Rust serialization and frontend reason-rendering tests.**
- [x] **Step 2: Run `cargo test --features test-utils agent_skill_toggle_reason -- --nocapture` and the focused Skills Settings Vitest; confirm failures are caused by the missing field and messages.**
- [x] **Step 3: Add the enum/field, map every reason to localized copy, and keep `can_toggle` for transport compatibility.**
- [x] **Step 4: Re-run both focused commands and confirm they pass.**

### Task 2: Replace Fan-Out With Private Root Plans

**Files:**
- Modify: `src-tauri/src/commands/acp.rs`

**Interfaces:**
- Produces: `SkillRootTopology`, resolving all configured roots once per request.
- Produces: `SkillRootPlan { active, vault, legacy_vault, shared }` for one Agent and scope.
- Produces: private vault paths containing workspace, Agent, and root identity.

- [x] **Step 1: Replace fan-out expectations with failing tests asserting `shared_root`, no filesystem mutation, Codeg-managed-link refusal, project vault placement outside the workspace, and distinct vaults for sibling/custom roots.**
- [x] **Step 2: Run the focused `shared_skill`, `managed_skill`, and `project_skill_vault` tests and confirm RED.**
- [x] **Step 3: Implement root plans, active-first listing, exact private moves, same-filesystem checks, collision/link preflight, and read-only legacy discovery. Remove fan-out/link/delete transaction helpers.**
- [x] **Step 4: Re-run focused private/shared tests and confirm GREEN.**

### Task 3: Pass The Effective Data Directory Through Both Runtimes

**Files:**
- Modify: `src-tauri/src/commands/acp.rs`
- Modify: `src-tauri/src/web/handlers/acp.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/tests/api_integration.rs`

**Interfaces:**
- Produces: `_core` list/toggle/read/save/delete functions accepting `data_dir: &Path`.
- Preserves: frontend JSON payloads and Tauri command names.

- [x] **Step 1: Add failing core and Axum tests proving two data directories cannot see each other's project vaults.**
- [x] **Step 2: Run the focused Rust tests and confirm missing explicit data-directory plumbing is the failure.**
- [x] **Step 3: Add Tauri wrappers using `resolve_effective_data_dir`, Axum handlers using `AppState.data_dir`, and route all operations through the core functions.**
- [x] **Step 4: Run desktop and server `cargo check` plus the focused API test.**

### Task 4: Match Codex Native Configuration Semantics

**Files:**
- Modify: `src-tauri/src/commands/acp.rs`

**Interfaces:**
- Consumes: valid selectors containing exactly one of `path` and `name`.
- Produces: path-only overrides for precise per-skill writes.
- Preserves: mixed/unknown entries and both array-of-tables and inline-array formatting.

- [x] **Step 1: Add failing tests for mixed selectors, root inline `skills = { config = [...] }`, frontmatter names, and `skills.bundled.enabled = false`.**
- [x] **Step 2: Run the four named tests and verify each fails for its intended semantic mismatch.**
- [x] **Step 3: Implement exactly-one selector matching, path-only updates, root inline-table mutation, frontmatter-name extraction, and bundled-state overlay.**
- [x] **Step 4: Re-run all Codex skill configuration tests and confirm GREEN.**

### Task 5: Discover Enabled Codex Plugin Skills

**Files:**
- Modify: `src-tauri/src/commands/acp.rs`

**Interfaces:**
- Reads: `[plugins."<name>@<marketplace>"].enabled` and cached `.codex-plugin/plugin.json` manifests.
- Produces: read-only, natively toggleable Codex items named `<plugin>:<skill-frontmatter-name>`.

- [x] **Step 1: Add a failing temporary-CODEX_HOME test with enabled and disabled plugins, duplicate cache versions, and a namespaced `SKILL.md`.**
- [x] **Step 2: Run the plugin test and confirm no plugin skill is currently discovered.**
- [x] **Step 3: Resolve one active cached manifest per enabled plugin, validate its declared skills directory, namespace item identities, and protect plugin files from save/delete.**
- [x] **Step 4: Re-run plugin and native-toggle tests and confirm GREEN.**

### Task 6: Remove Per-Skill Topology Rescans And Verify

**Files:**
- Modify: `src-tauri/src/commands/acp.rs`
- Modify: `docs/superpowers/plans/2026-09-07-agent-skill-switches.md`

**Interfaces:**
- Preserves: deterministic scope/name ordering and active-first duplicate handling.
- Removes: fan-out planning, peer link creation/deletion, and O(skills x peer-directory-scans) capability checks.

- [x] **Step 1: Add a regression test that counts topology/root directory reads independently of skill count where practical, and otherwise assert classification consumes a prebuilt topology.**
- [x] **Step 2: Remove obsolete fan-out tests/helpers and mark the old plan's fan-out task as superseded by this plan.**
- [x] **Step 3: Run `cargo fmt --check`, all skill-related Rust tests, focused frontend tests, and `git diff --check`.**
- [x] **Step 4: Run the complete frontend and Rust verification matrix from `AGENTS.md`, inspect the final diff, and commit the remediation on the current feature branch.**

Verification on 2026-09-09:

- `cargo test --features test-utils skill -- --nocapture`: 75 Skill-related
  tests passed, including the final two-hop alias, physical-parent relative
  link, and absolute-backreference regressions.
- `pnpm test`: 428 files and 6132 tests passed. `pnpm build` passed. ESLint
  exited successfully with one pre-existing warning in
  `src/components/layout/status-bar-mcp.tsx`.
- Desktop `cargo check` and all-target Clippy passed. The complete desktop test
  matrix passed with 3572 library tests plus all integration tests after
  filtering one pre-existing platform-sensitive Git assertion.
- Server check, Clippy, and library tests passed; 3536 library tests passed
  after filtering the same Git assertion. `codeg-mcp` check and Clippy passed.
- `git diff --check` passed.
- `cargo fmt --check` remains red because Rustfmt 1.93 reports repository-wide
  formatting drift across untouched files. Running full-repository formatting
  would create unrelated churn, so this change does not rewrite those files.
- The filtered test is
  `commands::folders::tests::remove_worktree_deletes_a_branch_plain_delete_cannot`.
  Apple Git 2.39.3 says `checked out at`, while the existing assertion accepts
  only `used by worktree`; `src-tauri/src/commands/folders.rs` is unchanged by
  this branch. Running that test alone reproduces the baseline failure.

Residual platform coverage: Unix symbolic-link behavior is covered locally and
Windows link/reparse handling is covered statically and by Windows-gated tests,
but the final link-chain changes were not exercised on a live Windows volume.
Chains longer than 40 links are intentionally classified as `unsafe_link`.
