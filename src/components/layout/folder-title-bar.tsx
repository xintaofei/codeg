"use client"

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import {
  Columns2,
  EllipsisVertical,
  FileCode2,
  Menu,
  MessageSquare,
  PanelLeft,
  PanelRight,
  Search,
  Settings,
  SquareTerminal,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { getGitBranch, openFolderWindow, openSettingsWindow } from "@/lib/api"
import { isDesktop, openFileDialog } from "@/lib/platform"
import { useFolderContext } from "@/contexts/folder-context"
import { Button } from "@/components/ui/button"
import { useSidebarContext } from "@/contexts/sidebar-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useTabContext } from "@/contexts/tab-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { useIsMac } from "@/hooks/use-is-mac"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import {
  formatShortcutLabel,
  matchShortcutEvent,
} from "@/lib/keyboard-shortcuts"
import { AppTitleBar } from "./app-title-bar"
import { FolderNameDropdown } from "./folder-name-dropdown"
import { BranchDropdown } from "./branch-dropdown"
import { CommandDropdown } from "./command-dropdown"
import { SearchCommandDialog } from "@/components/conversations/search-command-dialog"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"

const MODE_TABS = [
  {
    mode: "conversation",
    titleKey: "conversation",
    icon: MessageSquare,
  },
  {
    mode: "fusion",
    titleKey: "fusion",
    icon: Columns2,
  },
  {
    mode: "files",
    titleKey: "files",
    icon: FileCode2,
  },
] as const

export function FolderTitleBar() {
  const tModes = useTranslations("Folder.modes")
  const tTitleBar = useTranslations("Folder.folderTitleBar")
  const { folder } = useFolderContext()
  const { isOpen, toggle } = useSidebarContext()
  const { isOpen: auxPanelOpen, toggle: toggleAuxPanel } = useAuxPanelContext()
  const { isOpen: terminalOpen, toggle: toggleTerminal } = useTerminalContext()
  const { openNewConversationTab } = useTabContext()
  const { mode, setMode } = useWorkspaceContext()
  const isMac = useIsMac()
  const { shortcuts } = useShortcutSettings()
  const [branch, setBranch] = useState<string | null>(null)
  const [searchOpen, setSearchOpen] = useState(false)
  const [browserOpen, setBrowserOpen] = useState(false)
  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined
  )

  const folderPath = folder?.path ?? ""

  const handleOpenFolder = useCallback(async () => {
    if (isDesktop()) {
      try {
        const result = await openFileDialog({
          directory: true,
          multiple: false,
        })
        if (!result) return
        const selected = Array.isArray(result) ? result[0] : result
        await openFolderWindow(selected, { newWindow: true })
      } catch (err) {
        console.error("[FolderTitleBar] failed to open folder:", err)
      }
    } else {
      setBrowserOpen(true)
    }
  }, [])

  const handleOpenSettings = useCallback(() => {
    openSettingsWindow().catch((err) => {
      console.error("[FolderTitleBar] failed to open settings:", err)
    })
  }, [])

  useEffect(() => {
    if (!folderPath) return
    let cancelled = false

    // 10s when we have a branch, 60s when we don't. The slow poll still
    // discovers a branch created externally (e.g. `git init` in a terminal)
    // without hammering the backend when there is nothing to find.
    const POLL_FAST_MS = 10_000
    const POLL_SLOW_MS = 60_000

    const clearPoll = () => {
      if (pollTimerRef.current !== undefined) {
        clearTimeout(pollTimerRef.current)
        pollTimerRef.current = undefined
      }
    }

    const scheduleNext = (delayMs: number) => {
      clearPoll()
      pollTimerRef.current = setTimeout(() => {
        pollTimerRef.current = undefined
        void doFetch()
      }, delayMs)
    }

    async function doFetch() {
      if (document.visibilityState !== "visible") return

      let nextDelayMs = POLL_FAST_MS
      try {
        const b = await getGitBranch(folderPath)
        if (cancelled) return
        setBranch(b)
        if (b === null) nextDelayMs = POLL_SLOW_MS
      } catch {
        if (!cancelled) setBranch(null)
        nextDelayMs = POLL_SLOW_MS
      }
      if (!cancelled) scheduleNext(nextDelayMs)
    }

    function handleVisibilityChange() {
      if (document.visibilityState === "visible") {
        void doFetch()
      }
    }

    void doFetch()
    document.addEventListener("visibilitychange", handleVisibilityChange)

    return () => {
      cancelled = true
      clearPoll()
      document.removeEventListener("visibilitychange", handleVisibilityChange)
    }
  }, [folderPath])

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
        e.preventDefault()
        toggleAuxPanel()
        return
      }
      if (matchShortcutEvent(e, shortcuts.new_conversation)) {
        if (!folderPath) return
        e.preventDefault()
        openNewConversationTab(folderPath)
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
    folderPath,
    handleOpenFolder,
    handleOpenSettings,
    openNewConversationTab,
    shortcuts,
    toggle,
    toggleAuxPanel,
    toggleTerminal,
  ])

  const refreshBranch = useCallback(async () => {
    if (!folderPath) return
    try {
      setBranch(await getGitBranch(folderPath))
    } catch {
      setBranch(null)
    }
  }, [folderPath])
  const isMobile = useIsMobile()
  const modeContainerRef = useRef<HTMLDivElement>(null)
  const modeItemRefs = useRef<Map<string, HTMLDivElement>>(new Map())
  const [modeIndicator, setModeIndicator] = useState<{
    left: number
    width: number
  } | null>(null)

  useEffect(() => {
    const container = modeContainerRef.current
    if (!container) return

    const measure = () => {
      const btn = modeItemRefs.current.get(mode)
      if (!btn || !container) {
        setModeIndicator(null)
        return
      }
      const containerRect = container.getBoundingClientRect()
      const btnRect = btn.getBoundingClientRect()
      setModeIndicator({
        left: btnRect.left - containerRect.left,
        width: btnRect.width,
      })
    }

    const ro = new ResizeObserver(() => measure())
    for (const btn of modeItemRefs.current.values()) {
      ro.observe(btn)
    }
    ro.observe(container)
    measure()

    return () => {
      ro.disconnect()
    }
  }, [mode])

  const handleModeKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>, nextMode: typeof mode) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault()
        setMode(nextMode)
      }
    },
    [setMode]
  )

  const modeTabsElement = (
    <div
      ref={modeContainerRef}
      role="tablist"
      aria-label={tModes("workspaceModesAria")}
      className="relative inline-flex h-[27px] items-center rounded-full border border-border/50 bg-muted/50 p-0.5"
    >
      {modeIndicator && (
        <div
          className="pointer-events-none absolute top-0.5 bottom-0.5 rounded-full bg-background shadow-sm ring-1 ring-border/50 transition-all duration-300 ease-[cubic-bezier(0.4,0,0.2,1)]"
          style={{
            left: modeIndicator.left,
            width: modeIndicator.width,
          }}
        />
      )}
      {MODE_TABS.map((item) => {
        const Icon = item.icon
        const isActive = mode === item.mode
        const title = tModes(item.titleKey)
        return (
          <div
            key={item.mode}
            ref={(el) => {
              if (el) {
                modeItemRefs.current.set(item.mode, el)
              } else {
                modeItemRefs.current.delete(item.mode)
              }
            }}
            role="tab"
            tabIndex={0}
            className={cn(
              "relative z-10 m-0 flex h-[23px] cursor-pointer select-none items-center justify-center gap-1 rounded-full border-0 bg-transparent p-0 align-middle text-xs font-medium leading-none transition-all duration-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
              isActive ? "px-2.5" : "px-2",
              isActive
                ? "text-foreground"
                : "text-muted-foreground hover:text-foreground/70"
            )}
            onClick={() => setMode(item.mode)}
            onKeyDown={(event) => handleModeKeyDown(event, item.mode)}
            onMouseDown={(event) => event.preventDefault()}
            title={!isActive ? title : undefined}
            aria-label={title}
            aria-selected={isActive}
          >
            <Icon
              className="block h-3 w-3 shrink-0"
              shapeRendering="geometricPrecision"
            />
            {/* Hide text labels on mobile to save space */}
            {!isMobile && (
              <span
                className={cn(
                  "grid transition-[grid-template-columns] duration-300",
                  isActive ? "grid-cols-[1fr]" : "grid-cols-[0fr]"
                )}
              >
                <span
                  className={cn(
                    "min-w-0 overflow-hidden whitespace-nowrap transition-opacity duration-300",
                    isActive ? "opacity-100" : "opacity-0"
                  )}
                >
                  {title}
                </span>
              </span>
            )}
          </div>
        )
      })}
    </div>
  )

  return (
    <>
      <AppTitleBar
        centerInteractive
        left={
          isMobile ? (
            <div className="flex min-w-0 items-center gap-2">
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8 shrink-0"
                onClick={toggle}
              >
                <Menu className="h-4 w-4" />
              </Button>
              <FolderNameDropdown />
              <BranchDropdown
                branch={branch}
                parentBranch={folder?.parent_branch ?? null}
                onBranchChange={refreshBranch}
              />
            </div>
          ) : (
            <div className="flex min-w-0 items-center gap-4">
              <FolderNameDropdown />
              <BranchDropdown
                branch={branch}
                parentBranch={folder?.parent_branch ?? null}
                onBranchChange={refreshBranch}
              />
              <div data-tauri-drag-region className="h-8 flex-1" />
            </div>
          )
        }
        center={isMobile ? undefined : modeTabsElement}
        right={
          isMobile ? (
            <div className="flex items-center gap-1">
              <CommandDropdown />
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8"
                onClick={() => setSearchOpen(true)}
                title={tTitleBar("search")}
              >
                <Search className="h-4 w-4" />
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon" className="h-8 w-8">
                    <EllipsisVertical className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem onClick={toggleAuxPanel}>
                    <PanelRight className="h-3.5 w-3.5" />
                    {tTitleBar("toggleAuxPanel")}
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={() => toggleTerminal()}>
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
          ) : (
            <div className="flex items-center gap-10">
              <div className="flex items-center gap-2">
                <CommandDropdown />
              </div>
              <div className="flex items-center gap-2">
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-6 w-6 hover:text-foreground/80"
                  onClick={toggle}
                  title={tTitleBar("withShortcut", {
                    label: tTitleBar(isOpen ? "hideSidebar" : "showSidebar"),
                    shortcut: formatShortcutLabel(
                      shortcuts.toggle_sidebar,
                      isMac
                    ),
                  })}
                >
                  <PanelLeft className="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  className={`h-6 w-6 hover:text-foreground/80 ${auxPanelOpen ? "bg-accent" : ""}`}
                  onClick={toggleAuxPanel}
                  title={tTitleBar("withShortcut", {
                    label: tTitleBar("toggleAuxPanel"),
                    shortcut: formatShortcutLabel(
                      shortcuts.toggle_aux_panel,
                      isMac
                    ),
                  })}
                >
                  <PanelRight className="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  className={`h-6 w-6 hover:text-foreground/80 ${terminalOpen ? "bg-accent" : ""}`}
                  onClick={() => toggleTerminal()}
                  title={tTitleBar("withShortcut", {
                    label: tTitleBar("toggleTerminal"),
                    shortcut: formatShortcutLabel(
                      shortcuts.toggle_terminal,
                      isMac
                    ),
                  })}
                >
                  <SquareTerminal className="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-6 w-6 hover:text-foreground/80"
                  onClick={() => setSearchOpen(true)}
                  title={tTitleBar("withShortcut", {
                    label: tTitleBar("search"),
                    shortcut: formatShortcutLabel(
                      shortcuts.toggle_search,
                      isMac
                    ),
                  })}
                >
                  <Search className="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-6 w-6 hover:text-foreground/80"
                  onClick={handleOpenSettings}
                  title={tTitleBar("withShortcut", {
                    label: tTitleBar("openSettings"),
                    shortcut: formatShortcutLabel(
                      shortcuts.open_settings,
                      isMac
                    ),
                  })}
                >
                  <Settings className="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>
          )
        }
      />
      <SearchCommandDialog open={searchOpen} onOpenChange={setSearchOpen} />
      <DirectoryBrowserDialog
        open={browserOpen}
        onOpenChange={setBrowserOpen}
        onSelect={(path) => {
          openFolderWindow(path, { newWindow: true }).catch((err) => {
            console.error("[FolderTitleBar] failed to open folder:", err)
          })
        }}
      />
    </>
  )
}
