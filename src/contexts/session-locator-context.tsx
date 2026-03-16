"use client"

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  useRef,
  type ReactNode,
} from "react"
import type { SessionLocatorTarget } from "@/lib/session-locator"

type SessionLocatorJumpHandler = (target: SessionLocatorTarget) => void

interface SessionLocatorContextValue {
  registerJumpHandler: (
    conversationId: number,
    handler: SessionLocatorJumpHandler
  ) => () => void
  jumpToTarget: (conversationId: number, target: SessionLocatorTarget) => void
  getActiveTarget: (conversationId: number) => SessionLocatorTarget | null
}

const SessionLocatorContext = createContext<SessionLocatorContextValue | null>(
  null
)

export function SessionLocatorProvider({
  children,
}: {
  children: ReactNode
}) {
  const handlersRef = useRef(new Map<number, SessionLocatorJumpHandler>())
  const [activeTargets, setActiveTargets] = useState<
    Record<number, SessionLocatorTarget | null>
  >({})

  const registerJumpHandler = useCallback(
    (conversationId: number, handler: SessionLocatorJumpHandler) => {
      handlersRef.current.set(conversationId, handler)

      return () => {
        const currentHandler = handlersRef.current.get(conversationId)
        if (currentHandler === handler) {
          handlersRef.current.delete(conversationId)
        }
      }
    },
    []
  )

  const jumpToTarget = useCallback(
    (conversationId: number, target: SessionLocatorTarget) => {
      setActiveTargets((prev) => ({
        ...prev,
        [conversationId]: target,
      }))
      handlersRef.current.get(conversationId)?.(target)
    },
    []
  )

  const getActiveTarget = useCallback(
    (conversationId: number) => activeTargets[conversationId] ?? null,
    [activeTargets]
  )

  const value = useMemo(
    () => ({
      registerJumpHandler,
      jumpToTarget,
      getActiveTarget,
    }),
    [getActiveTarget, jumpToTarget, registerJumpHandler]
  )

  return (
    <SessionLocatorContext.Provider value={value}>
      {children}
    </SessionLocatorContext.Provider>
  )
}

export function useSessionLocatorContext() {
  const ctx = useContext(SessionLocatorContext)
  if (!ctx) {
    throw new Error(
      "useSessionLocatorContext must be used within SessionLocatorProvider"
    )
  }
  return ctx
}
