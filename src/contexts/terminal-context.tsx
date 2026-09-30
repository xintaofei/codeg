"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { getSystemTerminalSettings, terminalKill } from "@/lib/api"
import { getActiveRemoteConnectionId, getTransport } from "@/lib/transport"
import { randomUUID } from "@/lib/utils"
import { getCurrentWindowLabel } from "@/lib/browser/window-label"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"

export interface TerminalTab {
  id: string
  folderId: number
  title: string
  workingDir: string
  shell?: string
  initialCommand?: string
  restored?: boolean
}

const DEFAULT_HEIGHT = 300
const MIN_HEIGHT = 150
const MAX_HEIGHT = 600
const TERMINAL_SETTINGS_UPDATED_EVENT = "app://terminal-settings-updated"

const TERMINAL_SESSION_KEY = "codeg:terminal-session:v1"
const PAGE_NAME_PREFIX = "codeg-terminal-page:"
const MAX_STORED_TABS = 32
const UUID_V4 =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i

interface StoredTerminalSession {
  version: 1
  scope: string
  pageId: string
  isOpen: boolean
  activeTabId: string | null
  tabs: Pick<
    TerminalTab,
    "id" | "folderId" | "title" | "workingDir" | "shell"
  >[]
}

function terminalScope(): string {
  return JSON.stringify([
    getCurrentWindowLabel(),
    getActiveRemoteConnectionId(),
  ])
}

/**
 * sessionStorage is a discovery hint, not permission to use a PTY. A tab opened
 * with window.opener can inherit a copy of that storage; window.name belongs to
 * the browsing context and survives reload, but is not cloned into the new tab.
 */
function currentPageId(): string | null {
  if (typeof window === "undefined") return null
  // Do not overwrite a host/application-assigned window name. Those windows
  // keep their normal terminal behavior, but opt out of reload recovery.
  if (!window.name) window.name = `${PAGE_NAME_PREFIX}${randomUUID()}`
  if (!window.name.startsWith(PAGE_NAME_PREFIX)) return null
  const id = window.name.slice(PAGE_NAME_PREFIX.length)
  return UUID_V4.test(id) ? id : null
}

function readTerminalSession(): {
  pageId: string | null
  scope: string
  isOpen: boolean
  activeTabId: string | null
  tabs: TerminalTab[]
} {
  const scope = terminalScope()
  const pageId = currentPageId()
  const empty = { pageId, scope, isOpen: false, activeTabId: null, tabs: [] }
  if (!pageId || typeof window === "undefined") return empty
  try {
    const raw = window.sessionStorage.getItem(TERMINAL_SESSION_KEY)
    if (!raw) return empty
    const saved: StoredTerminalSession = JSON.parse(raw)
    if (
      saved.version !== 1 ||
      saved.scope !== scope ||
      saved.pageId !== pageId ||
      !Array.isArray(saved.tabs) ||
      saved.tabs.length > MAX_STORED_TABS
    )
      return empty
    const tabs: TerminalTab[] = saved.tabs.map((tab) => {
      if (
        !UUID_V4.test(tab.id) ||
        !Number.isSafeInteger(tab.folderId) ||
        tab.folderId <= 0 ||
        typeof tab.title !== "string" ||
        tab.title.length > 256 ||
        typeof tab.workingDir !== "string" ||
        tab.workingDir.length === 0 ||
        tab.workingDir.length > 4096 ||
        !/^(\/|[A-Za-z]:[\\/]|\\\\)/.test(tab.workingDir) ||
        (tab.shell !== undefined &&
          (typeof tab.shell !== "string" || tab.shell.length > 512))
      ) {
        throw new Error("invalid terminal tab")
      }
      return {
        id: tab.id,
        folderId: tab.folderId,
        title: tab.title,
        workingDir: tab.workingDir,
        shell: tab.shell,
        restored: true,
      }
    })
    return {
      pageId,
      scope,
      isOpen: saved.isOpen === true,
      tabs,
      activeTabId: tabs.some((tab) => tab.id === saved.activeTabId)
        ? saved.activeTabId
        : (tabs[0]?.id ?? null),
    }
  } catch {
    return empty
  }
}

