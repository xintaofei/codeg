"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react"

export type LoopsView = "chat" | "loops"

const STORAGE_KEY = "codeg:loops-view:v1"

interface LoopsViewContextValue {
  view: LoopsView
  setView: (view: LoopsView) => void
}

const LoopsViewContext = createContext<LoopsViewContextValue | null>(null)

function readStored(): LoopsView {
  if (typeof window === "undefined") return "chat"
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "loops" ? "loops" : "chat"
  } catch {
    return "chat"
  }
}

/**
 * Holds which workspace surface is showing — the normal chat workspace or the
 * loop engineering workbench. The choice persists in localStorage. Selecting a
 * chat tab (i.e. `activeTabId` changing after hydration) flips back to chat,
 * since that gesture means the user wants the chat surface.
 */
export function LoopsViewProvider({
  activeTabId,
  children,
}: {
  activeTabId: string | null
  children: ReactNode
}) {
  const [view, setViewState] = useState<LoopsView>(readStored)
  const prevTabRef = useRef<string | null | undefined>(undefined)

  const setView = useCallback((next: LoopsView) => {
    setViewState(next)
    try {
      window.localStorage.setItem(STORAGE_KEY, next)
    } catch {
      // ignore (private mode / unavailable storage)
    }
  }, [])

  useEffect(() => {
    // Skip the initial hydration pass so a restored "loops" view survives mount.
    if (prevTabRef.current === undefined) {
      prevTabRef.current = activeTabId
      return
    }
    if (prevTabRef.current !== activeTabId) {
      prevTabRef.current = activeTabId
      // eslint-disable-next-line react-hooks/set-state-in-effect -- flip to chat in response to a tab change
      setView("chat")
    }
  }, [activeTabId, setView])

  return (
    <LoopsViewContext.Provider value={{ view, setView }}>
      {children}
    </LoopsViewContext.Provider>
  )
}

export function useLoopsView(): LoopsViewContextValue {
  const ctx = useContext(LoopsViewContext)
  if (!ctx) {
    throw new Error("useLoopsView must be used within a LoopsViewProvider")
  }
  return ctx
}
