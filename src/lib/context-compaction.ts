/**
 * Context-compaction tool-call detection.
 *
 * A context-compaction lifecycle arrives as an ACP `tool_call` tagged with
 * `_meta.contextCompaction === true` — codex-acp emits it natively (#288), and
 * the Grok bridge synthesizes the same shape for `auto_compact_completed` (live
 * via `connection.rs` and historical via `parsers/grok.rs`). It renders through
 * the dedicated `<ContextCompactionCard>` (a subtle status row, not the generic
 * tool shell) and must NOT fold into a "调用 N 个工具" tool-group.
 *
 * Kept in a dependency-free module so both the grouping pass
 * (`ai-elements-adapter.ts`) and the card/renderer can share it without an
 * import cycle (the card re-exports `ToolCallState` from the adapter).
 */
export function isContextCompactionMeta(meta: unknown): boolean {
  return (
    !!meta &&
    typeof meta === "object" &&
    (meta as Record<string, unknown>).contextCompaction === true
  )
}
