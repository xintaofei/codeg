"use client"

import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  useRef,
  useMemo,
  type ReactNode,
} from "react"
import { useTranslations } from "next-intl"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { useSortedAvailableAgents } from "@/hooks/use-sorted-available-agents"
import { listOpenedTabs, saveOpenedTabs } from "@/lib/api"
import { resolveDefaultAgent } from "@/lib/resolve-default-agent"
import type { AgentType, ConversationStatus, OpenedTab } from "@/lib/types"

interface TabItemInternal {
  id: string
  kind: "conversation"
  folderId: number
  conversationId: number | null
  /** The runtime session key used by ConversationRuntimeContext.
   *  For new conversations this is a virtual (negative) ID that differs
   *  from the persisted `conversationId`. */
  runtimeConversationId?: number
  agentType: AgentType
  title: string
  isPinned: boolean
  workingDir?: string
  status?: ConversationStatus
  /**
   * Marks `agentType` as a system best-guess that should be replaced once
   * the agent list becomes fresh. True for draft tabs whose default came
   * from a stale localStorage seed or the AGENT_DISPLAY_ORDER fallback;
   * cleared by `confirmDraftAgent` (user click), `bindConversationTab`
   * (draft → real conversation), or the correction effect (fresh agent
   * list arrives). **Not persisted** to opened_tabs — hydrated drafts
   * default to false and are re-evaluated only when their agent_type is
   * no longer in the fresh sorted list (the `!sortedAvailableAgents.
   * includes(...)` branch of correction). Internal-only: no UI component
   * reads it, so a stale `true` value is harmless if correction never
   * runs (e.g. `acpListAgents()` keeps failing).
   */
  agentTypeProvisional?: boolean
}

export type TabItem = TabItemInternal

interface TabContextValue {
  tabs: TabItem[]
  activeTabId: string | null
  tabsHydrated: boolean
  isTileMode: boolean
  openTab: (
    folderId: number,
    conversationId: number,
    agentType: AgentType,
    pin?: boolean,
    title?: string
  ) => void
  closeTab: (tabId: string) => void
  closeConversationTab: (
    folderId: number,
    conversationId: number,
    agentType: AgentType
  ) => void
  closeOtherTabs: (tabId: string) => void
  closeAllTabs: () => void
  closeTabsByFolder: (folderId: number) => void
  switchTab: (tabId: string) => void
  pinTab: (tabId: string) => void
  toggleTileMode: () => void
  openNewConversationTab: (folderId: number, workingDir: string) => void
  /**
   * Mark a draft tab's agent as user-confirmed. Patches `agentType` on
   * the tab and clears the `agentTypeProvisional` flag so the correction
   * effect won't overwrite the user's choice. No-op for tabs already
   * bound to a real conversation (`conversationId != null`). Wired up
   * from conversation-detail-panel's `handleAgentSelect`.
   */
  confirmDraftAgent: (tabId: string, agentType: AgentType) => void
  bindConversationTab: (
    tabId: string,
    conversationId: number,
    agentType: AgentType,
    title: string,
    runtimeConversationId?: number
  ) => void
  setTabRuntimeConversationId: (
    tabId: string,
    runtimeConversationId: number
  ) => void
  reorderTabs: (reorderedTabs: TabItem[]) => void
  onPreviewTabReplaced: (callback: (tabId: string) => void) => () => void
}

const TabContext = createContext<TabContextValue | null>(null)

export function useTabContext() {
  const ctx = useContext(TabContext)
  if (!ctx) {
    throw new Error("useTabContext must be used within TabProvider")
  }
  return ctx
}

function makeConversationTabId(
  folderId: number,
  agentType: AgentType,
  conversationId: number
): string {
  return `conv-${folderId}-${agentType}-${conversationId}`
}

