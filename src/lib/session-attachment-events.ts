import type { DbConversationSummary } from "@/lib/types"

export const ATTACH_FILE_TO_SESSION_EVENT = "codeg:attach-file-to-session"

export interface AttachFileToSessionDetail {
  tabId: string
  path: string
  /**
   * Optional 1-based, inclusive line span to attach as a ranged file badge
   * (`foo.ts:10-25`). Omitted by whole-file callers (file tree, git changes);
   * supplied by the editor's "add selection to chat". When present the consumer
   * encodes it into the badge uri (`file://…#L10-25`) and label.
   */
  range?: { start: number; end: number }
}

export function emitAttachFileToSession(
  detail: AttachFileToSessionDetail
): void {
  if (typeof window === "undefined") return
  window.dispatchEvent(
    new CustomEvent<AttachFileToSessionDetail>(ATTACH_FILE_TO_SESSION_EVENT, {
      detail,
    })
  )
}

export const ATTACH_SESSION_TO_SESSION_EVENT = "codeg:attach-session-to-session"

export interface AttachSessionToSessionDetail {
  /** The conversation tab whose composer receives the mention badge. */
  tabId: string
  /**
   * The conversation being mentioned. Carried whole (rather than by id) so the
   * consumer builds the badge through the same `sessionToSuggestion` adapter the
   * `@` panel uses — one source of truth for the label / `codeg://session/<id>`
   * uri / agent + status + branch meta.
   */
  conversation: DbConversationSummary
}

export function emitAttachSessionToSession(
  detail: AttachSessionToSessionDetail
): void {
  if (typeof window === "undefined") return
  window.dispatchEvent(
    new CustomEvent<AttachSessionToSessionDetail>(
      ATTACH_SESSION_TO_SESSION_EVENT,
      { detail }
    )
  )
}

export const APPEND_TEXT_TO_SESSION_EVENT = "codeg:append-text-to-session"

export interface AppendTextToSessionDetail {
  tabId: string
  text: string
}

export function emitAppendTextToSession(
  detail: AppendTextToSessionDetail
): void {
  if (typeof window === "undefined") return
  window.dispatchEvent(
    new CustomEvent<AppendTextToSessionDetail>(APPEND_TEXT_TO_SESSION_EVENT, {
      detail,
    })
  )
}
