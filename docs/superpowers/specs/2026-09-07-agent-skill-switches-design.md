# Per-Agent Skill Switches Design

## Goal

Add an availability switch to Settings > Skills so the same skill can be
enabled for one agent and disabled for another, matching the per-agent meaning
of MCP assignments.

An off switch must change what the selected agent can discover. Hiding a skill
only from Codeg autocomplete is not sufficient.

## Current Behavior And Safety Boundary

Codeg discovers skills by scanning each agent's native global or project skill
directories. Some directories belong to one agent, such as
`~/.claude/skills`. Others, especially `.agents/skills`, are read by several
agents and by tools outside Codeg.

Codeg may only move an entry when its active root belongs to exactly one agent
and scope. A shared root remains owned by the upstream installer: Codeg neither
moves its entries nor creates replacement links in peer roots. This is a
deliberate scope limit, not a transient error.

## User Experience

- Every skill row has an availability switch for the currently selected agent.
- An enabled skill remains discoverable by that agent. A disabled skill remains
  visible in Settings so it can be previewed, edited, deleted, or re-enabled,
  but it is omitted from Codeg skill autocomplete and from the agent's
  effective native discovery result.
- A switch shows an in-progress state while the filesystem operation runs.
- On failure, the authoritative list is reloaded, the switch returns to its
  prior state, and a localized error toast is shown.
- For non-Codex agents, CLI-owned read-only skills remain visible but cannot be
  toggled. Codex system skills remain toggleable because Codeg changes only
  Codex's official availability configuration, not the skill files.
- A skill whose shared installation cannot be separated without changing
  another consumer is marked non-toggleable with a shared-root explanation.
- Skills linked from Codeg's central skill-pack store are marked as managed by
  the Experts, Office, Science, or Custom Skills page. The generic switch never
  moves those links.
- Every unavailable switch carries a machine-readable reason so the UI can
  distinguish read-only content, a shared root, Codeg-managed content, a
  storage collision, an unsafe link, a cross-filesystem move, legacy state, and
  a Codex-wide configuration override.
- A new or reconnected agent session is required when an already-running agent
  caches its skill inventory.

## Agent-Specific Control Models

No database flag is authoritative. Codeg changes the native state consumed by
each agent so its own discovery result remains the source of truth.

### Codex Native Configuration

Codeg does not move Codex skill files. It reads and atomically updates
`CODEX_HOME/config.toml` using Codex's official `skills.config` entries. Rules
are evaluated in file order and the last valid matching selector wins. A valid
entry has exactly one selector: `path` or `name`. Entries containing both (or
neither) are preserved but ignored when calculating state, matching current
Codex behavior. When a broader or later `name` selector would override the
requested state, Codeg appends a path-only rule. A repeated toggle updates only
an existing path-only rule instead of modifying a mixed selector or growing the
configuration indefinitely.

The selector name comes from the `SKILL.md` frontmatter, including the plugin
namespace used by Codex. Enabled plugin manifests under the Codex plugin cache
are included in discovery. `skills.bundled.enabled = false` is authoritative:
bundled system skills are shown disabled and cannot be individually re-enabled
until that global setting is enabled. Project, user, plugin, and system files
remain in place. A new Codex session is required because an existing session
may have cached its skill inventory.

### Filesystem Isolation For Other Agents

Agents without a native availability configuration continue to use filesystem
isolation because their CLIs scan native skill roots directly.

Each private global root uses a sibling Codeg vault with agent and root
identity. Each private project root uses Codeg's resolved data directory with
workspace, agent, and root identity:

```text
~/.claude/skills/pdf/SKILL.md
~/.claude/.codeg-skill-vaults/v1/claude_code/<root-key>/pdf/SKILL.md

<CODEG_DATA_DIR>/skill-vaults/v1/<workspace-key>/<agent>/<root-key>/pdf/SKILL.md
```

Directory skills and flat Markdown skills retain their original entry name and
layout. Before advertising the switch, Codeg verifies that source and vault are
on the same filesystem; otherwise it returns `cross_filesystem`. Codeg never
falls back to a recursive copy because that would change symlink identity and
weaken crash behavior.

