import type { JSONContent } from "@tiptap/core"
import { describe, expect, it } from "vitest"

import type { ReferenceMeta } from "@/components/chat/composer/types"
import type {
  AgentDelegationDefaults,
  PromptDraft,
  PromptInputBlock,
} from "@/lib/types"

import {
  clearDelegationOverride,
  collectDelegationOverrides,
  delegationDefaultsSummaryParts,
  isEmptyDelegationOverride,
  mergeDelegationOverride,
  readReferenceDelegationOverride,
  withDelegationOverrides,
  writeReferenceDelegationOverride,
} from "./delegation-overrides"
import { blocksToRestoredDraft } from "@/components/chat/composer/from-prompt-blocks"
import { textToSeededInlineContent } from "@/components/chat/composer/plain-text-content"

function agentMention(
  id: string,
  override: AgentDelegationDefaults | null
): JSONContent {
  const meta: ReferenceMeta | null = override
    ? ({ agentType: id, delegationOverride: override } as ReferenceMeta)
    : ({ agentType: id } as ReferenceMeta)
  return {
    type: "reference",
    attrs: { refType: "agent", id, label: id, uri: null, meta },
  }
}

function doc(...content: JSONContent[]): JSONContent {
  return {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [...content, { type: "text", text: "hi" }],
      },
    ],
  }
}

function draft(overrides?: PromptDraft["delegation_overrides"]): PromptDraft {
  return {
    blocks: [{ type: "text", text: "@claude_code do it" }],
    displayText: "@Claude Code do it",
    ...(overrides ? { delegation_overrides: overrides } : {}),
  }
}

const modeOverride: AgentDelegationDefaults = {
  mode_id: "plan",
  config_values: {},
}
const modelOverride: AgentDelegationDefaults = {
  config_values: { model: "claude-opus-4-1" },
}

describe("isEmptyDelegationOverride", () => {
  it("treats null, empty mode and empty config as absence", () => {
    expect(isEmptyDelegationOverride(null)).toBe(true)
    expect(isEmptyDelegationOverride(undefined)).toBe(true)
    expect(isEmptyDelegationOverride({ config_values: {} })).toBe(true)
    expect(isEmptyDelegationOverride({ mode_id: "", config_values: {} })).toBe(
      true
    )
  })

  it("treats any populated field as present", () => {
    expect(isEmptyDelegationOverride(modeOverride)).toBe(false)
    expect(isEmptyDelegationOverride(modelOverride)).toBe(false)
  })
})

describe("readReferenceDelegationOverride", () => {
  it("round-trips a stored override", () => {
    const meta = writeReferenceDelegationOverride(null, modelOverride)
    expect(readReferenceDelegationOverride(meta)).toEqual(modelOverride)
  })

  it("returns null for absent, malformed, or empty values", () => {
    expect(readReferenceDelegationOverride(null)).toBeNull()
    expect(readReferenceDelegationOverride({})).toBeNull()
    expect(
      readReferenceDelegationOverride({
        delegationOverride: { config_values: {} },
      })
    ).toBeNull()
    // Non-string config values (untrusted pasted payload) are dropped.
    expect(
      readReferenceDelegationOverride({
        delegationOverride: {
          config_values: { model: 42 } as unknown as Record<string, string>,
        },
      })
    ).toEqual({ config_values: {} })
  })
})

describe("writeReferenceDelegationOverride", () => {
  it("preserves unrelated meta fields when writing", () => {
    const meta: ReferenceMeta = { agentType: "claude_code", available: true }
    const next = writeReferenceDelegationOverride(meta, modeOverride)
    expect(next).toEqual({
      agentType: "claude_code",
      available: true,
      delegationOverride: modeOverride,
    })
    expect(meta).not.toHaveProperty("delegationOverride")
  })

  it("removes the field on reset instead of storing an empty object", () => {
    const meta = writeReferenceDelegationOverride(
      { agentType: "claude_code" },
      modeOverride
    )
    const reset = writeReferenceDelegationOverride(meta, null)
    expect(reset).toEqual({ agentType: "claude_code" })
    expect(
      writeReferenceDelegationOverride(meta, { config_values: {} })
    ).toEqual({ agentType: "claude_code" })
  })
})

