/**
 * Shared helpers for the install-stream hooks (agent / opencode-plugin /
 * officecli). Each hook appends one log line per backend event; a long
 * install can emit thousands, and the consuming panels render every line as a
 * DOM node. Without a cap the array (and the DOM list) grows unbounded and
 * each append is an O(n) copy, so a long install is O(n²) in total. Keep only
 * the tail — the recent lines are what a user actually scrolls to for
 * progress and the error at the end.
 */
export const MAX_INSTALL_LOG_LINES = 1000

/**
 * Append `line`, keeping at most the most recent `MAX_INSTALL_LOG_LINES`.
 * Bounded to O(MAX) per append regardless of how many lines were streamed.
 */
export function appendInstallLogLine(logs: string[], line: string): string[] {
  if (logs.length < MAX_INSTALL_LOG_LINES) {
    return [...logs, line]
  }
  return [...logs.slice(logs.length - MAX_INSTALL_LOG_LINES + 1), line]
}
