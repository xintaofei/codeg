"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react"
import {
  loadPersistedPanelState,
  savePersistedPanelState,
} from "@/lib/panel-state-storage"

const DEFAULT_WIDTH = 320
const MIN_WIDTH = 200
const MAX_WIDTH = 600
const DEFAULT_IS_OPEN = true

export type SidebarTab = "conversations" | "directory"

interface SidebarContextValue {
  isOpen: boolean
  width: number
  minWidth: number
  maxWidth: number
  activeTab: SidebarTab
  toggle: () => void
  setWidth: (w: number) => void
  setActiveTab: (tab: SidebarTab) => void
}

const SidebarContext = createContext<SidebarContextValue | null>(null)

export function useSidebarContext() {
  const ctx = useContext(SidebarContext)
  if (!ctx) {
    throw new Error("useSidebarContext must be used within SidebarProvider")
  }
  return ctx
}

function clampWidth(width: number) {
  return Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, width))
}

interface SidebarProviderProps {
  children: ReactNode
  folderId: number
}

export function SidebarProvider({ children, folderId }: SidebarProviderProps) {
  const storageKey = useMemo(
    () => `folder:${folderId}:left-sidebar`,
    [folderId]
  )
  const [isOpen, setIsOpen] = useState(DEFAULT_IS_OPEN)
  const [width, setWidthState] = useState(DEFAULT_WIDTH)
  const [activeTab, setActiveTab] = useState<SidebarTab>("conversations")
  const [restored, setRestored] = useState(false)

  const toggle = useCallback(() => setIsOpen((prev) => !prev), [])

  const setWidth = useCallback((w: number) => {
    setWidthState(clampWidth(w))
  }, [])

  useEffect(() => {
    const stored = loadPersistedPanelState(storageKey)
    // Hydrate from localStorage after mount to keep SSR/CSR markup consistent.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setIsOpen(stored?.isOpen ?? DEFAULT_IS_OPEN)
    setWidthState(clampWidth(stored?.width ?? DEFAULT_WIDTH))
    setRestored(true)
  }, [storageKey])

  useEffect(() => {
    if (!restored) return
    savePersistedPanelState(storageKey, { isOpen, width })
  }, [isOpen, restored, storageKey, width])

  const value = useMemo(
    () => ({
      isOpen,
      width,
      minWidth: MIN_WIDTH,
      maxWidth: MAX_WIDTH,
      activeTab,
      toggle,
      setWidth,
      setActiveTab,
    }),
    [activeTab, isOpen, width, toggle, setWidth]
  )

  return (
    <SidebarContext.Provider value={value}>{children}</SidebarContext.Provider>
  )
}
