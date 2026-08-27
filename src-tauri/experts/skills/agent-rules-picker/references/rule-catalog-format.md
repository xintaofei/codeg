# Agent Rules Picker Reference

## Enable in Codeg

Agent Rules Picker is a built-in Codeg Expert. Open **Settings → Skill Packs →
Experts**, find **Agent Rules Picker**, and enable it for each agent that should
load it. Codeg links the shared Expert bundle into that agent's native skill
directory; it never rewrites the agent's instruction files.

Start a new conversation after enabling the Expert. Open it with
`/agent-rules-picker` or **+ → Experts → Agent Rules Picker** in the composer.
Codeg inserts a compact selection capsule together with the exact selected
rule text.

Codeg supports the built-in agents represented in the Skill Packs matrix and
custom agents that declare either the shared skills store or a dedicated skills
directory. An agent without a declared skill location cannot load this Expert.

## Optional rule catalog

Store optional rules in `.codeg/rules/*.md`, never in `AGENTS.md`,
`AGENTS.override.md`, or another agent's native instruction file. Native
instructions load outside this picker and cannot be deselected here. The panel
reports detected `AGENTS` sources without reading their contents, but that list
is not a complete inventory of every agent's native instruction conventions.

Catalog files are read in stable lexical filename order. Unmarked Markdown is
documentation and is never selected. Each selectable block uses an opening and
closing marker on their own lines:

```md
<!-- codeg-rule id="tests" name="Testing" default="on" -->
## Testing

- Run relevant tests after modifying code.
<!-- /codeg-rule -->

<!-- codeg-rule id="no-dependencies" name="No new dependencies" default="off" -->
## Dependencies

- Do not add dependencies without approval.
<!-- /codeg-rule -->
```

Opening attributes may appear in any order, but each must appear exactly once:

- `id`: a stable lower-case identifier matching `[a-z0-9][a-z0-9._-]*`
- `name`: a non-empty, trimmed human-readable label
- `default`: exactly `on` or `off`

The parser normalizes CRLF and CR line endings to LF and otherwise preserves
the rule body exactly. It rejects malformed attributes, duplicate IDs, nested
blocks, unmatched end markers, unterminated blocks, and catalog symlinks that
resolve outside the active workspace. Diagnostics identify the file and line.

The catalog `sourceHash` is a SHA-256 digest of rule IDs, names, defaults,
source paths, and normalized bodies in catalog order. Unmarked documentation
and line-ending style do not affect it.

## Managing profiles

Open **+ → Experts → Agent Rules Picker** to preview a selection, apply it once,
or save, rename, and delete profiles. Codeg refuses to apply or save when the
catalog changed after the panel opened; refresh the picker and review the new
preview in that case.

## Profiles and session scope

Profiles live in `.codeg/agent-rule-profiles.json` with version 1, a map of
names to `ruleIds` and `sourceHash`, and an optional `defaultProfile`. Writes are
deterministic and use atomic replacement. Unknown top-level fields are
preserved.

Committing this file shares profiles with the project. Keeping it untracked
makes profiles personal. Agent Rules Picker never changes `.gitignore` or
`.git/info/exclude`; choose and configure the desired policy yourself.

A confirmed selection applies as user-level context to the current skill
workflow and its subsequent delegated tasks. It cannot remove native
instructions or retroactively change the current session. Start a new top-level
session and invoke the saved profile again when that new session must use the
selection.
