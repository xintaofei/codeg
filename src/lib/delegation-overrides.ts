import type { JSONContent } from "@tiptap/core"

import type { ReferenceMeta } from "@/components/chat/composer/types"
import type {
  AgentDelegationDefaults,
  AgentType,
  PromptDraft,
} from "@/lib/types"

/**
 * Draft-scoped delegation config overrides for `@Agent` mentions.
 *
 * The composer's per-mention config popover stores the user's pick on the
 * mention node's `ReferenceMeta.delegationOverride` (so it survives the doc's
 * JSON round-trips); these pure helpers collect those picks into the
 * `PromptDraft.delegation_overrides` map the send path ships to the backend,
 * and maintain such maps imperatively for callers that hold a draft-level
 * value (queue-edit seeding, the popover itself).
 *
 * An override object with nothing set counts as ABSENCE everywhere here — a
 * "reset to global/default" edit removes the entry instead of storing an empty
 * `{}`, so an untouched mention produces a draft with no `delegation_overrides`
 * field at all and the wire payload stays byte-identical to today's.
 */

/** A shallow copy with `mode_id`/`config_values` normalized to present shapes. */
function normalizeOverride(
  value: AgentDelegationDefaults
): AgentDelegationDefaults {
  return {
    ...(value.mode_id ? { mode_id: value.mode_id } : {}),
    config_values: { ...(value.config_values ?? {}) },
  }
}

/** True when the override carries nothing — the "use global/default" shape. */
export function isEmptyDelegationOverride(
  value: AgentDelegationDefaults | null | undefined
): boolean {
  if (!value) return true
  const modeEmpty = value.mode_id == null || value.mode_id.length === 0
  const configEmpty =
    value.config_values == null || Object.keys(value.config_values).length === 0
  return modeEmpty && configEmpty
}

/**
 * Read the override stored on one reference node's `meta`, or `null` when the
 * meta carries none (or a non-object / empty one — pasted HTML is an untrusted
 * input, so anything malformed degrades to "no override").
 */
export function readReferenceDelegationOverride(
  meta: ReferenceMeta | null | undefined
): AgentDelegationDefaults | null {
  const raw = meta?.delegationOverride
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null
  if (isEmptyDelegationOverride(raw)) return null
  const config_values: Record<string, string> = {}
  if (raw.config_values && typeof raw.config_values === "object") {
    for (const [key, value] of Object.entries(raw.config_values)) {
      if (typeof value === "string" && value.length > 0) {
        config_values[key] = value
      }
    }
  }
  return normalizeOverride({ mode_id: raw.mode_id ?? undefined, config_values })
}

/**
 * Immutably set or clear the override on a `ReferenceMeta`. `null` (or an
 * empty value) removes the field; other fields on the meta are preserved.
 */
export function writeReferenceDelegationOverride(
  meta: ReferenceMeta | null | undefined,
  value: AgentDelegationDefaults | null
): ReferenceMeta {
  const base: ReferenceMeta = { ...(meta ?? {}) }
  if (isEmptyDelegationOverride(value)) {
    delete base.delegationOverride
  } else {
    base.delegationOverride = normalizeOverride(
      value as AgentDelegationDefaults
    )
  }
  return base
}

/**
 * Collect the per-agent overrides stored on a composer document's agent
 * mention nodes. One draft-level value per agent: when the same agent is
 * mentioned more than once, the LAST occurrence in document order wins
 * (deterministic; the popover also writes every duplicate in one edit, so the
 * doc-order tiebreak only matters for docs edited out-of-band). Returns
 * `undefined` when the doc carries no overrides — callers omit the
 * `delegation_overrides` field entirely rather than shipping an empty map.
 */
export function collectDelegationOverrides(
  doc: JSONContent | null | undefined
): Partial<Record<AgentType, AgentDelegationDefaults>> | undefined {
  const collected: Partial<Record<AgentType, AgentDelegationDefaults>> = {}
  if (!doc) return undefined

  const walk = (node: JSONContent | undefined): void => {
    if (!node) return
    if (node.type === "reference") {
      const attrs = node.attrs as
        | { refType?: string; id?: string; meta?: ReferenceMeta | null }
        | undefined
      if (attrs?.refType === "agent" && attrs.id) {
        const override = readReferenceDelegationOverride(attrs.meta)
        if (override) collected[attrs.id as AgentType] = override
        else delete collected[attrs.id as AgentType]
      }
    }
    for (const child of node.content ?? []) walk(child)
  }
  walk(doc)

  if (Object.keys(collected).length === 0) return undefined
  return collected
}

