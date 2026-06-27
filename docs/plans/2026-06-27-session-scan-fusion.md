# Session Scan Fusion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand Codeg's local Codex/Claude history discovery and make imported native sessions resume through the native tools while keeping the existing clean Codeg UI and avoiding direct native transcript write-back.

**Architecture:** Phase 1 changes the Rust backend parser roots. `CodexParser` moves from one `base_dir` to ordered session roots, scans active and archived roots, deduplicates by native session id, and keeps existing `ConversationSummary`/`ConversationDetail` interfaces. Phase 2 is delivered in the same branch by verifying and tightening the existing `external_id -> acp_connect(sessionId)` resume path for imported Codex/Claude conversations; it does not fabricate Codex/Claude JSONL.

**Tech Stack:** Rust 2021, Tauri backend, `walkdir`, `chrono`, existing Codeg parser traits, Rust unit tests in `src-tauri/src/parsers`.

## Global Constraints

- Preserve Codeg's current UI and conversation database schema.
- Do not merge desktop-cc-gui's full session catalog UI or runtime model.
- Do not read another application's private SQLite state.
- Do not start Codex or Claude processes just to list history.
- Do not directly fabricate or rewrite Codex/Claude JSONL transcripts.
- Missing roots return empty results or partial results; unreadable files are skipped consistently with current parser behavior.
- Use `CODEG_CODEX_HOME_DIRS` for explicitly configured extra Codex/provider homes.

---

## File Structure

- Modify `src-tauri/src/parsers/codex.rs`: add Codex session root resolution, multi-root parser state, list/detail search, and focused tests.
- Modify `src-tauri/src/parsers/mod.rs`: update Codex external transcript sources to include active and archived roots.
- Modify `docs/specs/2026-06-27-session-scan-fusion-design.md`: already done; keep it as the source design.
- No frontend files change in Phase 1.
- No database migrations change in Phase 1.
- Phase 2 may modify `src/components/conversations/conversation-detail-panel.tsx`, `src/contexts/acp-connections-context.tsx`, `src/hooks/use-connection-lifecycle.ts`, and tests only if the existing imported-session resume path is not already covered.

---

### Task 1: Codex Session Root Resolver

**Files:**
- Modify: `src-tauri/src/parsers/codex.rs`

**Interfaces:**
- Produces: `pub(crate) fn resolve_codex_session_roots() -> Vec<PathBuf>`
- Produces: `fn resolve_codex_session_roots_from(codex_home_env: Option<OsString>, extra_homes_env: Option<OsString>, home_dir: Option<PathBuf>) -> Vec<PathBuf>`
- Consumes: existing `resolve_codex_home_dir_from(...) -> PathBuf`

- [ ] **Step 1: Write failing tests for default active and archived roots**

Add this test module content inside the existing `#[cfg(test)] mod tests` in `src-tauri/src/parsers/codex.rs`:

```rust
#[test]
fn codex_session_roots_include_sessions_and_archived_sessions() {
    let home = PathBuf::from("/Users/default");
    let roots = resolve_codex_session_roots_from(None, None, Some(home));

    assert_eq!(
        roots,
        vec![
            PathBuf::from("/Users/default/.codex/sessions"),
            PathBuf::from("/Users/default/.codex/archived_sessions"),
        ]
    );
}

#[test]
fn codex_session_roots_honor_codex_home_override() {
    let roots = resolve_codex_session_roots_from(
        Some(std::ffi::OsString::from("/tmp/custom-codex")),
        None,
        Some(PathBuf::from("/Users/default")),
    );

    assert_eq!(
        roots,
        vec![
            PathBuf::from("/tmp/custom-codex/sessions"),
            PathBuf::from("/tmp/custom-codex/archived_sessions"),
        ]
    );
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```powershell
cd src-tauri
cargo test codex_session_roots_include_sessions_and_archived_sessions codex_session_roots_honor_codex_home_override
```

Expected: FAIL because `resolve_codex_session_roots_from` does not exist.

- [ ] **Step 3: Implement root resolver**

In `src-tauri/src/parsers/codex.rs`, change imports:

```rust
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::OnceLock;
```

Add these functions after `resolve_codex_home_dir_from`:

```rust
pub(crate) fn resolve_codex_session_roots() -> Vec<PathBuf> {
    resolve_codex_session_roots_from(
        std::env::var_os("CODEX_HOME"),
        std::env::var_os("CODEG_CODEX_HOME_DIRS"),
        dirs::home_dir(),
    )
}

