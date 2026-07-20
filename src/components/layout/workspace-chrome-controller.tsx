"use client"

import { useCallback, useEffect, useState } from "react"
import { openSettingsWindow } from "@/lib/api"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { isDesktop, openFileDialog } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { useSidebarContext } from "@/contexts/sidebar-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useTabActions } from "@/contexts/tab-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useSearchDialog } from "@/contexts/search-dialog-context"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { SearchCommandDialog } from "@/components/conversations/search-command-dialog"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"

/**
 * Headless owner of the workspace's global keyboard shortcuts and the two
 * dialogs the shortcuts summon (search, remote directory browser). These used
 * to live in the full-width `FolderTitleBar`; with the desktop title bar removed
 * (its buttons relocated into per-column edge clusters), this component keeps
 * the shortcuts + dialogs alive on BOTH desktop and mobile, independent of any
 * visible bar. Renders no visible chrome — only the dialogs.
 */
export function WorkspaceChromeController() {
  const openFolder = useAppWorkspaceStore((s) => s.openFolder)
  const { activeFolder } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const { toggle } = useSidebarContext()
  const { toggle: toggleAuxPanel } = useAuxPanelContext()
  const { toggle: toggleTerminal } = useTerminalContext()
  const { openNewConversationTab } = useTabActions()
  const { openConversations } = useWorkbenchRoute()
  const { shortcuts } = useShortcutSettings()
  // Search open-state is shared (see search-dialog-context): the trigger lives
  // in the sidebar, but this always-mounted controller owns the dialog and the
  // ⌘K shortcut so search works even when the sidebar is collapsed.
  const { open: searchOpen, setOpen: setSearchOpen } = useSearchDialog()
  const [browserOpen, setBrowserOpen] = useState(false)

  const handleOpenFolder = useCallback(async () => {
    // The native Tauri dialog browses the LOCAL filesystem, so when bound to a
    // remote workspace fall through to the in-app DirectoryBrowserDialog (which
    // browses the remote host via the proxied `list_directory_entries`).
    if (isDesktop() && getActiveRemoteConnectionId() === null) {
      try {
        const result = await openFileDialog({
          directory: true,
          multiple: false,
        })
        if (!result) return
        const selected = Array.isArray(result) ? result[0] : result
        await openFolder(selected)
      } catch (err) {
        console.error("[WorkspaceChromeController] failed to open folder:", err)
      }
    } else {
      setBrowserOpen(true)
    }
  }, [openFolder])

  const handleOpenSettings = useCallback(() => {
    openSettingsWindow().catch((err) => {
      console.error("[WorkspaceChromeController] failed to open settings:", err)
    })
  }, [])

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (matchShortcutEvent(e, shortcuts.toggle_search)) {
        e.preventDefault()
        setSearchOpen((prev) => !prev)
        return
      }
      if (matchShortcutEvent(e, shortcuts.toggle_sidebar)) {
        e.preventDefault()
        toggle()
        return
      }
      if (matchShortcutEvent(e, shortcuts.toggle_terminal)) {
        e.preventDefault()
        toggleTerminal()
        return
      }
      if (matchShortcutEvent(e, shortcuts.toggle_aux_panel)) {
        // The aux panel hosts the Session Details tab, so it's usable in chat
        // mode too; only suppress the toggle when there's nothing to show.
        if (!activeFolder && !isChatMode) return
        e.preventDefault()
        toggleAuxPanel()
        return
      }
      if (matchShortcutEvent(e, shortcuts.new_conversation)) {
        if (!activeFolder) return
        e.preventDefault()
        // Return to the conversation workspace if a route (e.g. Automations) was
        // covering the content region, else the new tab opens unseen.
        openConversations()
        openNewConversationTab(activeFolder.id, activeFolder.path)
        return
      }
      if (matchShortcutEvent(e, shortcuts.open_folder)) {
        e.preventDefault()
        void handleOpenFolder()
        return
      }
      if (matchShortcutEvent(e, shortcuts.open_settings)) {
        e.preventDefault()
        handleOpenSettings()
      }
    }
    document.addEventListener("keydown", handleKeyDown)
    return () => document.removeEventListener("keydown", handleKeyDown)
  }, [
    activeFolder,
    handleOpenFolder,
    handleOpenSettings,
    openConversations,
    openNewConversationTab,
    setSearchOpen,
    shortcuts,
    toggle,
    toggleAuxPanel,
    toggleTerminal,
    isChatMode,
  ])

  return (
    <>
      <SearchCommandDialog open={searchOpen} onOpenChange={setSearchOpen} />
      <DirectoryBrowserDialog
        open={browserOpen}
        onOpenChange={setBrowserOpen}
        onSelect={(path) => {
          openFolder(path).catch((err) => {
            console.error(
              "[WorkspaceChromeController] failed to open folder:",
              err
            )
          })
        }}
      />
    </>
  )
}
