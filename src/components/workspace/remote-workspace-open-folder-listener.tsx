"use client"

import { useCallback, useEffect, useRef } from "react"
import { useSearchParams } from "next/navigation"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabActions, useTabStore } from "@/contexts/tab-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { toErrorMessage } from "@/lib/app-error"
import { isRemoteDesktopWindow } from "@/lib/platform"
import { REMOTE_OPEN_FOLDER_EVENT } from "@/lib/remote-workspace"

/**
 * URL param carrying a folder to open, set when this window was spawned by
 * `open_remote_workspace_folder`. MUST match `OPEN_FOLDER_PATH_PARAM` in
 * `src-tauri/src/commands/remote_workspace.rs`.
 */
export const OPEN_FOLDER_PATH_PARAM = "openFolderPath"

/**
 * Opens the folder a local window asked this remote workspace window to open.
 *
 * A folder belongs to the backend that owns its path, so the "Open Folder"
 * picker on the local machine can't open a remote folder itself — it raises
 * this window and hands the path over. Two delivery routes, one handler:
 *
 * - A URL param, when the window had to be spawned (an event can't reach a
 *   webview that doesn't exist yet).
 * - A `remote-open-folder` Tauri event, when the window was already open.
 *
 * Both are replayed after hydration: a spawn-time param is read before folders
 * or tabs exist, and an event that lands mid-startup would otherwise have
 * nowhere to put the folder.
 */
export function RemoteWorkspaceOpenFolderListener() {
  const t = useTranslations("Folder.workspaceDialog")
  const { openNewConversationTab } = useTabActions()
  const { openConversations } = useWorkbenchRoute()
  const searchParams = useSearchParams()
  const tabsHydrated = useTabStore((s) => s.tabsHydrated)
  // Only here to re-run the replay effect: `attempt` reads the store directly.
  const foldersHydrated = useAppWorkspaceStore((s) => s.foldersHydrated)

  const pendingRef = useRef<string | null>(null)

  // Read at run time via a ref so the subscription below never has to be torn
  // down and re-established when these change.
  const stateRef = useRef({
    tabsHydrated,
    openConversations,
    openNewConversationTab,
  })
  useEffect(() => {
    stateRef.current = {
      tabsHydrated,
      openConversations,
      openNewConversationTab,
    }
  }, [tabsHydrated, openConversations, openNewConversationTab])

  const attempt = useCallback(() => {
    const path = pendingRef.current
    if (!path) return
    const store = useAppWorkspaceStore.getState()
    if (!store.foldersHydrated || !stateRef.current.tabsHydrated) return
    // One-shot: clear before the async work so a later hydration change can't
    // open the same folder twice.
    pendingRef.current = null
    void (async () => {
      try {
        const detail = await store.openFolder(path)
        // Return to the conversation workspace if a route (e.g. Automations)
        // was covering the content region, else the new tab opens unseen.
        stateRef.current.openConversations()
        stateRef.current.openNewConversationTab(detail.id, detail.path)
      } catch (err) {
        toast.error(t("openFailed"), { description: toErrorMessage(err) })
      }
    })()
  }, [t])

  // Spawn-time request: queue it (folders aren't loaded yet) and strip the
  // param so a reload doesn't reopen the folder. Other params — notably the
  // remote identity — have to survive.
  useEffect(() => {
    const requested = searchParams.get(OPEN_FOLDER_PATH_PARAM)
    if (!requested) return
    pendingRef.current = requested
    try {
      const next = new URLSearchParams(window.location.search)
      next.delete(OPEN_FOLDER_PATH_PARAM)
      const query = next.toString()
      window.history.replaceState(
        {},
        "",
        query ? `/workspace?${query}` : "/workspace"
      )
    } catch {
      /* ignore */
    }
    attempt()
    // `searchParams` is read once on mount by design — this is a spawn-time
    // handoff, not a live route.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Both halves hydrate independently, so either can be the last to arrive.
  useEffect(() => {
    attempt()
  }, [foldersHydrated, tabsHydrated, attempt])

  // Live request: this window is already up, so the local window emits an
  // event instead of respawning it. Only a remote window is ever the target —
  // the local window hands folders *out*, it never receives them.
  useEffect(() => {
    if (!isRemoteDesktopWindow()) return
    let cancelled = false
    let unlisten: UnlistenFn | undefined

    void (async () => {
      try {
        const off = await listen<{ path?: string }>(
          REMOTE_OPEN_FOLDER_EVENT,
          (event) => {
            const path = event.payload?.path
            if (!path) return
            pendingRef.current = path
            attempt()
          }
        )
        if (cancelled) off()
        else unlisten = off
      } catch (err) {
        console.warn(
          "[RemoteWorkspaceOpenFolderListener] subscription failed:",
          err
        )
      }
    })()

    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [attempt])

  return null
}
