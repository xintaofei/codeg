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
import { useTabActions, useTabStore } from "@/contexts/tab-context"
import {
  useWorkspaceActions,
  useWorkspaceFileTabs,
  useWorkspaceView,
} from "@/contexts/workspace-context"
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
  const { openNewConversationTab, switchTab, closeTab } = useTabActions()
  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)
  // Tab-close/navigation shortcuts used to live in the visible tab strips.
  // Mobile no longer mounts those strips, so this always-mounted controller now
  // owns them too (see the keydown handler below).
  const { mode, activePane, filesMaximized } = useWorkspaceView()
  const { activeFileTabId } = useWorkspaceFileTabs()
  const { closeFileTab, closeAllFileTabs } = useWorkspaceActions()
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
        return
      }

      // Tab navigation + close. These once lived in the visible tab strips,
      // which mobile no longer mounts; owning them here keeps mod+w / mod+tab /
      // mod+shift+tab working at every width — and, crucially, keeps
      // preventDefault firing so mod+w never falls through to closing the OS
      // window. Routing mirrors the old split: conversation pane vs files pane.
      const conversationPaneActive =
        mode === "conversation" ||
        (mode === "fusion" && activePane === "conversation" && !filesMaximized)
      const filesPaneActive =
        mode === "fusion" && (activePane === "files" || filesMaximized)

      const isNextTab = matchShortcutEvent(e, shortcuts.next_tab)
      const isPrevTab = matchShortcutEvent(e, shortcuts.prev_tab)
      if (isNextTab || isPrevTab) {
        if (!conversationPaneActive) return
        if (tabs.length < 2 || !activeTabId) return
        const currentIndex = tabs.findIndex((tab) => tab.id === activeTabId)
        if (currentIndex === -1) return
        e.preventDefault()
        const offset = isNextTab ? 1 : -1
        const nextIndex = (currentIndex + offset + tabs.length) % tabs.length
        switchTab(tabs[nextIndex].id)
        return
      }

      if (matchShortcutEvent(e, shortcuts.close_all_file_tabs)) {
        if (!filesPaneActive) return
        e.preventDefault()
        closeAllFileTabs()
        return
      }

      if (matchShortcutEvent(e, shortcuts.close_current_tab)) {
        if (conversationPaneActive) {
          if (!activeTabId) return
          e.preventDefault()
          closeTab(activeTabId)
        } else if (filesPaneActive) {
          if (!activeFileTabId) return
          e.preventDefault()
          closeFileTab(activeFileTabId)
        }
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
    tabs,
    activeTabId,
    switchTab,
    closeTab,
    mode,
    activePane,
    filesMaximized,
    activeFileTabId,
    closeFileTab,
    closeAllFileTabs,
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