/**
 * Merge one agent's draft-scoped override into a `PromptDraft`, immutably.
 * An empty `value` clears the entry (back to the global default). Later calls
 * for the same agent overwrite earlier ones — the most recent explicit edit is
 * authoritative.
 */
export function mergeDelegationOverride(
  draft: PromptDraft,
  agentType: AgentType,
  value: AgentDelegationDefaults | null
): PromptDraft {
  if (isEmptyDelegationOverride(value)) {
    return clearDelegationOverride(draft, agentType)
  }
  const overrides = { ...(draft.delegation_overrides ?? {}) }
  overrides[agentType] = normalizeOverride(value as AgentDelegationDefaults)
  return { ...draft, delegation_overrides: overrides }
}

/**
 * Remove one agent's draft-scoped override from a `PromptDraft`, immutably.
 * Dropping the last entry removes the `delegation_overrides` field itself.
 */
export function clearDelegationOverride(
  draft: PromptDraft,
  agentType: AgentType
): PromptDraft {
  if (
    !draft.delegation_overrides ||
    !(agentType in draft.delegation_overrides)
  ) {
    return draft
  }
  const overrides = { ...draft.delegation_overrides }
  delete overrides[agentType]
  const next: PromptDraft = { ...draft }
  if (Object.keys(overrides).length === 0) {
    delete next.delegation_overrides
  } else {
    next.delegation_overrides = overrides
  }
  return next
}

/**
 * Human-readable summary of what a delegation to one agent would use, from its
 * (global-default or override) config: the mode id and the model-ish config
 * value, most informative first, capped at two parts — this renders inline in
 * the `@` panel's agent row, where only a few characters fit. Empty array =
 * nothing pinned, the agent's native default applies.
 */
export function delegationDefaultsSummaryParts(
  defaults: AgentDelegationDefaults | null | undefined,
  maxParts = 2
): string[] {
  if (isEmptyDelegationOverride(defaults)) return []
  const parts: string[] = []
  if (defaults?.mode_id) parts.push(defaults.mode_id)
  let modelPinned = false
  for (const [key, value] of Object.entries(defaults?.config_values ?? {})) {
    if (value.length === 0) continue
    const isModel = key.toLowerCase().includes("model")
    if (isModel) {
      // Model reads first (it's what people check first); mode shifts over.
      parts.splice(0, 0, value)
      modelPinned = true
      if (parts.length >= maxParts) break
      continue
    }
    if (modelPinned || parts.length >= maxParts) continue
    parts.push(value)
  }
  return parts.slice(0, maxParts)
}

/**
 * Write the given per-agent overrides onto a composer document's agent mention
 * badges IN PLACE (pure: returns a new tree, or `null` when nothing changed).
 *
 * This is how a queued item's delegation overrides survive a queue edit: the
 * queue stores only prompt prose, and the edit's block → badge hydration
 * (`restoreBlocksIntoEditor` → `textToSeededInlineContent`) rebuilds agent
 * badges WITHOUT their meta — so the host re-applies the queued overrides
 * right after hydration. Once the badges carry the values, the document is the
 * single source of truth: `collectDelegationOverrides` reads them back on
 * send, and a user reset / mention deletion is expressed in the doc itself
 * (cleared meta / no badge), with no seed layered on top at save time.
 */
export function withDelegationOverrides(
  doc: JSONContent,
  overrides: Partial<Record<AgentType, AgentDelegationDefaults>>
): JSONContent | null {
  let changed = false

  const walk = (node: JSONContent): JSONContent => {
    if (node.type === "reference") {
      const attrs = node.attrs as
        | { refType?: string; id?: string; meta?: ReferenceMeta | null }
        | undefined
      const agent = attrs?.refType === "agent" ? (attrs.id as AgentType) : null
      const incoming = agent ? overrides[agent] : undefined
      if (agent && !isEmptyDelegationOverride(incoming)) {
        const current = readReferenceDelegationOverride(attrs?.meta)
        if (
          !current ||
          JSON.stringify(current) !==
            JSON.stringify(
              normalizeOverride(incoming as AgentDelegationDefaults)
            )
        ) {
          changed = true
          return {
            ...node,
            attrs: {
              ...attrs,
              meta: writeReferenceDelegationOverride(attrs?.meta, incoming!),
            },
          }
        }
      }
      return node
    }
    if (!node.content?.length) return node
    const children = node.content.map(walk)
    return children.some((child, i) => child !== node.content![i])
      ? { ...node, content: children }
      : node
  }

  const next = walk(doc)
  return changed ? next : null
}
