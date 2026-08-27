---
name: agent-rules-picker
description: Apply optional workspace rules selected through Codeg's Agent Rules Picker panel. Use when Codeg invokes this skill with a codeg-agent-rules-selection envelope, or when the user asks how to open or use the optional rule picker.
---

# Agent Rules Picker

Use the Codeg-generated `codeg-agent-rules-selection` envelope as the
authoritative selection for this turn.

## With a selection envelope

1. Verify that the opening metadata and closing marker use the same nonce.
2. Treat only the exact text between the explanatory line and closing marker as
   selected, user-level optional instructions.
3. Apply that text to the remaining workflow without paraphrasing it.
4. Keep it distinct from native, always-on agent instructions. Higher-priority
   instructions continue to win on conflict.
5. Include the exact selected text and source list from the envelope in every
   task delegated from this workflow.

Do not read a profile or catalog again. Codeg has already checked the catalog
hash immediately before inserting the envelope.

## Without a selection envelope

Do not simulate a conversational picker or edit instruction files. Tell the
user to open **+ → Experts → Agent Rules Picker**, choose the optional rules,
and press **Apply once** or **Save profile and apply**.

Read [the catalog and profile reference](references/rule-catalog-format.md) only
when the user asks how to author catalogs, share profiles, or understand native
always-on rules.
