"use client"

import { useCallback } from "react"
import {
  Menu,
  PanelRight,
  Settings,
  SquarePen,
  SquareTerminal,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { openSettingsWindow } from "@/lib/api"
import { isDesktop } from "@/lib/platform"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { usePlatform } from "@/hooks/use-platform"
import { Button } from "@/components/ui/button"
import { useSidebarContext } from "@/contexts/sidebar-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useTabActions } from "@/contexts/tab-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { MAC_TRAFFIC_LIGHT_INSET } from "@/lib/window-chrome"
import { cn } from "@/lib/utils"
import { WindowControls } from "./window-controls"

/**
 * Mobile-only workspace title bar (`h-10`, matching the desktop column strip).
 *
 * On desktop the full-width title bar was removed: its buttons were relocated
 * into fixed corner overlays (`LeftEdgeChrome` / `RightEdgeChrome`) and its
 * global shortcuts + dialogs moved to `WorkspaceChromeController`. This bar is
 * mounted only on the mobile path (`FolderLayoutShell`), where the sidebar / aux
 * / terminal are `Sheet` overlays that need a compact bar to summon them.
 *
 * It mirrors the desktop chrome directly (rather than via `AppTitleBar`): the
 * left holds the sidebar toggle + a new-conversation shortcut; the right holds
 * the same terminal / aux / settings cluster as `RightEdgeChrome` (active
 * `bg-accent`, same disabled predicates). The empty middle is a full-height
 * `data-tauri-drag-region` filler so the window drags by it — plus a macOS
 * traffic-light inset and the Windows/Linux caption buttons (`WindowControls`
 * self-nulls elsewhere), exactly like the desktop edges.
 */
export function FolderTitleBar() {
  const tTitleBar = useTranslations("Folder.folderTitleBar")
  const tCard = useTranslations("Folder.conversationCard")
  const { isOpen: sidebarOpen, toggle } = useSidebarContext()
  const { isOpen: auxPanelOpen, toggle: toggleAuxPanel } = useAuxPanelContext()
  const { isOpen: terminalOpen, toggle: toggleTerminal } = useTerminalContext()
  const { activeFolder } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const { openNewConversationTab, openChatModeTab } = useTabActions()
  const { openConversations } = useWorkbenchRoute()
  const { isMac } = usePlatform()
  const showMacInset = isMac && isDesktop()

  const handleOpenSettings = useCallback(() => {
    openSettingsWindow().catch((err) => {
      console.error("[FolderTitleBar] failed to open settings:", err)
    })
  }, [])

  // Mirror the sidebar's "New chat": return to the conversation workspace, then
  // start a new conversation in the active folder — or folderless chat mode when
  // there's none, so this entry point is never a dead end.
  const handleNewConversation = useCallback(() => {
    openConversations()
    if (!activeFolder) {
      openChatModeTab()
      return
    }
    openNewConversationTab(activeFolder.id, activeFolder.path)
  }, [activeFolder, openChatModeTab, openNewConversationTab, openConversations])

  return (
    <div className="flex h-10 shrink-0 items-stretch border-b border-border ws-chrome-border bg-muted/70 select-none">
      {/* macOS traffic-light inset — a window-drag region so the left cluster
          clears the OS-drawn lights. */}
      {showMacInset && (
        <div
          data-tauri-drag-region
          className="h-full shrink-0"
          style={{ width: MAC_TRAFFIC_LIGHT_INSET }}
        />
      )}
      {/* Left cluster: sidebar toggle + new conversation. */}
      <div className="flex shrink-0 items-center gap-1 pl-2">
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 shrink-0"
          onClick={toggle}
          title={tTitleBar(sidebarOpen ? "hideSidebar" : "showSidebar")}
          aria-label={tTitleBar(sidebarOpen ? "hideSidebar" : "showSidebar")}
        >
          <Menu className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 shrink-0"
          onClick={handleNewConversation}
          title={tCard("newConversation")}
          aria-label={tCard("newConversation")}
        >
          <SquarePen className="h-4 w-4" />
        </Button>
      </div>
      {/* Empty middle is a full-height window-drag region. */}
      <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
      {/* Right cluster: terminal + aux + settings — the same controls the
          desktop RightEdgeChrome shows, now as direct buttons (no ⋯ menu). */}
      <div className="flex shrink-0 items-center gap-1 pr-2">
        <Button
          variant="ghost"
          size="icon"
          className={cn("h-8 w-8 shrink-0", terminalOpen && "bg-accent")}
          onClick={() => toggleTerminal()}
          disabled={!activeFolder}
          title={tTitleBar("toggleTerminal")}
          aria-label={tTitleBar("toggleTerminal")}
        >
          <SquareTerminal className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className={cn("h-8 w-8 shrink-0", auxPanelOpen && "bg-accent")}
          onClick={toggleAuxPanel}
          disabled={!activeFolder && !isChatMode}
          title={tTitleBar("toggleAuxPanel")}
          aria-label={tTitleBar("toggleAuxPanel")}
        >
          <PanelRight className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 shrink-0"
          onClick={handleOpenSettings}
          title={tTitleBar("openSettings")}
          aria-label={tTitleBar("openSettings")}
        >
          <Settings className="h-4 w-4" />
        </Button>
      </div>
      {/* Windows/Linux caption buttons; self-nulls on macOS / web. */}
      <WindowControls />
    </div>
  )
}
