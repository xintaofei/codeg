"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { useTaskContext } from "@/contexts/task-context"
import { useConnection, type UseConnectionReturn } from "@/hooks/use-connection"
import { TurnBusyError } from "@/lib/turn-busy"
import { AGENT_LABELS, type AgentType, type PromptDraft } from "@/lib/types"

interface UseConnectionLifecycleOptions {
  contextKey: string
  agentType: AgentType
  isActive: boolean
  workingDir?: string
  sessionId?: string
  /**
   * Persisted conversation id (when known). Passed to `connect()` so it can
   * discover and attach to a live connection another client already owns
   * (cross-client viewing) instead of always spawning a fresh agent.
   */
  conversationId?: number
}

export interface UseConnectionLifecycleReturn {
  conn: UseConnectionReturn
  modeLoading: boolean
  configOptionsLoading: boolean
  selectorsLoading: boolean
  autoConnectError: string | null
  handleFocus: () => void
  handleSend: (
    draft: PromptDraft,
    modeId?: string | null,
    opts?: {
      folderId?: number | null
      conversationId?: number | null
      clientMessageId?: string | null
      /**
       * Called when the backend rejected the send because a turn was already
       * in flight (a second, concurrent prompt). The caller re-queues the
       * draft instead of treating it as an error.
       */
      onTurnInProgress?: () => void
    }
  ) => void
  handleSetConfigOption: (configId: string, valueId: string) => void
  handleCancel: () => void
  handleRespondPermission: (requestId: string, optionId: string) => void
}

function normalizeErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}

function isExpectedConnectError(error: unknown): boolean {
  if (!error || typeof error !== "object") return false
  return (error as { alerted?: unknown }).alerted === true
}

