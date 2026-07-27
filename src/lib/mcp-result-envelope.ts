/**
 * Peel the host envelopes that wrap an MCP `CallToolResult` on its way to a
 * tool card.
 *
 * codex-acp forwards EVERY MCP tool call's outcome to the ACP wire as
 *   `rawOutput = { result: <CallToolResult> | null, error: <string> | null }`
 * (its `createMcpRawOutput`), and codex's own rollout tags the same result under
 * a serde `{ Ok: … }` variant. Neither layer is part of the result the codeg-mcp
 * companion actually returned, so a card that reads a companion result has to
 * strip them first — otherwise the whole envelope falls through as opaque text
 * and the card renders raw JSON instead of the report inside it.
 *
 * The peel is deliberately narrow: a tool result is only ever a *child agent's*
 * arbitrary payload away from being mangled, so both the destination and the
 * failure case are positively identified (a `CallToolResult` under the wrapper
 * key; a `result`-bearing envelope for the error string) rather than pattern-
 * matched on key names alone. A payload that merely happens to own a `result` or
 * `error` key is left exactly as it was.
 *
 * `ask-question.ts::parseOutcomeJson` peels the same layers inline for its own
 * shapes; this module is the reusable form for the delegation cards.
 */

/**
 * Keys a host uses to nest the actual `CallToolResult`. `result` is codex-acp's
 * live-wire envelope; `Ok`/`ok` are the serde-tagged `Result` variant codex
 * writes into its rollout.
 */
const RESULT_ENVELOPE_KEYS = ["result", "Ok", "ok"] as const

/** Depth cap — one host layer plus a serde tag is the deepest shape seen. */
const MAX_PEEL_DEPTH = 3

export type PeeledMcpResult = {
  /** The `CallToolResult` reached by peeling, or the input unchanged when no
   *  host envelope was positively identified. */
  obj: Record<string, unknown>
  /** codex-acp's `rawOutput.error`, read ONLY from an envelope that carries a
   *  `result` key with nothing in it — i.e. the MCP call failed outright and
   *  that string is all there is to show. Null in every other case, including a
   *  payload that just happens to have an `error` field. */
  hostError: string | null
}

/**
 * Whether `value` is an MCP `CallToolResult` — the only thing worth peeling TO.
 * Requiring this of the destination (not just the wrapper key's presence) is
 * what keeps a child's own `{result: {...}}` payload from being unwrapped and
 * then misread as a report by the caller.
 */
function isCallToolResult(value: unknown): value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false
  const obj = value as Record<string, unknown>
  const structured = obj.structuredContent
  return (
    Array.isArray(obj.content) ||
    (typeof structured === "object" &&
      structured !== null &&
      !Array.isArray(structured))
  )
}

/**
 * The error string of a host FAILURE envelope — `{result: null, error: "…"}`.
 * codex-acp always emits both keys, so requiring a present-but-empty `result`
 * alongside the string is what separates a failed MCP call from a child payload
 * that merely has an `error` field of its own.
 */
function hostFailureError(obj: Record<string, unknown>): string | null {
  if (!("result" in obj)) return null
  if (obj.result !== null && obj.result !== undefined) return null
  const err = obj.error
  return typeof err === "string" && err.trim() ? err : null
}

/**
 * Strip host `{ result, error }` / `{ Ok }` layers from `obj`.
 *
 * `isResolvable` is the caller's own "I can already read this shape" predicate
 * (a report, a batch, an MCP content envelope, …). Peeling stops as soon as it
 * holds, so a `CallToolResult` that itself owns a `result` key is never unwrapped
 * out from under the caller.
 */
export function peelMcpResultEnvelope(
  obj: Record<string, unknown>,
  isResolvable: (candidate: Record<string, unknown>) => boolean
): PeeledMcpResult {
  let current = obj
  for (
    let depth = 0;
    depth < MAX_PEEL_DEPTH && !isResolvable(current);
    depth++
  ) {
    let next: Record<string, unknown> | null = null
    for (const key of RESULT_ENVELOPE_KEYS) {
      const value = current[key]
      if (isCallToolResult(value)) {
        next = value
        break
      }
    }
    // Nothing peelable left: this is either the payload itself or a host
    // envelope whose call failed before producing a result.
    if (!next) return { obj: current, hostError: hostFailureError(current) }
    current = next
  }
  return { obj: current, hostError: null }
}
