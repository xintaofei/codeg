/**
 * Find the first complete JSON object embedded in `raw` and parse it.
 *
 * Used to recover a broker envelope/report from host-specific wrappings —
 * notably Codex, which serializes the MCP `function_call_output` as
 * `"Wall time: N seconds\nOutput:\n<json>"` (sometimes with a trailing
 * terminal-cursor character such as `_`). A direct `JSON.parse(raw)` fails on
 * these because of the textual prefix/suffix; this scanner walks back from the
 * last `}` until a balanced span parses cleanly.
 *
 * Returns null when no `{...}` substring parses. Bounded iteration: each
 * attempt shrinks the candidate by one `}`, so worst-case work is linear in the
 * count of `}` characters in `raw`.
 */
export function extractEmbeddedJsonObject(
  raw: string
): Record<string, unknown> | null {
  const start = raw.indexOf("{")
  if (start < 0) return null
  let end = raw.lastIndexOf("}")
  while (end > start) {
    const candidate = raw.slice(start, end + 1)
    try {
      const v = JSON.parse(candidate)
      if (v && typeof v === "object" && !Array.isArray(v)) {
        return v as Record<string, unknown>
      }
    } catch {
      // try a shorter span
    }
    end = raw.lastIndexOf("}", end - 1)
    if (end < 0) break
  }
  return null
}
