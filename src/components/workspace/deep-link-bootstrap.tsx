"use client"

import { useCallback, useEffect, useRef } from "react"
import { toast } from "sonner"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { useTabContext } from "@/contexts/tab-context"
import type { AgentType } from "@/lib/types"

/**
 * Handles `/workspace?folderId=X&conversationId=Y&agent=Z` URLs.
 * Runs once after both folders and tabs have hydrated.
 */
export function DeepLinkBootstrap() {
  const { foldersHydrated, folders, addFolderToWorkspaceById, conversations } =
    useAppWorkspace()
  const { tabsHydrated, openTab } = useTabContext()
  const ranRef = useRef(false)

  useEffect(() => {
    if (ranRef.current) return
    if (!foldersHydrated || !tabsHydrated) return
    ranRef.current = true

    if (typeof window === "undefined") return

    const params = new URLSearchParams(window.location.search)
    const rawFolderId = params.get("folderId")
    const rawConversationId = params.get("conversationId")
    const rawAgent = params.get("agent") as AgentType | null

    if (!rawFolderId && !rawConversationId) return

    const clearUrl = () => {
      try {
        window.history.replaceState({}, "", "/workspace")
      } catch {
        /* ignore */
      }
    }

    void (async () => {
      try {
        const folderId = rawFolderId ? Number(rawFolderId) : null
        const conversationId = rawConversationId
          ? Number(rawConversationId)
          : null

        if (folderId == null || !Number.isFinite(folderId)) return
        if (conversationId == null || !Number.isFinite(conversationId)) return
        if (!rawAgent) return

        let folder = folders.find((f) => f.id === folderId)
        if (!folder) {
          try {
            folder = await addFolderToWorkspaceById(folderId)
          } catch (err) {
            console.error("[DeepLinkBootstrap] open folder failed:", err)
            toast.error("Unable to open linked folder")
            return
          }
        }

        const hasConv = conversations.some(
          (c) =>
            c.id === conversationId &&
            c.folder_id === folderId &&
            c.agent_type === rawAgent
        )
        if (!hasConv) {
          toast.error("Linked conversation not found")
          return
        }

        openTab(folderId, conversationId, rawAgent, true)
      } finally {
        clearUrl()
      }
    })()
  }, [
    foldersHydrated,
    tabsHydrated,
    folders,
    conversations,
    addFolderToWorkspaceById,
    openTab,
  ])

  return null
}

type FocusRequest = {
  folderId: number
  conversationId: number
  agent: AgentType
}

/**
 * Live counterpart to {@link DeepLinkBootstrap}: listens for the pet panel's
 * `workspace://focus-conversation` request (emitted by the `focus_conversation`
 * command after bringing the main window forward) and opens the conversation
 * via `openTab` — no URL reload, so in-memory tab/session state survives.
 *
 * Latest workspace state is held in a ref so the single subscription always
 * sees fresh state without re-subscribing on every change. A request that
 * arrives before folders/tabs hydrate is queued and replayed.
 */
export function PetFocusBridge() {
  const { foldersHydrated, folders, addFolderToWorkspaceById } =
    useAppWorkspace()
  const { tabsHydrated, openTab } = useTabContext()

  const stateRef = useRef({
    foldersHydrated,
    folders,
    addFolderToWorkspaceById,
    tabsHydrated,
    openTab,
  })
  useEffect(() => {
    stateRef.current = {
      foldersHydrated,
      folders,
      addFolderToWorkspaceById,
      tabsHydrated,
      openTab,
    }
  }, [
    foldersHydrated,
    folders,
    addFolderToWorkspaceById,
    tabsHydrated,
    openTab,
  ])

  // Holds the latest focus request until the workspace has hydrated. The event
  // is one-shot, so a pet-panel click during startup/reload (before folders &
  // tabs hydrate) must not be dropped — replay it once hydration completes.
  const pendingRef = useRef<FocusRequest | null>(null)

  const attempt = useCallback(() => {
    const req = pendingRef.current
    if (!req) return
    const s = stateRef.current
    if (!s.foldersHydrated || !s.tabsHydrated) return // wait for hydration
    // One-shot after hydration (mirrors DeepLinkBootstrap): clear before the
    // async work so a later state change can't double-open.
    pendingRef.current = null
    void (async () => {
      // Ensure the folder is in the workspace so the tab has a home.
      if (!s.folders.some((f) => f.id === req.folderId)) {
        try {
          await s.addFolderToWorkspaceById(req.folderId)
        } catch (err) {
          console.error("[PetFocusBridge] open folder failed:", err)
          return
        }
      }
      // The event is backend-originated for a live session, so the conversation
      // exists; open the tab directly and let its title/content hydrate. We do
      // NOT gate on the conversations list — it loads independently of folders,
      // and waiting on it (without a ready flag) would drop the request.
      stateRef.current.openTab(
        req.folderId,
        req.conversationId,
        req.agent,
        true
      )
    })()
  }, [])

  // Replay a queued request once hydration flips ready.
  useEffect(() => {
    attempt()
  }, [foldersHydrated, tabsHydrated, attempt])

  useEffect(() => {
    let dispose: (() => void) | null = null
    let cancelled = false

    void (async () => {
      try {
        const { getTransport } = await import("@/lib/transport")
        const off = await getTransport().subscribe<{
          folderId?: number
          conversationId?: number
          agent?: string
        }>("workspace://focus-conversation", (payload) => {
          const folderId = Number(payload?.folderId)
          const conversationId = Number(payload?.conversationId)
          const agent = payload?.agent as AgentType | undefined
          if (
            !Number.isFinite(folderId) ||
            !Number.isFinite(conversationId) ||
            !agent
          ) {
            return
          }
          pendingRef.current = { folderId, conversationId, agent }
          attempt()
        })
        if (cancelled) off()
        else dispose = off
      } catch (err) {
        console.warn("[PetFocusBridge] subscription failed:", err)
      }
    })()

    return () => {
      cancelled = true
      if (dispose) dispose()
    }
  }, [attempt])

  return null
}
