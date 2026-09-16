"use client"

import { useCallback, useEffect, useRef } from "react"

import { getComposerDraft, putComposerDraft } from "@/lib/api"
import {
  getComposerClientOrigin,
  shouldApplyRemoteDraft,
} from "@/lib/composer-draft-sync"
import { onTransportReconnect, subscribe } from "@/lib/platform"
import {
  COMPOSER_DRAFT_CHANGED_EVENT,
  type ComposerDraftChanged,
} from "@/lib/types"

/** Two-way unsent-composer sync for a persisted conversation.
 *
 *  GET on open / reconnect. PUT after the local draft save (caller). WS
 *  notify is ids-only; the body is fetched over the authenticated GET.
 *  Own-origin echoes are ignored so typing does not bounce. */
export function useComposerDraftSync(opts: {
  conversationId: number | null
  enabled: boolean
  getLocalText: () => string
  onRemote: (text: string) => void
}): (text: string) => void {
  const { conversationId, enabled, getLocalText, onRemote } = opts
  const originRef = useRef(getComposerClientOrigin())
  const lastRevisionRef = useRef(0)
  const lastPutTextRef = useRef<string | null>(null)
  const onRemoteRef = useRef(onRemote)
  const getLocalTextRef = useRef(getLocalText)
  useEffect(() => {
    onRemoteRef.current = onRemote
    getLocalTextRef.current = getLocalText
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
          // No server row yet: publish this device's local cache so a
          // phone that opens the same chat can GET it. Skip when the
          // box is empty so we do not create tombstones for idle tabs.
          if (!hint) {
            const local = getLocalTextRef.current()
            if (local) {
              lastPutTextRef.current = null
              void putComposerDraft(
                conversationId,
                local,
                originRef.current
              ).then((result) => {
                lastPutTextRef.current = local
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
          return
        }
        lastRevisionRef.current = remote.revision
        lastPutTextRef.current = remote.text
        onRemoteRef.current(remote.text)
      } catch {
        // Draft sync is best-effort. A missed GET leaves the local box as-is.
      }
    },
    [conversationId, enabled]
  )

  useEffect(() => {
    lastRevisionRef.current = 0
    lastPutTextRef.current = null
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
    (text: string) => {
      if (!enabled || conversationId == null) return
      if (lastPutTextRef.current === text) return
      lastPutTextRef.current = text
      void putComposerDraft(conversationId, text, originRef.current)
        .then((result) => {
          lastRevisionRef.current = Math.max(
            lastRevisionRef.current,
            result.revision
          )
        })
        .catch(() => {
          // Leave lastPutText so we do not tight-loop a failing PUT.
        })
    },
    [conversationId, enabled]
  )
}