fn resolve_codex_session_roots_from(
    codex_home_env: Option<OsString>,
    extra_homes_env: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let primary_home = resolve_codex_home_dir_from(codex_home_env, home_dir);
    let mut homes = vec![primary_home];

    if let Some(extra_homes) = extra_homes_env {
        for home in std::env::split_paths(&extra_homes) {
            if !home.as_os_str().is_empty() {
                homes.push(home);
            }
        }
    }

    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    for home in homes {
        for root in [home.join("sessions"), home.join("archived_sessions")] {
            if seen.insert(root.clone()) {
                roots.push(root);
            }
        }
    }
    roots
}
```

- [ ] **Step 4: Run tests and verify pass**

Run:

```powershell
cd src-tauri
cargo test codex_session_roots_include_sessions_and_archived_sessions codex_session_roots_honor_codex_home_override
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/parsers/codex.rs
git commit -m "feat: resolve codex session roots"
```

---

### Task 2: Codex Parser Multi-Root Listing

**Files:**
- Modify: `src-tauri/src/parsers/codex.rs`

**Interfaces:**
- Consumes: `resolve_codex_session_roots() -> Vec<PathBuf>`
- Produces: `CodexParser { session_roots: Vec<PathBuf> }`
- Produces: `fn list_candidate_jsonl_files(&self) -> Vec<PathBuf>`

- [ ] **Step 1: Write failing tests for archived scan and dedupe**

Add helper and tests in `src-tauri/src/parsers/codex.rs` test module:

```rust
fn write_codex_rollout(path: &PathBuf, id: &str, cwd: &str, timestamp: &str, title: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    let content = format!(
        "{{\"timestamp\":\"{timestamp}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"{cwd}\",\"git\":{{\"branch\":\"main\"}}}}}}\n\
         {{\"timestamp\":\"{timestamp}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{title}\"}}}}\n"
    );
    std::fs::write(path, content).expect("write rollout");
}

#[test]
fn list_conversations_scans_archived_session_roots() {
    let base = std::env::temp_dir().join(format!(
        "codeg-codex-roots-{}",
        uuid::Uuid::new_v4()
    ));
    let archived = base.join("archived_sessions").join("2026").join("06").join("27");
    write_codex_rollout(
        &archived.join("rollout-2026-06-27T00-00-00-archived.jsonl"),
        "archived-1",
        "/repo",
        "2026-06-27T00:00:00Z",
        "archived prompt",
    );

    let parser = CodexParser::with_session_roots(vec![
        base.join("sessions"),
        base.join("archived_sessions"),
    ]);
    let summaries = parser.list_conversations().expect("list conversations");

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "archived-1");
    assert_eq!(summaries[0].title.as_deref(), Some("archived prompt"));

    std::fs::remove_dir_all(base).expect("cleanup");
}

