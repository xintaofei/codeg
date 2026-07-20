"use client"

import { useCallback, useState } from "react"
import {
  EllipsisVertical,
  FolderOpen,
  Menu,
  PanelRight,
  Settings,
  SquareTerminal,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useSidebarContext } from "@/contexts/sidebar-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { openSettingsWindow } from "@/lib/api"
import { isDesktop, openFileDialog } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { AppTitleBar } from "./app-title-bar"
import { BranchDropdown } from "./branch-dropdown"
import { CommandDropdown } from "./command-dropdown"

/**
 * Mobile-only workspace title bar.
 *
 * Desktop uses the upstream fixed edge chrome. Mobile keeps the larger touch
 * targets and compact branch/command controls from the companion UI.
 */
export function FolderTitleBar() {
  const tTitleBar = useTranslations("Folder.folderTitleBar")
  const tFolderMenu = useTranslations("Folder.folderNameDropdown")
  const openFolder = useAppWorkspaceStore((s) => s.openFolder)
  const { activeFolder } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const { isOpen, toggle } = useSidebarContext()
  const { toggle: toggleAuxPanel } = useAuxPanelContext()
  const { toggle: toggleTerminal } = useTerminalContext()
  const [browserOpen, setBrowserOpen] = useState(false)

  const handleOpenFolder = useCallback(async () => {
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
        console.error("[FolderTitleBar] failed to open folder:", err)
      }
      return
    }
    setBrowserOpen(true)
  }, [openFolder])

  const handleOpenSettings = useCallback(() => {
    openSettingsWindow().catch((err) => {
      console.error("[FolderTitleBar] failed to open settings:", err)
    })
  }, [])

  return (
    <>
      <AppTitleBar
        left={
          <div className="flex min-w-0 items-center gap-1 overflow-hidden">
            <Button
              variant="ghost"
              size="icon"
              className="h-11 w-11 shrink-0 rounded-xl"
              onClick={toggle}
              aria-label={tTitleBar(isOpen ? "hideSidebar" : "showSidebar")}
            >
              <Menu className="h-5 w-5" />
            </Button>
            <div className="min-w-0 flex-1 overflow-hidden">
              {activeFolder && !isChatMode ? (
                <BranchDropdown compact />
              ) : (
                <div className="min-w-0 px-1">
                  <div className="truncate text-sm font-semibold leading-5">
                    {activeFolder?.name ?? "Codeg"}
                  </div>
                  <div className="truncate text-[11px] leading-4 text-muted-foreground">
                    {isChatMode ? "Chat" : tFolderMenu("openFolder")}
                  </div>
                </div>
              )}
            </div>
          </div>
        }
        right={
          <div className="flex shrink-0 items-center gap-0.5">
            <CommandDropdown compact />
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-11 w-11 rounded-xl"
                  aria-label={tTitleBar("openSettings")}
                >
                  <EllipsisVertical className="h-5 w-5" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={() => void handleOpenFolder()}>
                  <FolderOpen className="h-4 w-4" />
                  {tFolderMenu("openFolder")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={toggleAuxPanel}
                  disabled={!activeFolder && !isChatMode}
                >
                  <PanelRight className="h-3.5 w-3.5" />
                  {tTitleBar("toggleAuxPanel")}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => toggleTerminal()}
                  disabled={!activeFolder}
                >
                  <SquareTerminal className="h-3.5 w-3.5" />
                  {tTitleBar("toggleTerminal")}
                </DropdownMenuItem>
                <DropdownMenuItem onClick={handleOpenSettings}>
                  <Settings className="h-3.5 w-3.5" />
                  {tTitleBar("openSettings")}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        }
      />
      <DirectoryBrowserDialog
        open={browserOpen}
        onOpenChange={setBrowserOpen}
        onSelect={(path) => {
          openFolder(path).catch((err) => {
            console.error("[FolderTitleBar] failed to open folder:", err)
          })
        }}
      />
    </>
  )
}