interface TerminalContextValue {
  isOpen: boolean
  height: number
  minHeight: number
  maxHeight: number
  toggle: () => void
  setHeight: (h: number) => void
  tabs: TerminalTab[]
  activeTabId: string | null
  exitedTerminals: Set<string>
  markTerminalExited: (id: string) => void
  markTerminalRunning: (id: string) => void
  markTerminalStarted: (id: string) => void
  createTerminal: () => Promise<void>
  createTerminalInDirectory: (
    workingDir: string,
    title?: string,
    shell?: string
  ) => Promise<string | null>
  createTerminalWithCommand: (
    title: string,
    command: string
  ) => Promise<string | null>
  closeTerminal: (id: string) => void
  closeOtherTerminals: (id: string) => void
  closeAllTerminals: () => void
  renameTerminal: (id: string, title: string) => void
  switchTerminal: (id: string) => void
}

const TerminalContext = createContext<TerminalContextValue | null>(null)

export function useTerminalContext() {
  const ctx = useContext(TerminalContext)
  if (!ctx) {
    throw new Error("useTerminalContext must be used within TerminalProvider")
  }
  return ctx
}

export function TerminalProvider({ children }: { children: ReactNode }) {
  const { activeFolder, activeFolderId } = useActiveFolder()
  const { shortcuts } = useShortcutSettings()
  const [restoredSession] = useState(readTerminalSession)
  const [isOpen, setIsOpen] = useState(restoredSession.isOpen)
  const [height, setHeightState] = useState(DEFAULT_HEIGHT)
  const [tabs, setTabs] = useState<TerminalTab[]>(restoredSession.tabs)
  const [activeTabId, setActiveTabId] = useState<string | null>(
    restoredSession.activeTabId
  )
  const tabCounterRef = useRef(restoredSession.tabs.length)
  const [exitedTerminals, setExitedTerminals] = useState<Set<string>>(new Set())
  const [defaultTerminalShell, setDefaultTerminalShell] = useState<
    string | null
  >(null)
  const lastMouseActivityInTerminalRef = useRef(false)
  // Persist before TerminalView's passive spawn effect. A refresh during the
  // spawn request must leave an ID that the next page can probe without replay.
  useLayoutEffect(() => {
    if (!restoredSession.pageId || typeof window === "undefined") return
    const state: StoredTerminalSession = {
      version: 1,
      scope: restoredSession.scope,
      pageId: restoredSession.pageId,
      isOpen,
      activeTabId,
      // The command is intentionally never stored: recovery only attaches to
      // the existing PTY and must not retain sensitive command arguments.
      tabs: tabs
        .slice(0, MAX_STORED_TABS)
        .map(({ id, folderId, title, workingDir, shell }) => ({
          id,
          folderId,
          title,
          workingDir,
          shell,
        })),
    }
    try {
      window.sessionStorage.setItem(TERMINAL_SESSION_KEY, JSON.stringify(state))
    } catch {
      // Private mode or quota failure: terminal remains usable in this page.
    }
  }, [restoredSession, isOpen, activeTabId, tabs])

  const folderPath = activeFolder?.path ?? ""
  const currentFolderId = activeFolderId ?? 0
  const resolveTerminalShell = useCallback(
    (shell?: string) => shell ?? defaultTerminalShell ?? undefined,
    [defaultTerminalShell]
  )

  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | undefined

    getSystemTerminalSettings()
      .then((settings) => {
        if (!cancelled) setDefaultTerminalShell(settings.default_shell)
      })
      .catch((err) => {
        console.error("[terminal] load terminal settings failed:", err)
      })

    getTransport()
      .subscribe<{ default_shell: string | null }>(
        TERMINAL_SETTINGS_UPDATED_EVENT,
        (settings) => {
          setDefaultTerminalShell(settings.default_shell)
        }
      )
      .then((dispose) => {
        if (cancelled) {
          dispose()
          return
        }
        unlisten = dispose
      })
      .catch((err) => {
        console.error("[terminal] subscribe terminal settings failed:", err)
      })

    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  const markTerminalRunning = useCallback((id: string) => {
    setExitedTerminals((prev) => {
      if (!prev.has(id)) return prev
      const next = new Set(prev)
      next.delete(id)
      return next
    })
  }, [])

  const markTerminalStarted = useCallback((id: string) => {
    setTabs((current) =>
      current.map((tab) =>
        tab.id === id && !tab.restored ? { ...tab, restored: true } : tab
      )
    )
  }, [])

  const markTerminalExited = useCallback((id: string) => {
    setExitedTerminals((prev) => {
      if (prev.has(id)) return prev
      const next = new Set(prev)
      next.add(id)
      return next
    })
  }, [])

  const removeExitedTerminals = useCallback((ids: string[]) => {
    setExitedTerminals((prev) => {
      if (prev.size === 0) return prev
      let changed = false
      const next = new Set(prev)
      for (const id of ids) {
        if (next.delete(id)) changed = true
      }
      return changed ? next : prev
    })
  }, [])

  const killTerminalTabs = useCallback((targetTabs: TerminalTab[]) => {
    targetTabs.forEach((tab) => {
      terminalKill(tab.id).catch(() => {})
    })
  }, [])

  const toggle = useCallback(() => {
    const autoId = randomUUID()
    const nextCounter = tabCounterRef.current + 1

    setIsOpen((wasOpen) => !wasOpen)

    // Auto-create first terminal when opening with no tabs
    setTabs((currentTabs) => {
      if (currentTabs.length > 0 || !folderPath) return currentTabs
      tabCounterRef.current = nextCounter
      return [
        {
          id: autoId,
          folderId: currentFolderId,
          title: `Terminal ${nextCounter}`,
          workingDir: folderPath,
          shell: resolveTerminalShell(),
        },
      ]
    })

    setActiveTabId((prev) => {
      if (prev !== null) return prev
      if (!folderPath) return null
      return autoId
    })
  }, [folderPath, currentFolderId, resolveTerminalShell])

  const createTerminalWithCommand = useCallback(
    async (title: string, command: string) => {
      if (!folderPath) return null

      setIsOpen(true)

      const id = randomUUID()
      tabCounterRef.current += 1
      setTabs((prev) => [
        ...prev,
        {
          id,
          folderId: currentFolderId,
          title,
          workingDir: folderPath,
          shell: resolveTerminalShell(),
          initialCommand: command,
        },
      ])
      setActiveTabId(id)

      return id
    },
    [folderPath, currentFolderId, resolveTerminalShell]
  )

  const createTerminalInDirectory = useCallback(
    async (workingDir: string, title?: string, shell?: string) => {
      if (!workingDir) return null

      setIsOpen(true)

      const id = randomUUID()
      tabCounterRef.current += 1
      const defaultTitle = `Terminal ${tabCounterRef.current}`
      setTabs((prev) => [
        ...prev,
        {
          id,
          folderId: currentFolderId,
          title: title ?? defaultTitle,
          workingDir,
          shell: resolveTerminalShell(shell),
        },
      ])
      setActiveTabId(id)

      return id
    },
    [currentFolderId, resolveTerminalShell]
  )

  const createTerminal = useCallback(async () => {
    if (!folderPath) return
    await createTerminalInDirectory(folderPath)
  }, [folderPath, createTerminalInDirectory])

  const setHeight = useCallback((h: number) => {
    setHeightState(Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, h)))
  }, [])

  const closeTerminal = useCallback(
    (id: string) => {
      markTerminalExited(id)
      removeExitedTerminals([id])
      terminalKill(id).catch(() => {})
      setTabs((prev) => {
        const next = prev.filter((t) => t.id !== id)
        if (next.length === 0) {
          tabCounterRef.current = 0
          setIsOpen(false)
          setActiveTabId(null)
        } else {
          setActiveTabId((prevActive) =>
            prevActive === id ? next[next.length - 1].id : prevActive
          )
        }
        return next
      })
    },
    [markTerminalExited, removeExitedTerminals]
  )

  const closeOtherTerminals = useCallback(
    (id: string) => {
      setTabs((prev) => {
        const closed = prev.filter((t) => t.id !== id)
        killTerminalTabs(closed)
        removeExitedTerminals(closed.map((t) => t.id))
        return prev.filter((t) => t.id === id)
      })
      setActiveTabId(id)
    },
    [killTerminalTabs, removeExitedTerminals]
  )

  const closeAllTerminals = useCallback(() => {
    setTabs((prev) => {
      killTerminalTabs(prev)
      removeExitedTerminals(prev.map((t) => t.id))
      return []
    })
    tabCounterRef.current = 0
    setActiveTabId(null)
    setIsOpen(false)
  }, [killTerminalTabs, removeExitedTerminals])

  const renameTerminal = useCallback((id: string, title: string) => {
    setTabs((prev) => prev.map((t) => (t.id === id ? { ...t, title } : t)))
  }, [])

  const switchTerminal = useCallback((id: string) => {
    setActiveTabId(id)
  }, [])

  const isInTerminalRegion = useCallback((target: EventTarget | null) => {
    if (!(target instanceof Element)) return false
    return Boolean(target.closest('[data-terminal-panel-region="true"]'))
  }, [])

  const updateLastMouseActivity = useCallback(
    (target: EventTarget | null) => {
      const next = isInTerminalRegion(target)
      if (lastMouseActivityInTerminalRef.current === next) return
      lastMouseActivityInTerminalRef.current = next
    },
    [isInTerminalRegion]
  )

  useEffect(() => {
    const handlePointerActivity = (event: PointerEvent) => {
      updateLastMouseActivity(event.target)
    }
    const handleFocusActivity = (event: FocusEvent) => {
      updateLastMouseActivity(event.target)
    }

    window.addEventListener("pointerover", handlePointerActivity, true)
    window.addEventListener("pointerdown", handlePointerActivity, true)
    window.addEventListener("focusin", handleFocusActivity, true)
    return () => {
      window.removeEventListener("pointerover", handlePointerActivity, true)
      window.removeEventListener("pointerdown", handlePointerActivity, true)
      window.removeEventListener("focusin", handleFocusActivity, true)
    }
  }, [updateLastMouseActivity])

  useEffect(() => {
    if (!isOpen) {
      lastMouseActivityInTerminalRef.current = false
    }
  }, [isOpen])

  useEffect(() => {
    const handleTerminalHotkeys = (event: KeyboardEvent) => {
      if (!isOpen) return

      const targetInTerminal = isInTerminalRegion(event.target)
      const activeElementInTerminal = isInTerminalRegion(document.activeElement)
      const shouldHandle =
        lastMouseActivityInTerminalRef.current ||
        targetInTerminal ||
        activeElementInTerminal
      if (!shouldHandle) return

      if (matchShortcutEvent(event, shortcuts.new_terminal_tab)) {
        event.preventDefault()
        event.stopPropagation()
        void createTerminal()
        return
      }

      if (
        activeTabId &&
        matchShortcutEvent(event, shortcuts.close_current_terminal_tab)
      ) {
        event.preventDefault()
        event.stopPropagation()
        closeTerminal(activeTabId)
      }
    }

    window.addEventListener("keydown", handleTerminalHotkeys, true)
    return () => {
      window.removeEventListener("keydown", handleTerminalHotkeys, true)
    }
  }, [
    activeTabId,
    closeTerminal,
    createTerminal,
    isInTerminalRegion,
    isOpen,
    shortcuts.close_current_terminal_tab,
    shortcuts.new_terminal_tab,
  ])

  const value = useMemo(
    () => ({
      isOpen,
      height,
      minHeight: MIN_HEIGHT,
      maxHeight: MAX_HEIGHT,
      toggle,
      setHeight,
      tabs,
      activeTabId,
      exitedTerminals,
      markTerminalExited,
      markTerminalRunning,
      markTerminalStarted,
      createTerminal,
      createTerminalInDirectory,
      createTerminalWithCommand,
      closeTerminal,
      closeOtherTerminals,
      closeAllTerminals,
      renameTerminal,
      switchTerminal,
    }),
    [
      isOpen,
      height,
      toggle,
      setHeight,
      tabs,
      activeTabId,
      exitedTerminals,
      markTerminalExited,
      markTerminalRunning,
      markTerminalStarted,
      createTerminal,
      createTerminalInDirectory,
      createTerminalWithCommand,
      closeTerminal,
      closeOtherTerminals,
      closeAllTerminals,
      renameTerminal,
      switchTerminal,
    ]
  )

  return (
    <TerminalContext.Provider value={value}>
      {children}
    </TerminalContext.Provider>
  )
}