export function useConnectionLifecycle({
  contextKey,
  agentType,
  isActive,
  workingDir,
  sessionId,
  conversationId,
}: UseConnectionLifecycleOptions): UseConnectionLifecycleReturn {
  const t = useTranslations("Folder.chat.connectionLifecycle")
  const { setActiveKey, touchActivity } = useAcpActions()
  const { addTask, updateTask, removeTask } = useTaskContext()
  const conn = useConnection(contextKey)

  // Destructure stable callbacks (depend only on actions + contextKey)
  // vs. volatile derived state (status, liveMessage, etc.)
  const {
    status,
    selectorsReady,
    connect: connConnect,
    disconnect: connDisconnect,
    sendPrompt,
    setMode: connSetMode,
    setConfigOption: connSetConfigOption,
    cancel: connCancel,
    respondPermission: connRespondPermission,
    modes,
    configOptions,
    hasCachedSelectors,
  } = conn
  const isInteractiveStatus = status === "connected" || status === "prompting"
  const hasSelectorsData = modes !== null || configOptions !== null
  const effectiveSelectorsReady = selectorsReady || hasSelectorsData
  const selectorTaskIdRef = useRef<string | null>(null)
  // Visual-only loading indicators for selector chips.
  // Skip loading indicators when we have cached selectors — even if the
  // cache contains no modes/configOptions (the agent simply doesn't have
  // them), we already know what to show and don't need a loading state.
  const modeLoading =
    !hasCachedSelectors &&
    (status === "connecting" ||
      (isInteractiveStatus && !effectiveSelectorsReady))
  const configOptionsLoading =
    !hasCachedSelectors &&
    (status === "connecting" ||
      (isInteractiveStatus && !effectiveSelectorsReady))
  // Gate for send button: block until the backend session is fully
  // initialized (selectorsReady from the real backend event, not cache).
  const selectorsLoading = isInteractiveStatus && !selectorsReady
  const [lastAutoConnectError, setLastAutoConnectError] = useState<{
    contextKey: string
    agentType: AgentType
    message: string
  } | null>(null)

  // Refs for auto-connect effect, which intentionally avoids volatile
  // dependencies to prevent reconnect loops. Synced via useEffect —
  // effects run in declaration order, so these are current before
  // the auto-connect effect reads them.
  const statusRef = useRef(status)
  useEffect(() => {
    statusRef.current = status
  }, [status])
  const isViewerRef = useRef(conn.isViewer)
  useEffect(() => {
    isViewerRef.current = conn.isViewer
  }, [conn.isViewer])
  const contextKeyRef = useRef(contextKey)
  useEffect(() => {
    contextKeyRef.current = contextKey
  }, [contextKey])
  const connConnectRef = useRef(connConnect)
  useEffect(() => {
    connConnectRef.current = connConnect
  }, [connConnect])
  const sessionIdRef = useRef(sessionId)
  useEffect(() => {
    sessionIdRef.current = sessionId
  }, [sessionId])
  const conversationIdRef = useRef(conversationId)
  useEffect(() => {
    conversationIdRef.current = conversationId
  }, [conversationId])
  const modeIdRef = useRef<string | null>(modes?.current_mode_id ?? null)
  useEffect(() => {
    modeIdRef.current = modes?.current_mode_id ?? null
  }, [modes?.current_mode_id])
  // Sync activeKey when this view is the active tab
  useEffect(() => {
    if (isActive && contextKey) {
      setActiveKey(contextKey)
      touchActivity(contextKey)
    }
  }, [isActive, contextKey, setActiveKey, touchActivity])

  // Auto-connect when tab becomes active and workingDir is available.
  // Depends on isActive + workingDir + agentType so that connections wait
  // for folder info to load (workingDir transitions from undefined →
  // folder.path), and so that changing folders or agents on an already-
  // connected tab triggers a reconnect. The context's connect() dedups
  // same-param calls and disconnects+reconnects when workingDir or
  // agentType differs. Status changes must NOT re-trigger this to avoid
  // infinite reconnect loops on transient errors.
  useEffect(() => {
    if (!isActive) return
    if (!workingDir) return
    let cancelled = false
    connConnectRef
      .current(
        agentType,
        workingDir,
        sessionIdRef.current,
        conversationIdRef.current
      )
      .then(() => {
        if (!cancelled) {
          setLastAutoConnectError(null)
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setLastAutoConnectError({
            contextKey: contextKeyRef.current,
            agentType,
            message: normalizeErrorMessage(e),
          })
        }
        if (!isExpectedConnectError(e)) {
          console.error("[ConnLifecycle] auto-connect:", e)
        }
      })
    return () => {
      cancelled = true
    }
  }, [isActive, workingDir, agentType])

  // Manage task status for connection progress
  const taskIdRef = useRef<string | null>(null)
  useEffect(() => {
    if (status === "connecting") {
      if (!taskIdRef.current) {
        const id = `acp-connect-${Date.now()}`
        taskIdRef.current = id
        const agent = AGENT_LABELS[agentType]
        addTask(
          id,
          t("tasks.connectingTitle", { agent }),
          t("tasks.connectingDescription")
        )
      }
      updateTask(taskIdRef.current, { status: "running" })
    } else if (status === "connected" || status === "prompting") {
      if (taskIdRef.current) {
        updateTask(taskIdRef.current, { status: "completed" })
        taskIdRef.current = null
      }
    } else if (status === "error") {
      if (taskIdRef.current) {
        updateTask(taskIdRef.current, {
          status: "failed",
          error: t("errors.connectionFailed"),
        })
        taskIdRef.current = null
      }
    } else if (status === "disconnected" || status === null) {
      if (taskIdRef.current) {
        removeTask(taskIdRef.current)
        taskIdRef.current = null
      }
    }
  }, [status, addTask, updateTask, removeTask, agentType, t])

  const clearSelectorTask = useCallback(() => {
    if (selectorTaskIdRef.current) {
      removeTask(selectorTaskIdRef.current)
      selectorTaskIdRef.current = null
    }
  }, [removeTask])

  useEffect(() => {
    const isInteractive = status === "connected" || status === "prompting"
    if (!isInteractive) {
      clearSelectorTask()
      return
    }

    if (selectorsReady) {
      clearSelectorTask()
      return
    }

    if (!selectorTaskIdRef.current) {
      const id = `acp-session-init-${Date.now()}`
      selectorTaskIdRef.current = id
      const agent = AGENT_LABELS[agentType]
      addTask(
        id,
        t("tasks.initSessionTitle", { agent }),
        t("tasks.initSessionDescription")
      )
      updateTask(id, { status: "running" })
    }
  }, [
    status,
    selectorsReady,
    agentType,
    addTask,
    updateTask,
    clearSelectorTask,
    t,
  ])

  // Keep a ref to disconnect so the unmount cleanup always calls the
  // latest version without adding it as a dependency.
  const connDisconnectRef = useRef(connDisconnect)
  useEffect(() => {
    connDisconnectRef.current = connDisconnect
  }, [connDisconnect])

  // Clean up on unmount (e.g. tab closed): disconnect the ACP connection
  // so it doesn't leak, and remove lingering tasks.
  // However, if the agent is actively prompting (generating a response),
  // keep it alive so it can finish in the background — the idle sweep
  // will clean it up once it transitions back to "connected".
  useEffect(() => {
    return () => {
      // Owners keep a prompting agent alive in the background to finish the
      // turn (the idle sweep reclaims it once it returns to "connected").
      // Viewers are different: disconnect() only DETACHES them (it never
      // acpDisconnects — that belongs to the owner), so tearing a viewer down
      // mid-turn is safe and leaves the owner's agent untouched. And it's
      // necessary: the idle sweep skips viewers, so a viewer left attached
      // here would leak its WS subscription until the whole provider unmounts.
      if (statusRef.current !== "prompting" || isViewerRef.current) {
        connDisconnectRef.current().catch(() => {})
      }
      if (taskIdRef.current) {
        removeTask(taskIdRef.current)
      }
      clearSelectorTask()
    }
  }, [removeTask, clearSelectorTask])

  const handleFocus = useCallback(() => {
    // Respect the caller's readiness gate — e.g. historical conversations
    // set isActive=false until the session's external_id resolves, to
    // avoid connecting with sessionId=undefined and orphaning context.
    if (!isActive) return
    touchActivity(contextKey)
    if (!status || status === "disconnected" || status === "error") {
      setLastAutoConnectError(null)
      connConnect(agentType, workingDir, sessionId, conversationId).catch(
        (e: unknown) => {
          if (!isExpectedConnectError(e)) {
            console.error("[ConnLifecycle] connect:", e)
          }
        }
      )
    }
  }, [
    isActive,
    agentType,
    workingDir,
    sessionId,
    conversationId,
    status,
    connConnect,
    contextKey,
    touchActivity,
  ])

  const autoConnectError =
    status === "connected" || status === "prompting"
      ? null
      : lastAutoConnectError?.contextKey === contextKey &&
          lastAutoConnectError.agentType === agentType
        ? lastAutoConnectError.message
        : null

  // sendPrompt, connCancel, connRespondPermission are stable (depend
  // only on actions + contextKey), so these callbacks are effectively stable.
  const handleSend = useCallback(
    (
      draft: PromptDraft,
      modeId?: string | null,
      opts?: {
        folderId?: number | null
        conversationId?: number | null
        clientMessageId?: string | null
        onTurnInProgress?: () => void
      }
    ) => {
      touchActivity(contextKey)
      const onTurnInProgress = opts?.onTurnInProgress
      void (async () => {
        const currentModeId = modeIdRef.current
        if (modeId && modeId !== currentModeId) {
          await connSetMode(modeId)
          // Optimistically track selected mode to avoid duplicate set_mode
          // calls before CurrentModeUpdate arrives from the agent.
          modeIdRef.current = modeId
        }
        await sendPrompt(draft.blocks, opts)
      })().catch((e: unknown) => {
        if (e instanceof TurnBusyError) {
          // A turn was already in flight on the connection (another
          // co-controlling client, or a "prompting" status this client hadn't
          // observed yet). Not an error — the draft is re-queued by the caller
          // so it auto-sends when the current turn finishes.
          onTurnInProgress?.()
          return
        }
        console.error("[ConnLifecycle] sendPrompt:", e)
      })
    },
    [connSetMode, sendPrompt, contextKey, touchActivity]
  )

  const handleCancel = useCallback(() => {
    connCancel().catch((e: unknown) =>
      console.error("[ConnLifecycle] cancel:", e)
    )
  }, [connCancel])

  const handleSetConfigOption = useCallback(
    (configId: string, valueId: string) => {
      touchActivity(contextKey)
      connSetConfigOption(configId, valueId).catch((e: unknown) =>
        console.error("[ConnLifecycle] setConfigOption:", e)
      )
    },
    [connSetConfigOption, contextKey, touchActivity]
  )

  const handleRespondPermission = useCallback(
    (requestId: string, optionId: string) => {
      touchActivity(contextKey)
      connRespondPermission(requestId, optionId).catch((e: unknown) =>
        console.error("[ConnLifecycle] respondPermission:", e)
      )
    },
    [connRespondPermission, contextKey, touchActivity]
  )

  return {
    conn,
    modeLoading,
    configOptionsLoading,
    selectorsLoading,
    autoConnectError,
    handleFocus,
    handleSend,
    handleSetConfigOption,
    handleCancel,
    handleRespondPermission,
  }
}
