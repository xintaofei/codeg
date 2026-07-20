"use client"

import { useCallback } from "react"
import { PanelRight, Settings, SquareTerminal } from "lucide-react"
import { useTranslations } from "next-intl"
import { openSettingsWindow } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { useIsMac } from "@/hooks/use-is-mac"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { formatShortcutLabel } from "@/lib/keyboard-shortcuts"
import { RIGHT_CHROME_CLUSTER } from "@/lib/window-chrome"

/**
 * Contents of the window's fixed top-RIGHT chrome overlay: terminal + aux-panel
 * toggles + settings. `FolderLayoutShell` pins this at the window's top-right
 * corner (to the LEFT of the Windows/Linux caption buttons) so it never moves —
 * or re-mounts — when the aux panel opens or closes. Preserves the old title
 * bar's disabled predicates and active styling. A leading drag filler right-
 * aligns the cluster and lets its empty space move the window; the fixed width
 * matches the right-edge column's reservation (see `rightChromeReserve`).
 */
export function RightEdgeChrome() {
  const tTitleBar = useTranslations("Folder.folderTitleBar")
  const { activeFolder } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const { isOpen: auxPanelOpen, toggle: toggleAuxPanel } = useAuxPanelContext()
  const { isOpen: terminalOpen, toggle: toggleTerminal } = useTerminalContext()
  const isMac = useIsMac()
  const { shortcuts } = useShortcutSettings()

  const handleOpenSettings = useCallback(() => {
    openSettingsWindow().catch((err) => {
      console.error("[RightEdgeChrome] failed to open settings:", err)
    })
  }, [])

  return (
    <div
      className="flex h-full items-center"
      style={{ width: RIGHT_CHROME_CLUSTER }}
    >
      {/* Empty head is a window-drag region; buttons stay flush right. */}
      <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
      <div className="flex items-center gap-1 pr-3">
        <Button
          variant="ghost"
          size="icon"
          className={`h-6 w-6 hover:bg-foreground/10 hover:text-foreground/80 dark:hover:bg-foreground/10 ${terminalOpen ? "bg-accent" : ""}`}
          onClick={() => toggleTerminal()}
          disabled={!activeFolder}
          title={tTitleBar("withShortcut", {
            label: tTitleBar("toggleTerminal"),
            shortcut: formatShortcutLabel(shortcuts.toggle_terminal, isMac),
          })}
        >
          <SquareTerminal className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className={`h-6 w-6 hover:bg-foreground/10 hover:text-foreground/80 dark:hover:bg-foreground/10 ${auxPanelOpen ? "bg-accent" : ""}`}
          onClick={toggleAuxPanel}
          disabled={!activeFolder && !isChatMode}
          title={tTitleBar("withShortcut", {
            label: tTitleBar("toggleAuxPanel"),
            shortcut: formatShortcutLabel(shortcuts.toggle_aux_panel, isMac),
          })}
        >
          <PanelRight className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="h-6 w-6 hover:bg-foreground/10 hover:text-foreground/80 dark:hover:bg-foreground/10"
          onClick={handleOpenSettings}
          title={tTitleBar("withShortcut", {
            label: tTitleBar("openSettings"),
            shortcut: formatShortcutLabel(shortcuts.open_settings, isMac),
          })}
        >
          <Settings className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  )
}
