"use client"

import { useCallback } from "react"
import {
  Folder,
  FolderPen,
  GitCommit,
  ReceiptText,
  Settings,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { openSettingsWindow } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { useActiveFolder } from "@/contexts/active-folder-context"
import {
  useAuxPanelContext,
  type AuxPanelTab,
} from "@/contexts/aux-panel-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { WorkbenchRouteChromeActions } from "@/components/workbench/workbench-content"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { useIsMac } from "@/hooks/use-is-mac"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { formatShortcutLabel } from "@/lib/keyboard-shortcuts"
import { cn } from "@/lib/utils"

// Same order + icons as the aux panel's own tab strip (aux-panel.tsx TAB_ORDER).
// Duplicated as a local table (rather than importing from aux-panel) so the rail
// never depends on the panel module; the labels come from the same i18n keys.
const RAIL_TABS: { tab: AuxPanelTab; Icon: LucideIcon }[] = [
  { tab: "session_details", Icon: ReceiptText },
  { tab: "file_tree", Icon: Folder },
  { tab: "changes", Icon: FolderPen },
  { tab: "git_log", Icon: GitCommit },
]

const RAIL_BUTTON_CLASS =
  "h-7 w-7 shrink-0 rounded-lg hover:bg-foreground/10 hover:text-foreground/80 dark:hover:bg-foreground/10"
const RAIL_ICON_CLASS = "h-4 w-4"

/**
 * The window's fixed right-edge vertical icon rail — the desktop replacement
 * for the old top-right horizontal chrome cluster (`RightEdgeChrome`). Mirrors
 * openchamber's right sidebar: one column of icons, flush to the window's right
 * edge, where each aux-panel tab has its own direct toggle.
 *
 * It is a LAYOUT column (not a floating overlay): `FolderWorkspaceShell`
 * renders it as the flex sibling after the resizable shell group, so nothing
 * has to reserve its width and the panel content never renders underneath.
 * The Windows/Linux caption buttons stay a fixed overlay (WindowControls) and
 * sit over the rail's leading `h-10` drag filler, exactly as they used to sit
 * over the right edge of the aux strip.
 *
 * Tab-button semantics: closed panel → open on that tab; open panel, other tab
 * → switch; open panel, SAME active tab → close (pressing the lit icon turns
 * the panel off). Folder-scoped tabs hide without a folder / in chat mode,
 * matching `resolveAuxTabView`'s predicate in the panel itself.
 */
export function RightEdgeRail() {
  const tTitleBar = useTranslations("Folder.folderTitleBar")
  const tTabs = useTranslations("Folder.auxPanel.tabs")
  const tDetails = useTranslations("Folder.sessionDetails")
  const { activeFolder } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const { isConversations } = useWorkbenchRoute()
  const {
    isOpen: auxPanelOpen,
    toggle: toggleAuxPanel,
    openTab,
    setActiveTab,
    activeTab,
  } = useAuxPanelContext()
  const { isOpen: terminalOpen, toggle: toggleTerminal } = useTerminalContext()
  const isMac = useIsMac()
  const { shortcuts } = useShortcutSettings()

  const handleOpenSettings = useCallback(() => {
    openSettingsWindow().catch((err) => {
      console.error("[RightEdgeRail] failed to open settings:", err)
    })
  }, [])

  const handleTabClick = useCallback(
    (tab: AuxPanelTab) => {
      if (auxPanelOpen && activeTab === tab) {
        toggleAuxPanel()
      } else if (auxPanelOpen) {
        setActiveTab(tab)
      } else {
        openTab(tab)
      }
    },
    [auxPanelOpen, activeTab, toggleAuxPanel, setActiveTab, openTab]
  )

  const tabLabel = useCallback(
    (tab: AuxPanelTab) =>
      tab === "session_details"
        ? tDetails("menuLabel")
        : tTabs(
            tab === "file_tree"
              ? "files"
              : tab === "changes"
                ? "changes"
                : "commits"
          ),
    [tTabs, tDetails]
  )

  // Same predicate as the panel's resolveAuxTabView: folder-scoped tabs need a
  // real folder workspace and a non-chat session.
  const showFolderTabs = Boolean(activeFolder) && !isChatMode

  const visibleTabs = RAIL_TABS.filter(
    ({ tab }) => tab === "session_details" || showFolderTabs
  )

  return (
    <aside
      data-testid="right-edge-rail"
      className={cn(
        "flex h-full w-10 shrink-0 flex-col items-center gap-1 border-l border-border py-1",
        // Off-image the rail matches the strips / StatusBar (bg-muted) and its
        // left border is the plain theme border; with a workspace background
        // image on it goes transparent like every other strip
        // (ws-transparent-bg) and the divider strengthens to the shared chrome
        // hairline color (ws-chrome-border), same family as the tab strips.
        "bg-muted ws-transparent-bg ws-chrome-border"
      )}
    >
      {/* Leading spacer under the (Windows/Linux) native caption buttons and
          the macOS traffic-light row's Y band; empty space drags the window. */}
      <div
        data-tauri-drag-region
        className="h-10 w-full shrink-0"
        aria-hidden="true"
      />
      {isConversations ? (
        <>
          {visibleTabs.map(({ tab, Icon }) => {
            const label = tabLabel(tab)
            return (
              <button
                key={tab}
                type="button"
                aria-label={label}
                title={label}
                onClick={() => handleTabClick(tab)}
                className={cn(
                  RAIL_BUTTON_CLASS,
                  "flex items-center justify-center",
                  auxPanelOpen && activeTab === tab
                    ? "bg-accent text-foreground"
                    : "text-foreground/70"
                )}
              >
                <Icon className={RAIL_ICON_CLASS} />
              </button>
            )
          })}
          <div
            className="my-1 h-px w-5 shrink-0 bg-border"
            aria-hidden="true"
          />
          <Button
            variant="ghost"
            size="icon"
            className={cn(RAIL_BUTTON_CLASS, terminalOpen && "bg-accent")}
            onClick={() => toggleTerminal()}
            disabled={!activeFolder}
            title={tTitleBar("withShortcut", {
              label: tTitleBar("toggleTerminal"),
              shortcut: formatShortcutLabel(shortcuts.toggle_terminal, isMac),
            })}
          >
            <SquareTerminal className={RAIL_ICON_CLASS} />
          </Button>
        </>
      ) : (
        <WorkbenchRouteChromeActions
          buttonClassName={RAIL_BUTTON_CLASS}
          iconClassName={RAIL_ICON_CLASS}
        />
      )}
      <Button
        variant="ghost"
        size="icon"
        className={RAIL_BUTTON_CLASS}
        onClick={handleOpenSettings}
        title={tTitleBar("withShortcut", {
          label: tTitleBar("openSettings"),
          shortcut: formatShortcutLabel(shortcuts.open_settings, isMac),
        })}
      >
        <Settings className={RAIL_ICON_CLASS} />
      </Button>
    </aside>
  )
}
