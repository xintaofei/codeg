# Session Scan Fusion Design

## Goal

Keep Codeg's existing workspace UI and conversation model, while importing the useful local-history scan coverage from `desktop-cc-gui`.

The first implementation phase expands Codeg's backend parsers and import path so Codex and Claude history discovery finds more local sessions without adding a new catalog UI.

## Current Codeg Behavior

Codeg already has a clean conversation sidebar and local import flow. The backend discovers external conversations through `AgentParser` implementations and imports matches into Codeg's database.

The Codex parser currently scans only `resolve_codex_home_dir()/sessions` and only `rollout-*.jsonl` files. The external archive source also only includes `CODEX_HOME/sessions`.

Claude scans `CLAUDE_CONFIG_DIR/projects` and imports sessions whose persisted folder path matches the selected Codeg folder.

## Desired Behavior

Codex discovery should scan all relevant local history roots:

- Default Codex home `~/.codex/sessions`.
- Default Codex home `~/.codex/archived_sessions`.
- `$CODEX_HOME/sessions` when `CODEX_HOME` is set.
- `$CODEX_HOME/archived_sessions` when `CODEX_HOME` is set.
- Optional extra Codex homes from a Codeg-specific environment variable.
- Compatible provider-home layout: `<provider-home>/sessions` and `<provider-home>/archived_sessions`, when such homes are explicitly listed.

Claude discovery should keep Codeg's current UI model but improve path matching resilience:

- Continue scanning `~/.claude/projects` or `$CLAUDE_CONFIG_DIR/projects`.
- Preserve current import behavior for selected folders.
- Normalize Windows and Unix path variants consistently.
- Keep subagent files out of the root sidebar import unless already referenced by a parent detail view.

## Non-Goals

- Do not merge the desktop-cc-gui application UI into Codeg.
- Do not add desktop-cc-gui's full session catalog projection in phase one.
- Do not read another application's private SQLite state in phase one.
- Do not start or depend on Codex/Claude runtime processes just to list history.
- Do not change Codeg's conversation database schema unless tests prove it is necessary.

## Architecture

### Codex Session Roots

Add a small root-resolution layer in `src-tauri/src/parsers/codex.rs`.

It should return an ordered, deduplicated list of session directories. A directory is eligible when it exists and is one of:

- `<codex-home>/sessions`
- `<codex-home>/archived_sessions`
- A compatible provider-home session directory

The first source is still `CODEX_HOME` or `~/.codex`, preserving existing behavior. Additional sources must be additive.

Use `CODEG_CODEX_HOME_DIRS` as the first-phase opt-in mechanism for extra homes. It is a path-list environment variable using the platform separator (`;` on Windows, `:` on Unix). Each listed value is treated as a Codex home or provider home, and Codeg scans its `sessions` and `archived_sessions` children.

Do not automatically crawl another application's private storage directory in phase one.

### Codex Summary Parsing

Keep the existing `parse_jsonl_summary` and `parse_conversation_detail` logic. The change is mostly root enumeration and lookup.

Listing should:

- Walk every resolved root.
- Accept Codex JSONL rollout files.
- Deduplicate by `(agent_type, conversation_id)`.
- Prefer the newest file if the same session appears in both active and archived roots.
- Sort newest first as today.

Detail lookup should:

- Search all resolved roots.
- Prefer an exact session id match when possible.
- Fall back to filename containment only for compatibility with the existing behavior.

### External Archive Sources

Update `external_transcript_sources()` so Codex archive/export includes both `sessions` and `archived_sessions`.

If the existing `ExternalSource` shape cannot represent multiple Codex roots cleanly, add multiple Codex sources with distinct archive labels, or add a helper that expands agent roots before archiving. Prefer the smaller change that does not alter backup restore semantics for other agents.

### Claude Path Matching

Do not redesign the Claude parser in phase one. Add regression tests around Windows path normalization and project directory matching if current helpers already cover the behavior.

Only change Claude code if a test proves current matching misses a real path shape from the user's environment.

## UI Behavior

No first-phase UI redesign.

Existing Codeg surfaces continue to work:

- Workspace conversation sidebar.
- Search.
- Import local sessions button.
- Conversation detail viewer.

If additional roots are found, imported Codex sessions simply appear through the current conversation list.

## Error Handling

Missing roots are skipped silently.

Unreadable roots log a warning and do not fail the whole import.

Malformed JSONL lines continue to be skipped as the current parser does.

If all roots are missing, the parser returns an empty list, preserving current behavior.

## Testing

Add Rust tests for the Codex parser:

- Default `sessions` root still works.
- `archived_sessions` root is scanned.
- Duplicate session ids across active and archived roots are deduplicated.
- Detail lookup searches archived roots.
- `CODEX_HOME` override still wins.
- Root ordering is stable.

Add or preserve tests for `external_transcript_sources()` to prove Codex archive/export includes archived sessions.

Run at least:

```powershell
cargo test -p codeg codex
```

If package selection differs in this repository, run the nearest focused command under `src-tauri`.

## Rollout

Implement behind existing parser behavior with no feature flag. The change is additive and should only increase discovered sessions.

If performance becomes an issue for very large histories, add a later pagination or cached index layer. Do not add it in phase one.
