"use client"

import type { JSONContent } from "@tiptap/core"

import { sanitizeComposerDraftDoc } from "./composer-draft-sanitize"

interface PersistedDraftState {
  text: string
}

/** v2 draft payload: the composer's Tiptap document (preserves reference badges,
 *  which a Markdown round-trip would downgrade to plain links). */
interface PersistedDraftStateV2 {
  doc: JSONContent
}

const STORAGE_PREFIX = "codeg:message-input-draft:v1"
const STORAGE_PREFIX_V2 = "codeg:message-input-draft:v2"
const draftTextCache = new Map<string, string>()
const draftDocCache = new Map<string, JSONContent>()
const pendingPersistDrafts = new Map<string, string>()
const pendingPersistDocs = new Map<string, JSONContent>()
let idlePersistHandle: number | null = null
let persistenceListenersBound = false

function isMobileMemoryOnlyDraft(): boolean {
  return (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in window &&
    /Android|iPhone|iPad|iPod/i.test(navigator.userAgent)
  )
}

function storageKeyForDraftKey(draftKey: string): string {
  return `${STORAGE_PREFIX}:${draftKey}`
}

function storageKeyForDraftKeyV2(draftKey: string): string {
  return `${STORAGE_PREFIX_V2}:${draftKey}`
}

/** A persisted v2 payload's `doc` is only trusted when it is a ProseMirror doc
 *  root (a non-array object whose `type` is "doc"); anything else (corrupt or
 *  partial payload, array, …) is rejected so we fall back to v1 / null rather
 *  than hand garbage to the editor. */
function isTiptapDoc(value: unknown): value is JSONContent {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    (value as { type?: unknown }).type === "doc"
  )
}

function flushPendingDraftPersistence(): void {
  if (typeof window === "undefined") return
  if (isMobileMemoryOnlyDraft()) {
    pendingPersistDrafts.clear()
    pendingPersistDocs.clear()
    idlePersistHandle = null
    return
  }
  if (pendingPersistDrafts.size === 0 && pendingPersistDocs.size === 0) {
    idlePersistHandle = null
    return
  }

  const textEntries = Array.from(pendingPersistDrafts.entries())
  pendingPersistDrafts.clear()
  const docEntries = Array.from(pendingPersistDocs.entries())
  pendingPersistDocs.clear()
  idlePersistHandle = null

  for (const [draftKey, text] of textEntries) {
    try {
      localStorage.setItem(
        storageKeyForDraftKey(draftKey),
        JSON.stringify({ text })
      )
    } catch {
      // Ignore storage quota/permission failures.
    }
  }
  for (const [draftKey, doc] of docEntries) {
    let persisted = false
    try {
      localStorage.setItem(
        storageKeyForDraftKeyV2(draftKey),
        JSON.stringify({ doc } satisfies PersistedDraftStateV2)
      )
      persisted = true
    } catch {
      // Keep the legacy v1 draft as a fallback when the v2 write fails
      // (quota / permission / serialization), so the draft is not lost.
    }
    // Only retire the legacy v1 draft once the v2 document is durably written.
    if (persisted) clearMessageInputDraft(draftKey)
  }
}

function cancelScheduledDraftPersistence(): void {
  if (typeof window === "undefined") return
  if (idlePersistHandle == null) return
  if ("cancelIdleCallback" in window) {
    window.cancelIdleCallback(idlePersistHandle)
  }
  idlePersistHandle = null
}

function ensurePersistenceListeners(): void {
  if (typeof window === "undefined") return
  if (persistenceListenersBound) return
  persistenceListenersBound = true

  const flushNow = () => {
    cancelScheduledDraftPersistence()
    flushPendingDraftPersistence()
  }

  window.addEventListener("pagehide", flushNow)
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") {
      flushNow()
    }
  })
}

function scheduleDraftPersistence(): void {
  if (typeof window === "undefined") return
  if (idlePersistHandle != null) return

  ensurePersistenceListeners()
  if ("requestIdleCallback" in window) {
    idlePersistHandle = window.requestIdleCallback(() => {
      flushPendingDraftPersistence()
    })
    return
  }

  // Fallback for runtimes without requestIdleCallback.
  flushPendingDraftPersistence()
}

export function buildConversationDraftStorageKey(
  conversationId: number
): string {
  return `conv:${conversationId}`
}

export function buildNewConversationDraftStorageKey(): string {
  return "new"
}

