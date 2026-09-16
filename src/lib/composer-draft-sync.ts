"use client"

const CLIENT_ORIGIN_KEY = "codeg:client-origin"
const CONV_DRAFT_KEY = /^conv:(\d+)$/

/** Persisted conversations use `conv:<id>` as the localStorage draft key.
 *  New-tab drafts (`draft:<tabId>`) have no conversation row yet and must
 *  stay local-only — there is nothing authenticated to attach them to. */
export function conversationIdFromDraftKey(
  key: string | null | undefined
): number | null {
  if (!key) return null
  const match = CONV_DRAFT_KEY.exec(key)
  if (!match) return null
  const id = Number(match[1])
  return Number.isInteger(id) && id > 0 ? id : null
}

/** Stable per-browser origin id. Echoed on PUT so this client can ignore
 *  its own `composer-draft://changed` notify. Not a secret. */
export function getComposerClientOrigin(): string {
  if (typeof window === "undefined") return "ssr"
  try {
    const existing = window.localStorage.getItem(CLIENT_ORIGIN_KEY)
    if (existing && existing.length > 0 && existing.length <= 64) {
      return existing
    }
    const fresh =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `web-${Date.now().toString(36)}`
    window.localStorage.setItem(CLIENT_ORIGIN_KEY, fresh)
    return fresh
  } catch {
    return "anon"
  }
}

export function shouldApplyRemoteDraft(opts: {
  remoteRevision: number
  lastAppliedRevision: number
  remoteOrigin: string
  localOrigin: string
}): boolean {
  if (opts.remoteOrigin === opts.localOrigin) return false
  return opts.remoteRevision > opts.lastAppliedRevision
}