### Private Root

Disabling renames the entry from the native root into its exact vault. Enabling
renames it back. Destination collisions and symlinks that would change meaning
after the move are rejected before mutation. A private entry is also rejected
when another configured Agent root contains a directory, Markdown, or
`SKILL.md` link into that entry; moving it would otherwise leave the peer with
a dangling link. Preflight simulates every missing destination-root ancestor
that `create_dir_all` creates, so safe internal and stable absolute links remain
toggleable even before a multi-level vault exists.

Incoming-link discovery records both lexical and resolved identities for every
link target and each intermediate target in a chain, with traversal bounded at
40 links. A relative target is resolved from the link's resolved physical
parent, not merely from the path used to enter a symlinked directory. This keeps
an internal relative `SKILL.md -> docs/body.md` link valid when the outer Skill
entry is itself a symlink. An absolute link that points back through the active
entry is rejected because moving that entry would break the reference. An
unreadable, dangling, cyclic, or overlong chain makes the topology scan
incomplete, and private moves are then conservatively unavailable.

### Shared Root

Shared roots such as global or project `.agents/skills` are listed normally,
but their generic per-agent switch is unavailable with reason `shared_root`.
Codeg leaves both the canonical entry and every peer root unchanged. An agent
that later gains a native declarative availability setting can opt into an
agent-specific implementation without changing this filesystem rule.

Vaults produced by pre-release fan-out builds have no trustworthy ownership
manifest. Codeg may show those entries as `legacy_state` for recovery, but it
does not infer ownership, relink peers, delete them, or move them automatically.

## API And Data Flow

`AgentSkillItem` gains:

```text
enabled: bool
can_toggle: bool
toggle_reason: read_only | shared_root | managed_elsewhere |
               storage_conflict | unsafe_link | cross_filesystem |
               legacy_state | bundled_disabled | config_error | null
```

For non-Codex agents, the list command scans each active root and its exact
private vault once. Active entries win when the same ID occurs more than once.
Root topology and incoming peer links are indexed once per request rather than
rescanned once per skill. An incomplete peer-link scan conservatively disables
private moves. For Codex, no peer-link scan is needed: the list command overlays
native configuration state on user, project, plugin, and system skills while
retaining each original path.

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

The resolved Codeg data directory is passed explicitly by both Tauri and Axum
to list, toggle, read, save, and delete core functions. Read, save, and delete
resolve both active and private-vault entries so turning a skill off does not
make it unmanageable. Saving a new skill creates an enabled entry. Plugin,
system, shared legacy, and Codeg-managed content remains protected from writes.
The frontend invalidates its skill cache after every toggle, and the generic
`useAgentSkills` hook returns enabled entries only.

## Consistency And Failure Handling

- Skill mutations are serialized inside the backend.
- Validation, ownership, filesystem, destination, and link checks run before a
  private move.
- Shared roots are never mutated by the availability command.
- Repeated enable or disable requests are idempotent.
- Symbolic links are moved as links, never followed and copied during private
  disable operations.
- Built-in system paths retain the existing backend content-write protection.
  Codex availability remains configurable because toggling writes only the
  Codex config file.

## Tests

Rust tests cover Codex selector validity, frontmatter names, root inline tables,
plugin namespaces, bundled disablement, stable paths, atomic config writes, and
new-session behavior. They also cover private directory and flat-file toggles,
project vault placement, root identity, disabled discovery, idempotency,
shared-root refusal without mutation, Codeg-managed links, collision refusal,
incoming directory/content/Markdown links, fresh multi-level vault link
round-trips, cross-filesystem refusal, and non-Codex read-only rejection.
Existing skill storage tests continue to pin each agent's native roots.

Frontend tests cover switch state, the exact toggle request, cache invalidation,
authoritative reload, disabled autocomplete filtering, read-only/non-toggleable
rows, and failure rollback. The final gate runs focused tests followed by the
repository's frontend lint/test/build and Rust desktop/server checks appropriate
to the touched shared backend.
