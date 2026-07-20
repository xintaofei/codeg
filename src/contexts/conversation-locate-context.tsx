"use client"

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  type ReactNode,
} from "react"
import { useSidebarContext } from "@/contexts/sidebar-context"

interface ConversationLocateContextValue {
  /**
   * The sidebar conversation list registers its `scrollToActive` here on mount
   * and clears it (null) on unmount. Lifting it out of the sidebar lets the
   * conversation detail header's "locate" button reach it even though the two
   * live in different columns.
   */
  registerLocate: (scrollToActive: (() => void) | null) => void
  /**
   * Scroll the sidebar conversation list to the active conversation, first
   * opening the sidebar if it's collapsed (the list is unmounted while closed,
   * so the request is queued and runs once the list registers on mount).
   */
  locateActiveConversation: () => void
}

const ConversationLocateContext =
  createContext<ConversationLocateContextValue | null>(null)

export function useConversationLocate() {
  const ctx = useContext(ConversationLocateContext)
  if (!ctx) {
    throw new Error(
      "useConversationLocate must be used within ConversationLocateProvider"
    )
  }
  return ctx
}

export function ConversationLocateProvider({
  children,
}: {
  children: ReactNode
}) {
  const { isOpen, toggle } = useSidebarContext()
  const locateRef = useRef<(() => void) | null>(null)
  const pendingRef = useRef(false)

  const registerLocate = useCallback((scrollToActive: (() => void) | null) => {
    locateRef.current = scrollToActive
    // A locate requested while the sidebar was collapsed is fulfilled the
    // moment the freshly-mounted list registers. Defer one frame so the list
    // is laid out (virtua measured) before we scroll to the active row.
    if (scrollToActive && pendingRef.current) {
      pendingRef.current = false
      requestAnimationFrame(() => scrollToActive())
    }
  }, [])

  const locateActiveConversation = useCallback(() => {
    if (locateRef.current) {
      locateRef.current()
      return
    }
    // Sidebar collapsed → list unmounted. Queue the request and open the
    // sidebar; `registerLocate` runs it when the list mounts.
    pendingRef.current = true
    if (!isOpen) toggle()
  }, [isOpen, toggle])

  const value = useMemo(
    () => ({ registerLocate, locateActiveConversation }),
    [registerLocate, locateActiveConversation]
  )

  return (
    <ConversationLocateContext.Provider value={value}>
      {children}
    </ConversationLocateContext.Provider>
  )
}