describe("collectDelegationOverrides", () => {
  it("returns undefined for an untouched doc (no meta overrides)", () => {
    expect(
      collectDelegationOverrides(doc(agentMention("claude_code", null)))
    ).toBeUndefined()
    expect(collectDelegationOverrides(doc())).toBeUndefined()
    expect(collectDelegationOverrides(null)).toBeUndefined()
  })

  it("ignores non-agent references", () => {
    const fileRef: JSONContent = {
      type: "reference",
      attrs: {
        refType: "file",
        id: "src/app.ts",
        label: "app.ts",
        uri: "file:///repo/src/app.ts",
        meta: { fileKind: "file" },
      },
    }
    expect(collectDelegationOverrides(doc(fileRef))).toBeUndefined()
  })

  it("collects one entry per mentioned agent", () => {
    const collected = collectDelegationOverrides(
      doc(
        agentMention("claude_code", modeOverride),
        agentMention("codex", modelOverride)
      )
    )
    expect(collected).toEqual({
      claude_code: modeOverride,
      codex: modelOverride,
    })
  })

  it("picks the last occurrence in doc order for duplicate mentions", () => {
    const collected = collectDelegationOverrides(
      doc(
        agentMention("claude_code", modeOverride),
        agentMention("claude_code", modelOverride)
      )
    )
    expect(collected).toEqual({ claude_code: modelOverride })
  })

  it("drops the entry when the last duplicate was reset to global", () => {
    const collected = collectDelegationOverrides(
      doc(
        agentMention("claude_code", modeOverride),
        agentMention("claude_code", null)
      )
    )
    expect(collected).toBeUndefined()
  })
})

describe("mergeDelegationOverride", () => {
  it("adds an override without mutating the input draft", () => {
    const base = draft()
    const next = mergeDelegationOverride(base, "claude_code", modeOverride)
    expect(next.delegation_overrides).toEqual({ claude_code: modeOverride })
    expect(base).not.toHaveProperty("delegation_overrides")
  })

  it("preserves unrelated agents", () => {
    const base = mergeDelegationOverride(draft(), "codex", modelOverride)
    const next = mergeDelegationOverride(base, "claude_code", modeOverride)
    expect(next.delegation_overrides).toEqual({
      codex: modelOverride,
      claude_code: modeOverride,
    })
  })

  it("lets the most recent explicit edit win for the same agent", () => {
    const base = mergeDelegationOverride(draft(), "claude_code", modeOverride)
    const next = mergeDelegationOverride(base, "claude_code", modelOverride)
    expect(next.delegation_overrides).toEqual({ claude_code: modelOverride })
  })

  it("clears back to global on an empty value", () => {
    const base = mergeDelegationOverride(draft(), "claude_code", modeOverride)
    const reset = mergeDelegationOverride(base, "claude_code", null)
    expect(reset).not.toHaveProperty("delegation_overrides")
    expect(reset.blocks).toEqual(base.blocks)
  })

  it("clearing an absent agent returns the same draft", () => {
    const base = draft()
    expect(clearDelegationOverride(base, "claude_code")).toBe(base)
  })

  it("removes the whole field when the last entry is cleared", () => {
    const base = mergeDelegationOverride(
      mergeDelegationOverride(draft(), "codex", modelOverride),
      "claude_code",
      modeOverride
    )
    const one = clearDelegationOverride(base, "codex")
    expect(one.delegation_overrides).toEqual({ claude_code: modeOverride })
    const none = clearDelegationOverride(one, "claude_code")
    expect(none).not.toHaveProperty("delegation_overrides")
    expect(none).not.toBe(base)
  })
})

/**
 * Reviewer regression: a queue edit must PRESERVE its delegation overrides.
 * Real path, no stubs: the queued blocks are exactly what a send serialized
 * (agent mention reduced to prose) → blocksToRestoredDraft (the queue-edit
 * restore) → textToSeededInlineContent (badge hydration) → the hydration-time
 * override injection (`withDelegationOverrides`) → what buildDraft collects.
 */
function restoredDocFromBlocks(blocks: PromptInputBlock[]): JSONContent {
  const { segments } = blocksToRestoredDraft(blocks)
  const content = segments.flatMap((segment) =>
    segment.kind === "text"
      ? textToSeededInlineContent(segment.text)
      : [{ type: "reference", attrs: segment.attrs } as JSONContent]
  )
  return { type: "doc", content: [{ type: "paragraph", content }] }
}