#[test]
fn list_conversations_dedupes_duplicate_session_ids_by_newest_timestamp() {
    let base = std::env::temp_dir().join(format!(
        "codeg-codex-dedupe-{}",
        uuid::Uuid::new_v4()
    ));
    let active = base.join("sessions").join("2026").join("06").join("27");
    let archived = base.join("archived_sessions").join("2026").join("06").join("27");
    write_codex_rollout(
        &archived.join("rollout-2026-06-27T00-00-00-same.jsonl"),
        "same-1",
        "/repo",
        "2026-06-27T00:00:00Z",
        "older archived prompt",
    );
    write_codex_rollout(
        &active.join("rollout-2026-06-27T01-00-00-same.jsonl"),
        "same-1",
        "/repo",
        "2026-06-27T01:00:00Z",
        "newer active prompt",
    );

    let parser = CodexParser::with_session_roots(vec![
        base.join("sessions"),
        base.join("archived_sessions"),
    ]);
    let summaries = parser.list_conversations().expect("list conversations");

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "same-1");
    assert_eq!(summaries[0].title.as_deref(), Some("newer active prompt"));

    std::fs::remove_dir_all(base).expect("cleanup");
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```powershell
cd src-tauri
cargo test list_conversations_scans_archived_session_roots list_conversations_dedupes_duplicate_session_ids_by_newest_timestamp
```

Expected: FAIL because `with_session_roots` does not exist and parser only has `base_dir`.

- [ ] **Step 3: Replace single base dir with session roots**

In `src-tauri/src/parsers/codex.rs`, replace:

```rust
pub struct CodexParser {
    base_dir: PathBuf,
}
```

with:

```rust
pub struct CodexParser {
    session_roots: Vec<PathBuf>,
}
```

Replace `new()` and test constructors with:

```rust
pub fn new() -> Self {
    Self {
        session_roots: resolve_codex_session_roots(),
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub fn with_base_dir(base_dir: PathBuf) -> Self {
    Self {
        session_roots: vec![base_dir],
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub fn with_session_roots(session_roots: Vec<PathBuf>) -> Self {
    Self { session_roots }
}
```

Add helper methods inside `impl CodexParser`:

```rust
fn candidate_jsonl_files(&self) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in &self.session_roots {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path().to_path_buf();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            if fname.starts_with("rollout-") {
                files.push(path);
            }
        }
    }
    files
}

fn dedupe_conversations(
    conversations: Vec<ConversationSummary>,
) -> Vec<ConversationSummary> {
    let mut by_id: HashMap<String, ConversationSummary> = HashMap::new();
    for summary in conversations {
        match by_id.get(&summary.id) {
            Some(existing) if existing.started_at >= summary.started_at => {}
            _ => {
                by_id.insert(summary.id.clone(), summary);
            }
        }
    }
    let mut conversations: Vec<ConversationSummary> = by_id.into_values().collect();
    conversations.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    conversations
}
```

Replace `list_conversations` with:

```rust
fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
    let mut conversations = Vec::new();

    for path in self.candidate_jsonl_files() {
        match self.parse_jsonl_summary(&path) {
            Ok(Some(summary)) => conversations.push(summary),
            _ => continue,
        }
    }

    Ok(Self::dedupe_conversations(conversations))
}
```

- [ ] **Step 4: Run tests and verify pass**

Run:

```powershell
cd src-tauri
cargo test list_conversations_scans_archived_session_roots list_conversations_dedupes_duplicate_session_ids_by_newest_timestamp
```

Expected: PASS.

- [ ] **Step 5: Run existing Codex parser tests**

Run:

```powershell
cd src-tauri
cargo test codex
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src-tauri/src/parsers/codex.rs
git commit -m "feat: scan multiple codex session roots"
```

---

### Task 3: Codex Detail Lookup Across Roots

**Files:**
- Modify: `src-tauri/src/parsers/codex.rs`

**Interfaces:**
- Consumes: `CodexParser::candidate_jsonl_files() -> Vec<PathBuf>`
- Produces: root-wide `get_conversation(&self, conversation_id: &str)`

- [ ] **Step 1: Write failing test for archived detail lookup**

Add this test:

