"use client"

import { useCallback, useEffect, useRef } from "react"

import { getComposerDraft, putComposerDraft } from "@/lib/api"
import { draftSnapshotKey } from "@/lib/composer-draft-attachments"
import {
  getComposerClientOrigin,
  shouldApplyRemoteDraft,
} from "@/lib/composer-draft-sync"
import { onTransportReconnect, subscribe } from "@/lib/platform"
import {
  COMPOSER_DRAFT_CHANGED_EVENT,
  type ComposerDraftAttachment,
  type ComposerDraftChanged,
} from "@/lib/types"

export interface ComposerDraftSnapshot {
  text: string
  attachments: ComposerDraftAttachment[]
}

/** Two-way unsent-composer sync for a persisted conversation.
 *
 *  GET on open / reconnect. PUT after the local draft save (caller). WS
 *  notify is ids-only; the body is fetched over the authenticated GET.
 *  Own-origin echoes are ignored so typing does not bounce.
 *
 *  Attachments are jail/file refs, never bytes. A client that has not
 *  finished the first GET omits the field so it cannot wipe the other
 *  side's images. After that, the full snapshot is last-write-wins. */
export function useComposerDraftSync(opts: {
  conversationId: number | null
  enabled: boolean
  getLocalSnapshot: () => ComposerDraftSnapshot
  onRemote: (snapshot: ComposerDraftSnapshot) => void | Promise<void>
}): (snapshot: ComposerDraftSnapshot) => void {
  const { conversationId, enabled, getLocalSnapshot, onRemote } = opts
  const originRef = useRef(getComposerClientOrigin())
  const lastRevisionRef = useRef(0)
  const lastPutKeyRef = useRef<string | null>(null)
  const hydratedRef = useRef(false)
  const onRemoteRef = useRef(onRemote)
  const getLocalSnapshotRef = useRef(getLocalSnapshot)
  useEffect(() => {
    onRemoteRef.current = onRemote
    getLocalSnapshotRef.current = getLocalSnapshot
  })

  const applyIfNewer = useCallback(
    async (hint?: ComposerDraftChanged) => {
      if (!enabled || conversationId == null) return
      if (
        hint &&
        !shouldApplyRemoteDraft({
          remoteRevision: hint.revision,
          lastAppliedRevision: lastRevisionRef.current,
          remoteOrigin: hint.origin,
          localOrigin: originRef.current,
        })
      ) {
        if (hint.revision > lastRevisionRef.current) {
          lastRevisionRef.current = hint.revision
        }
        return
      }
      try {
        const remote = await getComposerDraft(conversationId)
        if (!remote) {
          hydratedRef.current = true
          if (!hint) {
            const local = getLocalSnapshotRef.current()
            if (local.text || local.attachments.length > 0) {
              lastPutKeyRef.current = null
              void putComposerDraft(
                conversationId,
                local.text,
                originRef.current,
                local.attachments
              ).then((result) => {
                lastPutKeyRef.current = draftSnapshotKey(
                  local.text,
                  local.attachments
                )
                lastRevisionRef.current = Math.max(
                  lastRevisionRef.current,
                  result.revision
                )
              })
            }
          }
          return
        }
        if (
          !shouldApplyRemoteDraft({
            remoteRevision: remote.revision,
            lastAppliedRevision: lastRevisionRef.current,
            remoteOrigin: remote.origin,
            localOrigin: originRef.current,
          })
        ) {
          lastRevisionRef.current = Math.max(
            lastRevisionRef.current,
            remote.revision
          )
          hydratedRef.current = true
          return
        }
        lastRevisionRef.current = remote.revision
        const attachments = remote.attachments ?? []
        lastPutKeyRef.current = draftSnapshotKey(remote.text, attachments)
        hydratedRef.current = true
        await onRemoteRef.current({ text: remote.text, attachments })
      } catch {
        // Draft sync is best-effort. A missed GET leaves the local box as-is.
        hydratedRef.current = true
      }
    },
    [conversationId, enabled]
  )

  useEffect(() => {
    lastRevisionRef.current = 0
    lastPutKeyRef.current = null
    hydratedRef.current = false
    if (!enabled || conversationId == null) return
    void applyIfNewer()
  }, [applyIfNewer, conversationId, enabled])

  useEffect(() => {
    if (!enabled || conversationId == null) return
    let disposed = false
    let unlisten: (() => void) | undefined
    void (async () => {
      const dispose = await subscribe<ComposerDraftChanged>(
        COMPOSER_DRAFT_CHANGED_EVENT,
        (change) => {
          if (change.conversation_id !== conversationId) return
          void applyIfNewer(change)
        }
      )
      if (disposed) {
        dispose()
        return
      }
      unlisten = dispose
    })()
    const offReconnect = onTransportReconnect(() => {
      void applyIfNewer()
    })
    return () => {
      disposed = true
      unlisten?.()
      offReconnect?.()
    }
  }, [applyIfNewer, conversationId, enabled])

  return useCallback(
    (snapshot: ComposerDraftSnapshot) => {
      if (!enabled || conversationId == null) return
      const key = draftSnapshotKey(snapshot.text, snapshot.attachments)
      if (lastPutKeyRef.current === key) return
      lastPutKeyRef.current = key
      void putComposerDraft(
        conversationId,
        snapshot.text,
        originRef.current,
        hydratedRef.current ? snapshot.attachments : undefined
      )
        .then((result) => {
          lastRevisionRef.current = Math.max(
            lastRevisionRef.current,
            result.revision
          )
        })
        .catch(() => {
          // Leave lastPutKey so we do not tight-loop a failing PUT.
        })
    },
    [conversationId, enabled]
  )
}
