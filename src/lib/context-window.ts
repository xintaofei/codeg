/**
 * Shared context-window percentage helpers.
 *
 * The below-composer context indicator (`composer-context-usage.tsx`) and the
 * Session Details dialog both show a context-window usage percentage; keeping
 * the precedence and the one-decimal formatting here ensures the two never
 * drift apart.
 */

/**
 * Resolve the context-window usage percentage from a session-stats snapshot,
 * mirroring the context indicator's session-stats branch: trust the backend-provided
 * `percent`, and only recompute from `used / max` when the backend figure is
 * absent. The result is clamped into 0–100. Returns `null` when nothing is
 * known.
 *
 * (The context indicator layers a live-connection tier on top of this — it prefers a
 * `used/max` recompute from the live ACP connection before falling back to the
 * session stats handled here — but the dialog has no live connection, so it maps
 * onto exactly this branch.)
 */
export function resolveContextWindowPercent(
  percent: number | null | undefined,
  used: number | null | undefined,
  max: number | null | undefined
): number | null {
  const raw =
    percent ??
    (used != null && max != null && max > 0 ? (used / max) * 100 : null)
  return raw == null ? null : Math.max(0, Math.min(100, raw))
}

/**
 * Format a context-window percentage keeping one decimal place (e.g. `87.3%`),
 * matching the below-composer context indicator. Returns `--` for an unknown value.
 */
export function formatContextWindowPercent(percent: number | null): string {
  if (percent == null) return "--"
  return `${percent.toFixed(1)}%`
}