```rust
#[test]
fn get_conversation_searches_archived_session_roots() {
    let base = std::env::temp_dir().join(format!(
        "codeg-codex-detail-{}",
        uuid::Uuid::new_v4()
    ));
    let archived = base.join("archived_sessions").join("2026").join("06").join("27");
    write_codex_rollout(
        &archived.join("rollout-2026-06-27T00-00-00-detail.jsonl"),
        "detail-1",
        "/repo",
        "2026-06-27T00:00:00Z",
        "detail prompt",
    );

    let parser = CodexParser::with_session_roots(vec![
        base.join("sessions"),
        base.join("archived_sessions"),
    ]);
    let detail = parser
        .get_conversation("detail-1")
        .expect("load archived detail");

    assert_eq!(detail.id, "detail-1");

    std::fs::remove_dir_all(base).expect("cleanup");
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```powershell
cd src-tauri
cargo test get_conversation_searches_archived_session_roots
```

Expected: FAIL if `get_conversation` still references removed `base_dir` or does not search all roots.

- [ ] **Step 3: Implement all-root detail search**

Replace `get_conversation` in `impl AgentParser for CodexParser` with:

```rust
fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
    let files = self.candidate_jsonl_files();

    for path in &files {
        if let Ok(Some(summary)) = self.parse_jsonl_summary(path) {
            if summary.id == conversation_id {
                return self.parse_conversation_detail(path, conversation_id);
            }
        }
    }

    for path in files {
        let fname = path.file_name().unwrap_or_default().to_string_lossy();
        if fname.contains(conversation_id) {
            return self.parse_conversation_detail(&path, conversation_id);
        }
    }

    Err(ParseError::ConversationNotFound(
        conversation_id.to_string(),
    ))
}
```

- [ ] **Step 4: Run test and verify pass**

Run:

```powershell
cd src-tauri
cargo test get_conversation_searches_archived_session_roots
```

Expected: PASS.

- [ ] **Step 5: Run parser test group**

Run:

```powershell
cd src-tauri
cargo test codex
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src-tauri/src/parsers/codex.rs
git commit -m "feat: load codex details from all roots"
```

---

### Task 4: External Transcript Sources Include Archived Codex Sessions

**Files:**
- Modify: `src-tauri/src/parsers/mod.rs`
- Modify: `src-tauri/src/parsers/codex.rs`

**Interfaces:**
- Consumes: `codex::resolve_codex_session_roots() -> Vec<PathBuf>`
- Produces: `external_transcript_sources()` returns Codex active and archived roots

- [ ] **Step 1: Add test for Codex external sources**

Add a test module near the bottom of `src-tauri/src/parsers/mod.rs`:

```rust
#[cfg(test)]
mod external_source_tests {
    use super::*;

