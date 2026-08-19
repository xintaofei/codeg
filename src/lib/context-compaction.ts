/**
 * Context-compaction tool-call detection.
 *
 * A context-compaction lifecycle arrives as an ACP `tool_call` whose
 * `_meta.contextCompaction` takes one of two shapes:
 *
 * - the legacy boolean marker `true` — codex-acp 1.1.3–1.2.x (#288) and the
 *   Grok bridge's `auto_compact_completed` synthesis (live via `connection.rs`,
 *   historical via `parsers/grok.rs`), which stamps sibling top-level
 *   `tokensBefore`/`tokensAfter` counts next to the marker;
 * - the versioned object `{version: 1, …}` — codex-acp 1.3.0+ (#396), whose
 *   schema reserves optional `trigger`, `preTokens`, `postTokens`, `durationMs`
 *   and `error` fields (1.3.0 itself emits the bare `{version: 1}`).
 *
 * BOTH shapes stay accepted long-term: the Grok bridge keeps emitting the
 * boolean. Claude emits no compaction meta at all (plain "Compacting…" text),
 * so this stays a codex/grok surface. Matching renders through the dedicated
 * `<ContextCompactionCard>` (a subtle status row, not the generic tool shell)
 * and must NOT fold into a "调用 N 个工具" tool-group.
 *
 * Kept in a dependency-free module so both the grouping pass
 * (`ai-elements-adapter.ts`) and the card/renderer can share it without an
 * import cycle (the card re-exports `ToolCallState` from the adapter).
 */
export function isContextCompactionMeta(meta: unknown): boolean {
  if (!meta || typeof meta !== "object") return false
  const marker = (meta as Record<string, unknown>).contextCompaction
  if (marker === true) return true
  if (!marker || typeof marker !== "object") return false
  const version = (marker as Record<string, unknown>).version
  return (
    typeof version === "number" && Number.isInteger(version) && version >= 1
  )
}

/**
 * The versioned `_meta.contextCompaction` payload (codex-acp 1.3.0+), or
 * `null` for the boolean-marker shape and for non-compaction meta. Callers
 * read individual fields leniently — the schema only reserves them, and every
 * one may be absent.
 */
export function contextCompactionPayload(
  meta: unknown
): Record<string, unknown> | null {
  if (!isContextCompactionMeta(meta)) return null
  const marker = (meta as Record<string, unknown>).contextCompaction
  return marker && typeof marker === "object"
    ? (marker as Record<string, unknown>)
    : null
}
