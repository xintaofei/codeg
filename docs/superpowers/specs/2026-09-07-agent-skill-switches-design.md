# Per-Agent Skill Switches Design

## Goal

Add an availability switch to Settings > Skills so the same skill can be
enabled for one agent and disabled for another, matching the per-agent meaning
of MCP assignments.

An off switch must change what the selected agent can discover. Hiding a skill
only from Codeg autocomplete is not sufficient.

## Current Behavior

Codeg discovers skills by scanning each agent's native global or project skill
directories. Some directories belong to one agent, such as
`~/.codex/skills`. Others, especially `.agents/skills`, are read by several
agents. The settings list currently exposes only discovered entries and has no
disabled state.

## User Experience

- Every skill row has an availability switch for the currently selected agent.
- An enabled skill remains discoverable by that agent. A disabled skill remains
  visible in Settings so it can be previewed, edited, deleted, or re-enabled,
  but it is omitted from Codeg skill autocomplete and from the agent's native
  scan roots.
- A switch shows an in-progress state while the filesystem operation runs.
- On failure, the authoritative list is reloaded, the switch returns to its
  prior state, and a localized error toast is shown.
- CLI-owned read-only skills remain visible but cannot be toggled.
- A skill whose shared installation cannot be separated without changing
  another configured agent is marked non-toggleable instead of pretending the
  operation succeeded.
- A new or reconnected agent session is required when an already-running agent
  caches its skill inventory.

## Storage Model

No database flag is authoritative. The filesystem remains the source of truth
because the agent CLIs scan it directly.

For each native skill root, Codeg uses a sibling vault that is outside the
agent's scan path:

```text
~/.codex/skills/pdf/SKILL.md
~/.codex/.skills.codeg-disabled/pdf/SKILL.md
```

Directory skills and flat Markdown skills retain their original entry name and
layout in the vault. Renaming within the same parent filesystem makes a private
skill toggle reversible and preserves all supporting assets and symlink
identity.

### Private Root

Disabling moves the entry from the native root to its sibling vault. Enabling
moves it back. Destination collisions are rejected before mutation.

### Shared Root

Before hiding an entry from a shared root, Codeg identifies every configured
agent that currently relies on that root. For each peer other than the agent
being disabled, it creates a link in an agent-unique native root. The shared
entry is then moved into the shared root's sibling vault and becomes the
canonical link target.

Example:

```text
before:
  ~/.agents/skills/pdf                  # Codex and Gemini can both see it

after disabling only Gemini:
  ~/.agents/.skills.codeg-disabled/pdf  # canonical content, not scanned
  ~/.codex/skills/pdf -> canonical      # Codex still sees it
  ~/.gemini/skills/pdf                  # absent, so Gemini does not see it
```

If a peer has no unique native skill root, or a conflicting entry blocks a
required link, Codeg rejects the operation before moving the shared source.
This preserves the per-agent contract for all agents managed by Codeg. Tools
outside Codeg that independently consume `.agents/skills` are outside this
assignment model.

## API And Data Flow

`AgentSkillItem` gains:

```text
enabled: bool
can_toggle: bool
```

The list command scans active roots first and disabled vaults second. Active
entries win when the same ID occurs more than once. This means a peer link is
reported enabled even though its canonical source is held in a shared vault.

A new command is available over both Tauri and Axum transports:

```text
acp_set_agent_skill_enabled(
  agent_type,
  scope,
  skill_id,
  workspace_path,
  enabled
) -> AgentSkillItem
```

Read, save, and delete operations resolve both active and disabled entries so
turning a skill off does not make it unmanageable. Saving a new skill creates
an enabled entry. The frontend invalidates its skill cache after every toggle,
and the generic `useAgentSkills` hook returns enabled entries only.

## Consistency And Failure Handling

- Skill mutations are serialized inside the backend.
- Validation and destination/link collision checks run before the first move.
- Shared fan-out records links created by the operation. If a later step fails,
  those links are removed and a moved source is restored.
- Repeated enable or disable requests are idempotent.
- Symbolic links are moved as links, never followed and copied during private
  disable operations.
- Built-in system paths retain the existing backend write protection.

## Tests

Rust tests cover private directory and flat-file toggles, disabled discovery,
idempotency, shared fan-out, peer isolation, collision refusal, rollback, and
read-only rejection. Existing skill storage tests continue to pin each agent's
native roots.

Frontend tests cover switch state, the exact toggle request, cache invalidation,
authoritative reload, disabled autocomplete filtering, read-only/non-toggleable
rows, and failure rollback. The final gate runs focused tests followed by the
repository's frontend lint/test/build and Rust desktop/server checks appropriate
to the touched shared backend.