    #[test]
    fn external_sources_include_codex_active_and_archived_roots() {
        let sources = external_transcript_sources();
        let codex_roots: Vec<String> = sources
            .iter()
            .filter(|source| source.agent == "codex")
            .map(|source| source.root.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(
            codex_roots.iter().any(|root| root.ends_with("/sessions")),
            "codex sessions root missing: {codex_roots:?}"
        );
        assert!(
            codex_roots
                .iter()
                .any(|root| root.ends_with("/archived_sessions")),
            "codex archived_sessions root missing: {codex_roots:?}"
        );
    }
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```powershell
cd src-tauri
cargo test external_sources_include_codex_active_and_archived_roots
```

Expected: FAIL because only `sessions` is listed.

- [ ] **Step 3: Update external sources**

In `src-tauri/src/parsers/mod.rs`, replace the single Codex `ExternalSource` entry:

```rust
ExternalSource {
    agent: "codex",
    root: codex::resolve_codex_home_dir().join("sessions"),
    is_file: false,
    include_top: None,
},
```

with dynamic extension after the initial vector is created. Remove the single Codex entry from the `vec![...]`, then add:

```rust
    for root in codex::resolve_codex_session_roots() {
        sources.push(ExternalSource {
            agent: "codex",
            root,
            is_file: false,
            include_top: None,
        });
    }
```

Place the loop after the initial `vec![...]` and before the OpenClaw home block so Codex sources are included once.

- [ ] **Step 4: Run test and verify pass**

Run:

```powershell
cd src-tauri
cargo test external_sources_include_codex_active_and_archived_roots
```

Expected: PASS.

- [ ] **Step 5: Run backup-related external parser tests**

Run:

```powershell
cd src-tauri
cargo test external
```

Expected: PASS or no unrelated failures.

- [ ] **Step 6: Commit**

```powershell
git add src-tauri/src/parsers/mod.rs src-tauri/src/parsers/codex.rs
git commit -m "feat: archive codex active and archived transcripts"
```

---

### Task 5: Resume-Based Reverse Sync Verification

**Files:**
- Test: `src/components/conversations/conversation-detail-panel-layout.test.tsx` or nearest existing conversation-detail test
- Test: `src/contexts/acp-connections-context.test.tsx`
- Modify: `src/components/conversations/conversation-detail-panel.tsx` only if tests expose a missing imported-session resume guard
- Modify: `src/contexts/acp-connections-context.tsx` only if tests expose session id loss before `acpConnect`
- Modify: `src/hooks/use-connection-lifecycle.ts` only if tests expose stale `sessionIdRef` behavior

**Interfaces:**
- Consumes: imported conversation `summary.external_id`
- Consumes: `useConnectionLifecycle({ sessionId })`
- Consumes: `acpConnect(agentType, workingDir, sessionId, ...)`
- Produces: imported Codex/Claude conversations auto-connect with native `sessionId`

- [ ] **Step 1: Add frontend test proving imported history passes external_id to lifecycle**

Locate the nearest existing test that renders `ConversationDetailPanel` with a persisted conversation detail. Add a test equivalent to:

```tsx
it("passes imported native external_id as sessionId when auto-connecting", async () => {
  mockUseConversationDetail.mockReturnValue({
    detail: {
      summary: {
        id: 42,
        external_id: "native-session-123",
        agent_type: "codex",
        title: "Imported Codex",
      },
      turns: [],
    },
    loading: false,
    error: null,
    acpLoadError: null,
  })

  renderConversationDetailPanel({
    conversationId: 42,
    agentType: "codex",
    workingDir: "G:\\\\CTF",
    isActive: true,
  })

  await waitFor(() => {
    expect(mockUseConnectionLifecycle).toHaveBeenCalledWith(
      expect.objectContaining({
        agentType: "codex",
        sessionId: "native-session-123",
        conversationId: 42,
      })
    )
  })
})
```

Use the repository's actual mocks and helper names. The assertion must prove `external_id` reaches `useConnectionLifecycle` as `sessionId`.

- [ ] **Step 2: Run test and verify current behavior**

Run the relevant Vitest file:

```powershell
pnpm test src/components/conversations/conversation-detail-panel-layout.test.tsx
```

Expected: PASS if the existing code already routes imported `external_id` correctly; FAIL if the test helper exposes a missing guard.

- [ ] **Step 3: Add ACP context test proving sessionId reaches acpConnect**

In `src/contexts/acp-connections-context.test.tsx`, add or adapt a test equivalent to:

```tsx
it("passes imported native session id to acpConnect", async () => {
  const { actions } = renderAcpConnectionsProvider()

  await act(async () => {
    await actions.connect(
      "conv-42",
      "codex",
      "G:\\\\CTF",
      "native-session-123",
      42
    )
  })

  expect(acpConnectMock).toHaveBeenCalledWith(
    "codex",
    "G:\\\\CTF",
    "native-session-123",
    expect.anything(),
    expect.anything()
  )
})
```

Use the repository's actual provider/test harness. The assertion must prove no layer strips `sessionId` before `acpConnect`.

- [ ] **Step 4: Run ACP context test**

Run:

```powershell
pnpm test src/contexts/acp-connections-context.test.tsx
```

Expected: PASS if existing code already supports resume-based reverse sync.

- [ ] **Step 5: Fix only if a test fails**

If Step 1 fails, keep `conversation-detail-panel.tsx` behavior aligned with this existing logic:

```tsx
const externalId =
  detail?.summary.external_id ?? runtimeSession?.externalId ?? undefined

const awaitingHistoricalSessionId =
  hasPersistedConversation && selectedAgent !== "cline" && detailLoading
```

If Step 3 fails, keep `acp-connections-context.tsx` behavior aligned with this existing call:

```tsx
const connectionId = await acpConnect(
  agentType,
  workingDir,
  sessionId,
  savedPrefs.modeId,
  savedPrefs.configValues
)
```

Do not add JSONL write-back. Do not add a new runtime mode unless these tests prove the existing path cannot support imported-session resume.

- [ ] **Step 6: Add user-facing note only if needed**

If current UI does not make native resume behavior discoverable, add a short tooltip or session details copy near the existing external id display:

```tsx
Native session id. When this conversation is continued, Codeg resumes this native session instead of rewriting the transcript directly.
```

Do not add a new button in this task unless existing UX has no way to continue imported sessions.

- [ ] **Step 7: Commit**

```powershell
git add src/components/conversations src/contexts src/hooks
git commit -m "test: cover imported native session resume"
```

---

### Task 6: Focused Verification and PR

**Files:**
- Modify: `docs/specs/2026-06-27-session-scan-fusion-design.md` only if implementation changed the accepted design
- No direct transcript write-back is allowed

**Interfaces:**
- Consumes: all previous tasks
- Produces: a branch ready to push and review

- [ ] **Step 1: Run focused Rust tests**

Run:

```powershell
cd src-tauri
cargo test codex
```

Expected: PASS.

- [ ] **Step 2: Run parser module tests**

Run:

```powershell
cd src-tauri
cargo test parsers
```

Expected: PASS or identify unrelated pre-existing failures before proceeding.

- [ ] **Step 3: Inspect diff**

Run:

```powershell
git diff --stat origin/main..HEAD
git diff origin/main..HEAD -- src-tauri/src/parsers/codex.rs src-tauri/src/parsers/mod.rs docs/specs/2026-06-27-session-scan-fusion-design.md
```

Expected: diff contains only parser/root/source/spec changes.

- [ ] **Step 4: Push branch to fork**

Run:

```powershell
git push -u fork fusion/session-scan-roots
```

Expected: branch pushed to `https://github.com/hight456789/codeg`.

- [ ] **Step 5: Create PR draft**

Run:

```powershell
gh pr create --repo xintaofei/codeg --head hight456789:fusion/session-scan-roots --base main --draft --title "Expand local Codex session scanning" --body "Adds multi-root Codex history scanning for active and archived sessions while preserving Codeg's existing UI. Reverse sync remains resume-based and does not write native JSONL directly."
```

Expected: draft PR URL printed.

- [ ] **Step 6: Record reverse sync behavior in PR body**

Add this paragraph to the PR body or follow-up issue if requested:

```markdown
Reverse sync behavior: imported Codex/Claude conversations continue through native resume flows by passing `external_id` as the native session id to Codeg's ACP connect path. Native tools remain the transcript writers. Codeg metadata stays in Codeg storage keyed by `(agent_type, external_id)`.
```

---

## Self-Review

- Spec coverage: Task 1-4 cover Codex active/archived/default/extra root scanning, dedupe, detail lookup, and archive/export source inclusion. Claude remains unchanged unless a test later proves path matching gaps, matching the spec. Task 5 covers same-branch resume-based reverse sync by testing the imported `external_id` path through frontend lifecycle and ACP connect.
- Placeholder scan: No TODO/TBD placeholders.
- Type consistency: `resolve_codex_session_roots()` and `resolve_codex_session_roots_from(...)` are defined before use; `CodexParser::with_session_roots(...)` is test-only; `candidate_jsonl_files()` is private to the parser.