export function loadMessageInputDraft(draftKey: string): string | null {
  const cached = draftTextCache.get(draftKey)
  if (typeof cached === "string") return cached
  if (typeof window === "undefined") return null
  if (isMobileMemoryOnlyDraft()) return null

  try {
    const raw = localStorage.getItem(storageKeyForDraftKey(draftKey))
    if (!raw) return null
    const parsed = JSON.parse(raw) as Partial<PersistedDraftState>
    if (typeof parsed.text !== "string") return null
    draftTextCache.set(draftKey, parsed.text)
    return parsed.text
  } catch {
    return null
  }
}

export function saveMessageInputDraft(draftKey: string, text: string): void {
  if (text.length === 0) {
    clearMessageInputDraft(draftKey)
    return
  }

  if (draftTextCache.get(draftKey) === text) return
  draftTextCache.set(draftKey, text)
  if (typeof window === "undefined") return
  if (isMobileMemoryOnlyDraft()) return

  pendingPersistDrafts.set(draftKey, text)
  scheduleDraftPersistence()
}

export function clearMessageInputDraft(draftKey: string): void {
  draftTextCache.delete(draftKey)
  pendingPersistDrafts.delete(draftKey)
  if (typeof window === "undefined") return

  try {
    localStorage.removeItem(storageKeyForDraftKey(draftKey))
  } catch {
    /* ignore */
  }
}

/**
 * Result of loading a v2 draft: a parsed composer document, a legacy v1 Markdown
 * string to hydrate via `setText` (migration), or null when no draft exists.
 */
export type LoadedDraftV2 =
  | { kind: "doc"; doc: JSONContent }
  | { kind: "legacyMarkdown"; markdown: string }
  | null

/**
 * Load the persisted composer draft for a key. Prefers the v2 document; falls
 * back to a v1 text draft (returned as `legacyMarkdown` for the host to hydrate
 * as Markdown), or null. The v1 draft is left in place on read — it is only
 * cleared once a v2 document is actually saved ({@link saveMessageInputDraftV2}),
 * so an unedited migration is never lost.
 */
export function loadMessageInputDraftV2(draftKey: string): LoadedDraftV2 {
  const cached = draftDocCache.get(draftKey)
  if (cached) return { kind: "doc", doc: cached }

  if (typeof window !== "undefined") {
    if (isMobileMemoryOnlyDraft()) return null
    try {
      const raw = localStorage.getItem(storageKeyForDraftKeyV2(draftKey))
      if (raw) {
        const parsed = JSON.parse(raw) as Partial<PersistedDraftStateV2>
        if (isTiptapDoc(parsed?.doc)) {
          // Down-migrate a draft saved by the old rich-text composer: a stale
          // heading/list/codeBlock/mark would otherwise make `setContent`
          // silently discard the WHOLE doc (losing the user's text and badges).
          // A draft already in the plain-text schema passes through untouched.
          const doc = sanitizeComposerDraftDoc(parsed.doc)
          draftDocCache.set(draftKey, doc)
          return { kind: "doc", doc }
        }
      }
    } catch {
      // Fall through to the legacy v1 draft.
    }
  }

  const legacy = loadMessageInputDraft(draftKey)
  if (legacy != null && legacy.length > 0) {
    return { kind: "legacyMarkdown", markdown: legacy }
  }
  return null
}

/**
 * Persist the composer document for a key (v2). The host calls this only for a
 * non-empty document and {@link clearMessageInputDraftV2} otherwise. The in-memory
 * `draftDocCache` makes the v2 document win immediately; any legacy v1 text draft
 * is only removed once the v2 write is durably flushed (see the flush path), so a
 * deferred write that later fails cannot lose the draft.
 */
export function saveMessageInputDraftV2(
  draftKey: string,
  doc: JSONContent
): void {
  draftDocCache.set(draftKey, doc)
  if (typeof window === "undefined") return
  if (isMobileMemoryOnlyDraft()) return

  pendingPersistDocs.set(draftKey, doc)
  scheduleDraftPersistence()
}

export function clearMessageInputDraftV2(draftKey: string): void {
  draftDocCache.delete(draftKey)
  pendingPersistDocs.delete(draftKey)
  // Also drop any legacy v1 draft so a cleared composer stays cleared.
  clearMessageInputDraft(draftKey)
  if (typeof window === "undefined") return

  try {
    localStorage.removeItem(storageKeyForDraftKeyV2(draftKey))
  } catch {
    /* ignore */
  }
}
