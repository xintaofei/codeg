import type { AgentType } from "@/lib/types"

/**
 * A conversation opened in its own window (`open_conversation_window` on the
 * desktop, a named browser window in web mode).
 *
 * The window loads the ordinary `/workspace` route with the conversation in the
 * query string — the frontend is a static export, so there is no dynamic route
 * to open instead. `conversationWindow=1` is the part that matters: it marks
 * the webview as a DETACHED view of one conversation rather than a second
 * workspace, and the tab store keys its whole behaviour off it. A detached
 * window seeds the single tab it was opened for and never reads, writes or
 * mirrors the shared `opened_tabs` — without that, both windows would show the
 * same tab set and, because focus is mirrored across clients, would be dragged
 * onto the same conversation whenever either one switched tabs.
 */
export const CONVERSATION_WINDOW_PARAM = "conversationWindow"

export interface ConversationWindowTarget {
  folderId: number
  conversationId: number
  agentType: AgentType
}

/** A row id from the query string, or `null` for anything that is not one.
 *  Not `Number(...)` alone: that reads a missing parameter as `0`, which is
 *  finite and would seed a tab pointing at nothing. */
function rowId(raw: string | null): number | null {
  if (!raw) return null
  const value = Number(raw)
  return Number.isSafeInteger(value) && value > 0 ? value : null
}

/**
 * Read a conversation-window target out of a query string. Returns `null`
 * unless the marker AND a complete target are all present: a partial URL must
 * fall through to the ordinary workspace rather than open a window showing
 * nothing.
 */
export function parseConversationWindowTarget(
  search: string
): ConversationWindowTarget | null {
  const params = new URLSearchParams(search)
  if (params.get(CONVERSATION_WINDOW_PARAM) !== "1") return null

  const folderId = rowId(params.get("folderId"))
  const conversationId = rowId(params.get("conversationId"))
  const agent = params.get("agent")
  if (folderId == null || conversationId == null || !agent) return null

  return { folderId, conversationId, agentType: agent as AgentType }
}

/**
 * The target this webview was opened for, or `null` in the workspace and every
 * auxiliary window.
 *
 * Deliberately re-read on each call rather than memoized: the value is a
 * property of the window's URL, and `DeepLinkBootstrap` (which DOES rewrite the
 * URL for the plain `?folderId=…` deep link) skips a conversation window
 * precisely so this stays true for the window's whole life.
 */
export function conversationWindowTarget(): ConversationWindowTarget | null {
  if (typeof window === "undefined") return null
  return parseConversationWindowTarget(window.location.search)
}

/**
 * The route a conversation window loads. Mirrors `conversation_window_route`
 * in `src-tauri/src/commands/windows.rs`, which builds the same URL for the
 * desktop window; this copy serves the web fallback.
 */
export function conversationWindowRoute(
  target: ConversationWindowTarget
): string {
  const params = new URLSearchParams({
    [CONVERSATION_WINDOW_PARAM]: "1",
    folderId: String(target.folderId),
    conversationId: String(target.conversationId),
    agent: target.agentType,
  })
  return `/workspace?${params.toString()}`
}

/** Window name / Tauri label for a conversation. Keyed by the conversation id
 *  alone, so re-invoking "Open in New Window" focuses the existing window
 *  instead of stacking duplicates. Mirrors `conversation_window_label`. */
export function conversationWindowName(conversationId: number): string {
  return `conversation-${conversationId}`
}