describe("queue-edit override round-trip (real hydration path)", () => {
  const queuedBlocks: PromptInputBlock[] = [
    {
      type: "text",
      text: "[@Codex](codeg://agent/codex) refactor the parser",
    },
  ]
  const queuedOverrides = {
    codex: { config_values: { model: "gpt-5.2-codex" } },
  }

  it("block hydration rebuilds the agent badge WITHOUT its meta", () => {
    // The precondition the injection exists for: the restored badge has no
    // delegationOverride, because the queue only stored prompt prose.
    const restored = restoredDocFromBlocks(queuedBlocks)
    expect(collectDelegationOverrides(restored)).toBeUndefined()
    const badge = findBadge(restored)
    expect(badge?.attrs).toMatchObject({ refType: "agent", id: "codex" })
  })

  it("injecting the queued overrides makes an untouched edit preserve them", () => {
    const restored = restoredDocFromBlocks(queuedBlocks)
    const injected = withDelegationOverrides(restored, queuedOverrides)
    expect(injected).not.toBeNull()
    expect(collectDelegationOverrides(injected!)).toEqual(queuedOverrides)
  })

  it("injection is idempotent and skips badges the doc already agrees with", () => {
    const restored = restoredDocFromBlocks(queuedBlocks)
    const once = withDelegationOverrides(restored, queuedOverrides)!
    // A second pass (e.g. a re-hydrate) changes nothing.
    expect(withDelegationOverrides(once, queuedOverrides)).toBeNull()
    expect(collectDelegationOverrides(once)).toEqual(queuedOverrides)
  })

  it("a reset expressed in the doc (cleared meta) is NOT resurrected", () => {
    // User re-mentioned @Codex and clicked "use global default": the badge's
    // meta was cleared in the doc. Injection happens only at hydration time,
    // so a later save reads the doc and sends nothing.
    const d = doc(agentMention("codex", null))
    expect(withDelegationOverrides(d, queuedOverrides)).not.toBeNull()
    // …but only hydration applies the seed; collectDelegationOverrides — what
    // buildDraft reads — still sees no override on the badge.
    expect(collectDelegationOverrides(d)).toBeUndefined()
  })

  it("a deleted mention simply has nothing to collect", () => {
    const restored = restoredDocFromBlocks([
      { type: "text", text: "refactor the parser" },
    ])
    expect(collectDelegationOverrides(restored)).toBeUndefined()
    expect(withDelegationOverrides(restored, queuedOverrides)).toBeNull()
  })
})

function findBadge(node: JSONContent): JSONContent | undefined {
  if (node.type === "reference") return node
  for (const child of node.content ?? []) {
    const found = findBadge(child)
    if (found) return found
  }
  return undefined
}

describe("delegationDefaultsSummaryParts (readonly @ panel summary)", () => {
  it("returns empty for nothing pinned (row shows the Agent default label)", () => {
    expect(delegationDefaultsSummaryParts(null)).toEqual([])
    expect(delegationDefaultsSummaryParts({ config_values: {} })).toEqual([])
  })

  it("puts the model first, then the mode", () => {
    expect(
      delegationDefaultsSummaryParts({
        mode_id: "plan",
        config_values: { model: "claude-opus-4-1", permission_mode: "plan" },
      })
    ).toEqual(["claude-opus-4-1", "plan"])
  })

  it("caps at two parts to fit the row", () => {
    expect(
      delegationDefaultsSummaryParts({
        mode_id: "plan",
        config_values: {
          model: "claude-opus-4-1",
          permission_mode: "plan",
          extra: "x",
        },
      })
    ).toHaveLength(2)
  })
})

describe("one agent, one config (explicit non-goal: per-mention overrides)", () => {
  it("applies the same override to EVERY badge of the agent in one doc", () => {
    // Documented limitation: two mentions of the same agent share ONE
    // override. The popover writes all same-agent badges in a single
    // transaction (message-input handleDelegationChange); the hydration-time
    // injector has the same shape. There is no per-mention wire schema, and
    // no last-write-wins ambiguity — every badge of the agent carries the
    // identical value by construction.
    const doc: JSONContent = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            agentMention("claude_code", null),
            { type: "text", text: " then " },
            agentMention("claude_code", null),
          ],
        },
      ],
    }
    const next = withDelegationOverrides(doc, {
      claude_code: modelOverride,
    })
    expect(next).not.toBeNull()
    const badges: JSONContent[] = []
    const walk = (node: JSONContent): void => {
      if (node.type === "reference") badges.push(node)
      for (const child of node.content ?? []) walk(child)
    }
    walk(next!)
    expect(badges).toHaveLength(2)
    for (const badge of badges) {
      expect(
        readReferenceDelegationOverride(
          (badge.attrs as { meta: ReferenceMeta }).meta
        )
      ).toEqual(modelOverride)
    }
  })
})
