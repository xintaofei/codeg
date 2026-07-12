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

const STORAGE_KEY = "workspace:left-sidebar"

const DEFAULT_WIDTH = 320
const MIN_WIDTH = 200
const MAX_WIDTH = 900
const DEFAULT_IS_OPEN = true

interface SidebarContextValue {
  isOpen: boolean
  restored: boolean
  width: number
  minWidth: number
  maxWidth: number
  toggle: () => void
  setWidth: (w: number) => void
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
}

export function SidebarProvider({ children }: SidebarProviderProps) {
  const storageKey = STORAGE_KEY
  const [isOpen, setIsOpen] = useState(DEFAULT_IS_OPEN)
  const [width, setWidthState] = useState(DEFAULT_WIDTH)
  const [restored, setRestored] = useState(false)

  const toggle = useCallback(() => setIsOpen((prev) => !prev), [])

  const setWidth = useCallback((w: number) => {
    setWidthState(clampWidth(w))
  }, [])

  useEffect(() => {
    const stored = loadPersistedPanelState(storageKey)
    // On mobile (< 768px), always start closed regardless of persisted state.
    const isMobileViewport = window.innerWidth < 768
    const defaultOpen = isMobileViewport ? false : DEFAULT_IS_OPEN
    // Hydrate from localStorage after mount to keep SSR/CSR markup consistent.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setIsOpen(isMobileViewport ? false : (stored?.isOpen ?? defaultOpen))
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
      restored,
      width,
      minWidth: MIN_WIDTH,
      maxWidth: MAX_WIDTH,
      toggle,
      setWidth,
    }),
    [isOpen, restored, width, toggle, setWidth]
  )

  return (
    <SidebarContext.Provider value={value}>{children}</SidebarContext.Provider>
  )
}