function makeNewConversationTabId(): string {
  return `new-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function findTabIndexForConversation(
  tabs: TabItemInternal[],
  folderId: number,
  agentType: AgentType,
  conversationId: number
): number {
  const canonicalId = makeConversationTabId(folderId, agentType, conversationId)
  const idx = tabs.findIndex((t) => t.id === canonicalId)
  if (idx >= 0) return idx
  return tabs.findIndex(
    (t) =>
      t.folderId === folderId &&
      t.conversationId === conversationId &&
      t.agentType === agentType
  )
}

interface TabProviderProps {
  children: ReactNode
}

const TILE_MODE_STORAGE_KEY = "workspace:tile-mode"

export function TabProvider({ children }: TabProviderProps) {
  const t = useTranslations("Folder.tabContext")
  const { activateConversationPane } = useWorkspaceContext()
  const { conversations, folders, setActiveFolderId } = useAppWorkspace()
  const { disconnect: acpDisconnect } = useAcpActions()

  const [rawTabs, setTabs] = useState<TabItemInternal[]>([])
  const [activeTabId, setActiveTabId] = useState<string | null>(null)
  const [tabsHydrated, setTabsHydrated] = useState(false)

  // Refs for volatile state
  const activeTabIdRef = useRef(activeTabId)
  useEffect(() => {
    activeTabIdRef.current = activeTabId
  }, [activeTabId])

  const rawTabsRef = useRef(rawTabs)
  useEffect(() => {
    rawTabsRef.current = rawTabs
  }, [rawTabs])

  // Sync active tab's folderId up to AppWorkspaceProvider so derived
  // consumers (ActiveFolderProvider, branch polling, etc.) reflect the
  // currently-focused folder.
  useEffect(() => {
    const activeTab = rawTabs.find((t) => t.id === activeTabId) ?? null
    setActiveFolderId(activeTab?.folderId ?? null)
  }, [rawTabs, activeTabId, setActiveFolderId])

  const conversationsRef = useRef(conversations)
  useEffect(() => {
    conversationsRef.current = conversations
  }, [conversations])

  const foldersRef = useRef(folders)
  useEffect(() => {
    foldersRef.current = folders
  }, [folders])

  // ACP agent list driven by the shared hook. `sortedTypes` reflects the
  // user-defined drag-sort order (filtered to enabled+available) and is
  // seeded from localStorage for synchronous cold-start use. `fresh`
  // flips true after the first successful `acpListAgents()` call this
  // session and stays true thereafter — used to gate provisional default
  // assignment and the correction effect below.
  const { sortedTypes: sortedAvailableAgents, fresh: agentsFresh } =
    useSortedAvailableAgents()

  const sortedAvailableAgentsRef = useRef<AgentType[]>(sortedAvailableAgents)
  useEffect(() => {
    sortedAvailableAgentsRef.current = sortedAvailableAgents
  }, [sortedAvailableAgents])

  const agentsFreshRef = useRef(agentsFresh)
  useEffect(() => {
    agentsFreshRef.current = agentsFresh
  }, [agentsFresh])

  // Pick the agent + provisional flag for a new draft tab. Wraps the
  // pure `resolveDefaultAgent` helper with TabProvider-scoped lookups
  // (folder default, latest sorted types, fresh flag). Reads via refs so
  // callbacks don't need to depend on the state values.
  const resolveAgentForFolder = useCallback(
    (
      folderId: number,
      inherit: AgentType | null
    ): { agentType: AgentType; provisional: boolean } => {
      const folderDefault =
        foldersRef.current.find((f) => f.id === folderId)?.default_agent_type ??
        null
      return resolveDefaultAgent({
        folderDefault,
        inherit,
        sortedTypes: sortedAvailableAgentsRef.current,
        fresh: agentsFreshRef.current,
      })
    },
    []
  )

  // Callback set for preview tab replacement notifications
  const previewReplacedCallbacksRef = useRef(new Set<(tabId: string) => void>())
  const onPreviewTabReplaced = useCallback(
    (callback: (tabId: string) => void) => {
      previewReplacedCallbacksRef.current.add(callback)
      return () => {
        previewReplacedCallbacksRef.current.delete(callback)
      }
    },
    []
  )

  // Hydrate from persisted opened_tabs on mount
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const items = await listOpenedTabs()
        if (cancelled) return
        const restored: TabItemInternal[] = items.map((it) => ({
          id:
            it.conversation_id != null
              ? makeConversationTabId(
                  it.folder_id,
                  it.agent_type,
                  it.conversation_id
                )
              : makeNewConversationTabId(),
          kind: "conversation",
          folderId: it.folder_id,
          conversationId: it.conversation_id,
          agentType: it.agent_type,
          title:
            it.conversation_id != null
              ? t("loadingConversation")
              : t("newConversation"),
          isPinned: it.is_pinned,
        }))
        setTabs(restored)
        const active = items.find((it) => it.is_active)
        if (active) {
          const activeRestored = restored.find(
            (r) =>
              r.folderId === active.folder_id &&
              r.agentType === active.agent_type &&
              r.conversationId === active.conversation_id
          )
          if (activeRestored) setActiveTabId(activeRestored.id)
        } else if (restored.length > 0) {
          setActiveTabId(restored[0].id)
        }
      } catch (err) {
        console.error("[TabProvider] listOpenedTabs failed:", err)
      } finally {
        if (!cancelled) setTabsHydrated(true)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [t])

  // Debounced save to DB
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    if (!tabsHydrated) return

    if (saveTimerRef.current) {
      clearTimeout(saveTimerRef.current)
    }

    saveTimerRef.current = setTimeout(() => {
      const items: OpenedTab[] = rawTabs.map((tab, i) => ({
        id: 0,
        folder_id: tab.folderId,
        conversation_id: tab.conversationId,
        agent_type: tab.agentType,
        position: i,
        is_active: tab.id === activeTabId,
        is_pinned: tab.isPinned,
      }))

      saveOpenedTabs(items).catch(() => {
        // Silently ignore save errors
      })
    }, 500)

    return () => {
      if (saveTimerRef.current) {
        clearTimeout(saveTimerRef.current)
      }
    }
  }, [rawTabs, activeTabId, tabsHydrated])

  // Pre-index conversations for O(1) lookup in tabs derivation
  const conversationMap = useMemo(() => {
    const m = new Map<string, (typeof conversations)[number]>()
    for (const c of conversations) {
      m.set(`${c.folder_id}-${c.agent_type}-${c.id}`, c)
    }
    return m
  }, [conversations])

  // Derive tabs with up-to-date titles and status from conversations
  const tabs = useMemo(() => {
    if (conversationMap.size === 0) return rawTabs
    return rawTabs.map((tab) => {
      if (tab.conversationId != null) {
        const conv = conversationMap.get(
          `${tab.folderId}-${tab.agentType}-${tab.conversationId}`
        )
        if (conv) {
          const newTitle = conv.title || t("untitledConversation")
          const newStatus = conv.status as ConversationStatus | undefined
          if (tab.title !== newTitle || tab.status !== newStatus) {
            return { ...tab, title: newTitle, status: newStatus }
          }
        }
      }
      return tab
    })
  }, [rawTabs, conversationMap, t])

  const openTab = useCallback(
    (
      folderId: number,
      conversationId: number,
      agentType: AgentType,
      pin = false,
      title?: string
    ) => {
      let activateTabId: string | undefined
      let replacedPreviewTabId: string | undefined

      setTabs((prev) => {
        const existingIndex = findTabIndexForConversation(
          prev,
          folderId,
          agentType,
          conversationId
        )

        if (existingIndex >= 0) {
          activateTabId = prev[existingIndex].id
          if (pin && !prev[existingIndex].isPinned) {
            const updated = [...prev]
            updated[existingIndex] = {
              ...updated[existingIndex],
              isPinned: true,
            }
            return updated
          }
          return prev
        }

        const resolvedTitle =
          title ??
          conversationsRef.current.find(
            (c) =>
              c.id === conversationId &&
              c.agent_type === agentType &&
              c.folder_id === folderId
          )?.title ??
          t("untitledConversation")

        const tabId = makeConversationTabId(folderId, agentType, conversationId)
        activateTabId = tabId
        const newTab: TabItemInternal = {
          id: tabId,
          kind: "conversation",
          folderId,
          conversationId,
          agentType,
          title: resolvedTitle,
          isPinned: pin,
        }

        if (pin) {
          return [...prev, newTab]
        }

        const previewIndex = prev.findIndex((t) => !t.isPinned)
        if (previewIndex >= 0) {
          replacedPreviewTabId = prev[previewIndex].id
          const updated = [...prev]
          updated[previewIndex] = newTab
          return updated
        }

        return [...prev, newTab]
      })

      if (replacedPreviewTabId) {
        for (const cb of previewReplacedCallbacksRef.current) {
          cb(replacedPreviewTabId)
        }
      }

      if (activateTabId) {
        setActiveTabId(activateTabId)
      }
      activateConversationPane()
    },
    [activateConversationPane, t]
  )

  const makeReplacementDraftTab = useCallback(
    (preferred?: TabItemInternal): TabItemInternal => {
      const folderId = preferred?.folderId ?? foldersRef.current[0]?.id ?? 0
      const workingDir =
        preferred?.workingDir ??
        foldersRef.current.find((f) => f.id === folderId)?.path ??
        ""
      // If we have a preferred (closing) tab, inherit BOTH its agent and
      // its provisional flag — we should not silently launder a system
      // best-guess into a confirmed value just because the source tab was
      // closed. Otherwise resolve from scratch.
      const { agentType, provisional } = preferred?.agentType
        ? {
            agentType: preferred.agentType,
            provisional: preferred.agentTypeProvisional ?? false,
          }
        : resolveAgentForFolder(folderId, null)
      return {
        id: makeNewConversationTabId(),
        kind: "conversation",
        folderId,
        conversationId: null,
        agentType,
        title: t("newConversation"),
        isPinned: true,
        workingDir,
        agentTypeProvisional: provisional,
      }
    },
    [resolveAgentForFolder, t]
  )

  const [isTileMode, setIsTileMode] = useState(() => {
    if (typeof window === "undefined") return false
    try {
      return localStorage.getItem(TILE_MODE_STORAGE_KEY) === "true"
    } catch {
      return false
    }
  })

  useEffect(() => {
    try {
      localStorage.setItem(TILE_MODE_STORAGE_KEY, String(isTileMode))
    } catch {
      /* ignore */
    }
  }, [isTileMode])

  const closeTab = useCallback(
    (tabId: string) => {
      let neighborToSync: TabItemInternal | undefined
      let shouldReplaceWithEmpty = false

      setTabs((prev) => {
        const index = prev.findIndex((t) => t.id === tabId)
        if (index < 0) return prev

        const closingTab = prev[index]
        const next = prev.filter((t) => t.id !== tabId)

        if (next.length === 0) {
          if (foldersRef.current.length === 0) {
            shouldReplaceWithEmpty = true
            return []
          }
          const replacementTab = makeReplacementDraftTab(closingTab)
          neighborToSync = replacementTab
          return [replacementTab]
        }

        if (tabId === activeTabIdRef.current) {
          const newIndex = Math.min(index, next.length - 1)
          neighborToSync = next[newIndex]
        }

        return next
      })

      if (shouldReplaceWithEmpty) {
        setActiveTabId(null)
        return
      }

      if (neighborToSync) {
        setActiveTabId(neighborToSync.id)
        activateConversationPane()
      }
    },
    [activateConversationPane, makeReplacementDraftTab]
  )

  const closeConversationTab = useCallback(
    (folderId: number, conversationId: number, agentType: AgentType) => {
      const target = rawTabsRef.current.find(
        (tab) =>
          tab.folderId === folderId &&
          tab.conversationId === conversationId &&
          tab.agentType === agentType
      )
      if (!target) return
      closeTab(target.id)
    },
    [closeTab]
  )

  const closeOtherTabs = useCallback((tabId: string) => {
    setTabs((prev) => {
      const kept = prev.filter((t) => t.id === tabId)
      return kept.length === prev.length ? prev : kept
    })
    setActiveTabId(tabId)
  }, [])

  const closeAllTabs = useCallback(() => {
    const seedTab =
      rawTabsRef.current.find(
        (t) => t.conversationId == null && t.workingDir
      ) ??
      rawTabsRef.current.find((t) => t.id === activeTabIdRef.current) ??
      rawTabsRef.current[0]

    if (foldersRef.current.length === 0) {
      setTabs([])
      setActiveTabId(null)
      return
    }

    const replacementTab = makeReplacementDraftTab(seedTab)
    setTabs([replacementTab])
    setActiveTabId(replacementTab.id)
    activateConversationPane()
  }, [activateConversationPane, makeReplacementDraftTab])

  const closeTabsByFolder = useCallback((folderId: number) => {
    setTabs((prev) => {
      const remaining = prev.filter((t) => t.folderId !== folderId)
      if (remaining.length === prev.length) return prev

      // If active tab is being closed, move to first remaining tab
      const currentActive = activeTabIdRef.current
      const stillActive =
        currentActive != null && remaining.some((t) => t.id === currentActive)
      if (!stillActive) {
        setActiveTabId(remaining.length > 0 ? remaining[0].id : null)
      }
      return remaining
    })
  }, [])

  const switchTab = useCallback(
    (tabId: string) => {
      const tab = rawTabsRef.current.find((t) => t.id === tabId)
      if (!tab) return

      setActiveTabId(tabId)
      activateConversationPane()
    },
    [activateConversationPane]
  )

  const pinTab = useCallback((tabId: string) => {
    setTabs((prev) =>
      prev.map((t) => (t.id === tabId ? { ...t, isPinned: true } : t))
    )
  }, [])

  const toggleTileMode = useCallback(() => {
    setIsTileMode((prev) => !prev)
  }, [])

  const reorderTabs = useCallback(
    (reorderedTabs: TabItem[]) => setTabs(reorderedTabs),
    []
  )

  const openNewConversationTab = useCallback(
    (folderId: number, workingDir: string) => {
      // Pick the agent for the new conversation via the shared resolver.
      // Inherit from the active tab only when it's a real conversation
      // (so "new conversation" inside a Claude Code session stays on
      // Claude Code instead of snapping back to the global default);
      // never inherit from another draft, since its agent may itself be
      // provisional. AgentSelector will further pick the first available
      // agent if the chosen one turns out to be disabled or uninstalled.
      const activeTab = rawTabsRef.current.find(
        (t) => t.id === activeTabIdRef.current
      )
      const inherit =
        activeTab && activeTab.conversationId != null
          ? activeTab.agentType
          : null
      const { agentType: targetAgent, provisional } = resolveAgentForFolder(
        folderId,
        inherit
      )

      // Singleton: reuse any existing draft tab regardless of folder,
      // so only one new-conversation tab can exist at a time.
      const existingTab = rawTabsRef.current.find(
        (t) => t.conversationId == null
      )

      if (existingTab) {
        const folderChanged = existingTab.folderId !== folderId
        const workingDirChanged = existingTab.workingDir !== workingDir
        const agentChanged = existingTab.agentType !== targetAgent
        const provisionalChanged =
          (existingTab.agentTypeProvisional ?? false) !== provisional

        setActiveTabId(existingTab.id)
        activateConversationPane()

        if (folderChanged || agentChanged) {
          // Tear down the old ACP connection (bound to the old
          // workingDir/agent) before patching tab fields. The
          // connection-lifecycle effect watches workingDir and
          // agentType; once status has settled to disconnected and
          // either flips, it auto-reconnects against the new params.
          const expectedAgent = existingTab.agentType
          void (async () => {
            try {
              await acpDisconnect(existingTab.id)
            } catch (err) {
              console.error("[TabProvider] disconnect draft tab:", err)
            }
            // Race guard: if the tab was bound to a real conversation
            // (e.g. the user sent a message just before we got the
            // disconnect callback) or its agent was changed by another
            // path (confirmDraftAgent, correctDraftAgents) during the
            // await window, leave it alone.
            setTabs((prev) => {
              const target = prev.find((tab) => tab.id === existingTab.id)
              if (!target) return prev
              if (target.conversationId != null) return prev
              if (target.agentType !== expectedAgent) return prev
              return prev.map((tab) =>
                tab.id === existingTab.id
                  ? {
                      ...tab,
                      folderId,
                      workingDir,
                      agentType: targetAgent,
                      agentTypeProvisional: provisional,
                    }
                  : tab
              )
            })
          })()
        } else if (workingDirChanged || provisionalChanged) {
          setTabs((prev) =>
            prev.map((t) =>
              t.id === existingTab.id
                ? {
                    ...t,
                    workingDir,
                    agentTypeProvisional: provisional,
                  }
                : t
            )
          )
        }
        return
      }

      const tabId = makeNewConversationTabId()
      const newTab: TabItemInternal = {
        id: tabId,
        kind: "conversation",
        folderId,
        conversationId: null,
        agentType: targetAgent,
        title: t("newConversation"),
        isPinned: true,
        workingDir,
        agentTypeProvisional: provisional,
      }

      setTabs((prev) => [...prev, newTab])
      setActiveTabId(tabId)
      activateConversationPane()
    },
    [acpDisconnect, activateConversationPane, resolveAgentForFolder, t]
  )

  const confirmDraftAgent = useCallback(
    (tabId: string, agentType: AgentType) => {
      setTabs((prev) =>
        prev.map((t) => {
          if (t.id !== tabId) return t
          if (t.conversationId != null) return t // not a draft
          if (t.agentType === agentType && !t.agentTypeProvisional) return t
          return { ...t, agentType, agentTypeProvisional: false }
        })
      )
    },
    []
  )

  const bindConversationTab = useCallback(
    (
      tabId: string,
      conversationId: number,
      agentType: AgentType,
      title: string,
      runtimeConversationId?: number
    ) => {
      let nextActiveTabId: string | null = null
      setTabs((prev) =>
        prev.flatMap((tab) => {
          if (tab.id === tabId) {
            const nextTab: TabItemInternal = {
              ...tab,
              conversationId,
              agentType,
              title,
              runtimeConversationId,
              // Bound to a real conversation now — drop the provisional
              // hint so the correction effect never revisits it.
              agentTypeProvisional: false,
            }
            return [nextTab]
          }

          // Drop any other tab that already represents the same
          // (conversationId, agentType) — conversation IDs are globally
          // unique, so two tabs pointing at the same one would diverge
          // immediately. (The `tab.folderId === tab.folderId` tautology
          // that used to live here was a no-op; the dedupe was always
          // scoped to (conversationId, agentType).)
          if (
            tab.conversationId === conversationId &&
            tab.agentType === agentType
          ) {
            if (activeTabIdRef.current === tabId) {
              nextActiveTabId = tab.id
            }
            return []
          }

          return [tab]
        })
      )
      if (nextActiveTabId) {
        setActiveTabId(nextActiveTabId)
      }
    },
    []
  )

  const setTabRuntimeConversationId = useCallback(
    (tabId: string, runtimeConversationId: number) => {
      setTabs((prev) => {
        const target = prev.find((tab) => tab.id === tabId)
        if (!target || target.runtimeConversationId === runtimeConversationId) {
          return prev
        }
        return prev.map((tab) =>
          tab.id === tabId ? { ...tab, runtimeConversationId } : tab
        )
      })
    },
    []
  )

  // Once the agent list is fresh for the first time this session, fix up
  // any draft tabs whose agent was assigned from a stale cache or the
  // global fallback. Two cases need correction:
  //   1. agentTypeProvisional flag is set (system best-guess at creation)
  //   2. agentType is no longer in the fresh sorted list (hydrated draft
  //      whose agent has since been disabled or uninstalled)
  // Each correction runs in an independent async IIFE so the disconnect-
  // then-patch dance doesn't serialize across drafts. The IIFE
  // re-checks the tab's current `agentType` after the disconnect resolves;
  // if anything else patched it during the await (most notably
  // `confirmDraftAgent` from a user click), that write wins.
  // Runs at most once per session (correctionRanRef).
  const correctionRanRef = useRef(false)
  const correctDraftAgents = useCallback(() => {
    const candidates = rawTabsRef.current.filter((tab) => {
      if (tab.conversationId != null) return false
      if (tab.agentTypeProvisional) return true
      if (!sortedAvailableAgentsRef.current.includes(tab.agentType)) return true
      return false
    })
    if (candidates.length === 0) return

    for (const tab of candidates) {
      void (async () => {
        const { agentType: newAgent } = resolveAgentForFolder(
          tab.folderId,
          null
        )
        const current = rawTabsRef.current.find((t) => t.id === tab.id)
        if (!current || current.conversationId != null) return

        if (current.agentType === newAgent) {
          // Same value — nothing to disconnect/reconnect. If the tab was
          // flagged provisional (system best-guess that happened to land
          // on the right answer), clear the flag so future checks treat
          // it as confirmed.
          if (!current.agentTypeProvisional) return
          setTabs((prev) =>
            prev.map((t) =>
              t.id === tab.id &&
              t.conversationId == null &&
              t.agentTypeProvisional
                ? { ...t, agentTypeProvisional: false }
                : t
            )
          )
          return
        }

        // Agent changed — disconnect the old ACP session first, then
        // patch agentType. Connection lifecycle re-attaches against the
        // new agent once the patched tab prop reaches detail-panel.
        const expectedAgent = current.agentType
        try {
          await acpDisconnect(tab.id)
        } catch (err) {
          // Log and proceed. Backend disconnect rejects when the front-
          // end and backend connection registries briefly diverge (e.g.
          // tab created but ACP session never finished spinning up);
          // returning here would leave the draft stuck on the wrong
          // agent because `correctionRanRef` is one-shot per session.
          // The race guard below still protects a concurrent user click.
          // This mirrors `openNewConversationTab`'s disconnect dance.
          console.error("[TabProvider] correct provisional disconnect:", err)
        }

        // Race guard: if `agentType` changed during the await (user
        // clicked a different agent → `confirmDraftAgent`), their choice
        // wins. We can't gate on `agentTypeProvisional` here because
        // hydrated drafts are never provisional — but they DO need
        // correcting when their agent has been disabled/uninstalled,
        // which is exactly when `agentType !== expectedAgent` stays
        // false (nobody else touched it).
        setTabs((prev) => {
          const target = prev.find((t) => t.id === tab.id)
          if (!target) return prev
          if (target.conversationId != null) return prev
          if (target.agentType !== expectedAgent) return prev
          return prev.map((t) =>
            t.id === tab.id
              ? { ...t, agentType: newAgent, agentTypeProvisional: false }
              : t
          )
        })
      })()
    }
  }, [acpDisconnect, resolveAgentForFolder])

  // Correction must wait for BOTH `agentsFresh` (so the sorted list is
  // real) AND `tabsHydrated` (so any persisted drafts are already in
  // `rawTabs`). Without the second gate, correction can fire against an
  // empty rawTabs (immediately after mount but before hydration resolves)
  // and then never re-run because `correctionRanRef` is one-shot — leaving
  // hydrated drafts whose agent has since been disabled stuck on the
  // wrong agent.
  //
  // No timer-based fallback: if `acpListAgents()` never succeeds this
  // session, drafts simply keep their `agentTypeProvisional` hint. The
  // flag is internal-only (no UI consumer reads it) and is cleared
  // unconditionally by `bindConversationTab` and `confirmDraftAgent`, so
  // leaving it set is safer than racing to clear it and risking a "fresh
  // arrived late" case where we'd no longer be able to identify which
  // drafts came from a stale seed.
  useEffect(() => {
    if (correctionRanRef.current) return
    if (!agentsFresh) return
    if (!tabsHydrated) return
    correctionRanRef.current = true
    correctDraftAgents()
  }, [agentsFresh, tabsHydrated, correctDraftAgents])

  const value = useMemo(
    () => ({
      tabs,
      activeTabId,
      tabsHydrated,
      isTileMode,
      openTab,
      closeTab,
      closeConversationTab,
      closeOtherTabs,
      closeAllTabs,
      closeTabsByFolder,
      switchTab,
      pinTab,
      toggleTileMode,
      openNewConversationTab,
      confirmDraftAgent,
      bindConversationTab,
      setTabRuntimeConversationId,
      reorderTabs,
      onPreviewTabReplaced,
    }),
    [
      tabs,
      activeTabId,
      tabsHydrated,
      isTileMode,
      openTab,
      closeTab,
      closeConversationTab,
      closeOtherTabs,
      closeAllTabs,
      closeTabsByFolder,
      switchTab,
      pinTab,
      toggleTileMode,
      openNewConversationTab,
      confirmDraftAgent,
      bindConversationTab,
      setTabRuntimeConversationId,
      reorderTabs,
      onPreviewTabReplaced,
    ]
  )

  return <TabContext.Provider value={value}>{children}</TabContext.Provider>
}
