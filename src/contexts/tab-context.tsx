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
import { useFolderContext } from "@/contexts/folder-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { saveFolderOpenedConversations } from "@/lib/tauri"
import type {
  AgentType,
  ConversationStatus,
  OpenedConversation,
} from "@/lib/types"

interface TabItemInternal {
  id: string
  kind: "conversation"
  conversationId: number | null
  agentType: AgentType
  title: string
  isPinned: boolean
  workingDir?: string
  status?: ConversationStatus
}

export type TabItem = TabItemInternal

interface TabContextValue {
  tabs: TabItem[]
  activeTabId: string | null
  isTileMode: boolean
  openTab: (
    conversationId: number,
    agentType: AgentType,
    pin?: boolean,
    title?: string
  ) => void
  closeTab: (tabId: string) => void
  closeConversationTab: (conversationId: number, agentType: AgentType) => void
  closeOtherTabs: (tabId: string) => void
  closeAllTabs: () => void
  switchTab: (tabId: string) => void
  pinTab: (tabId: string) => void
  toggleTileMode: () => void
  openNewConversationTab: (agentType: AgentType, workingDir: string) => void
  bindConversationTab: (
    tabId: string,
    conversationId: number,
    agentType: AgentType,
    title: string
  ) => void
  reorderTabs: (reorderedTabs: TabItem[]) => void
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
  agentType: AgentType,
  conversationId: number
): string {
  return `conv-${agentType}-${conversationId}`
}

function makeNewConversationTabId(): string {
  return `new-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}
function findTabIndexForConversation(
  tabs: TabItemInternal[],
  agentType: AgentType,
  conversationId: number
): number {
  const canonicalId = makeConversationTabId(agentType, conversationId)
  const idx = tabs.findIndex((t) => t.id === canonicalId)
  if (idx >= 0) return idx
  return tabs.findIndex(
    (t) => t.conversationId === conversationId && t.agentType === agentType
  )
}

interface TabProviderProps {
  children: ReactNode
}

export function TabProvider({ children }: TabProviderProps) {
  const t = useTranslations("Folder.tabContext")
  const { activateConversationPane } = useWorkspaceContext()
  const {
    folder,
    folderId,
    selectedConversation,
    selectConversation,
    clearSelection,
    startNewConversation,
    cancelNewConversation,
    conversations,
  } = useFolderContext()

  const [rawTabs, setTabs] = useState<TabItemInternal[]>(() => {
    if (selectedConversation) {
      const tabId = makeConversationTabId(
        selectedConversation.agentType,
        selectedConversation.id
      )
      return [
        {
          id: tabId,
          kind: "conversation",
          conversationId: selectedConversation.id,
          agentType: selectedConversation.agentType,
          title: t("loadingConversation"),
          isPinned: true,
        },
      ]
    }
    return []
  })

  const [activeTabId, setActiveTabId] = useState<string | null>(() => {
    if (selectedConversation) {
      return makeConversationTabId(
        selectedConversation.agentType,
        selectedConversation.id
      )
    }
    return null
  })

  // Refs for volatile state — used in callbacks to avoid re-creation
  const activeTabIdRef = useRef(activeTabId)
  useEffect(() => {
    activeTabIdRef.current = activeTabId
  }, [activeTabId])

  const rawTabsRef = useRef(rawTabs)
  useEffect(() => {
    rawTabsRef.current = rawTabs
  }, [rawTabs])

  const conversationsRef = useRef(conversations)
  useEffect(() => {
    conversationsRef.current = conversations
  }, [conversations])

  // Restore tabs from folder.opened_conversations when folder first loads
  const [restoredFolderId, setRestoredFolderId] = useState<number | null>(() =>
    selectedConversation ? folderId : null
  )

  useEffect(() => {
    if (!folder) return
    if (restoredFolderId === folder.id) return

    let cancelled = false
    queueMicrotask(() => {
      if (cancelled) return

      setRestoredFolderId(folder.id)

      const opened = folder.opened_conversations
      if (opened.length === 0) return

      const restoredTabs: TabItemInternal[] = opened.map((oc) => ({
        id: makeConversationTabId(oc.agent_type, oc.conversation_id),
        kind: "conversation",
        conversationId: oc.conversation_id,
        agentType: oc.agent_type,
        title: t("loadingConversation"),
        isPinned: oc.is_pinned,
      }))

      setTabs(restoredTabs)

      const activeItem = opened.find((oc) => oc.is_active)
      const target = activeItem ?? opened[0]
      setActiveTabId(
        makeConversationTabId(target.agent_type, target.conversation_id)
      )
    })

    return () => {
      cancelled = true
    }
  }, [folder, restoredFolderId, t])

  // Sync restored active tab to FolderProvider (deferred to avoid
  // updating parent during child render)
  const prevRestoredIdRef = useRef(restoredFolderId)
  useEffect(() => {
    if (restoredFolderId === prevRestoredIdRef.current) return
    prevRestoredIdRef.current = restoredFolderId

    if (!folder || folder.opened_conversations.length === 0) return
    const opened = folder.opened_conversations
    const target = opened.find((oc) => oc.is_active) ?? opened[0]
    selectConversation(target.conversation_id, target.agent_type)
  }, [restoredFolderId, folder, selectConversation])

  // Debounced save to DB
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const skipSaveRef = useRef(true) // skip saving until first restore completes

  useEffect(() => {
    // Skip the initial render and restoration phase
    if (skipSaveRef.current) {
      if (restoredFolderId != null) {
        skipSaveRef.current = false
      }
      return
    }

    if (saveTimerRef.current) {
      clearTimeout(saveTimerRef.current)
    }

    saveTimerRef.current = setTimeout(() => {
      const items: OpenedConversation[] = rawTabs
        .filter(
          (t): t is TabItemInternal & { conversationId: number } =>
            t.conversationId != null
        )
        .map((t, i) => ({
          conversation_id: t.conversationId,
          agent_type: t.agentType,
          position: i,
          is_active: t.id === activeTabId,
          is_pinned: t.isPinned,
        }))

      saveFolderOpenedConversations(folderId, items).catch(() => {
        // Silently ignore save errors
      })
    }, 500)

    return () => {
      if (saveTimerRef.current) {
        clearTimeout(saveTimerRef.current)
      }
    }
  }, [rawTabs, activeTabId, folderId, restoredFolderId])

  // Pre-index conversations for O(1) lookup in tabs derivation
  const conversationMap = useMemo(() => {
    const m = new Map<string, (typeof conversations)[number]>()
    for (const c of conversations) {
      m.set(`${c.agent_type}-${c.id}`, c)
    }
    return m
  }, [conversations])

  // Derive tabs with up-to-date titles and status from conversations
  const tabs = useMemo(() => {
    if (conversationMap.size === 0) return rawTabs
    return rawTabs.map((tab) => {
      if (tab.conversationId != null) {
        const conv = conversationMap.get(
          `${tab.agentType}-${tab.conversationId}`
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

  const syncFolderContext = useCallback(
    (tab: TabItem | null) => {
      if (!tab) {
        clearSelection()
        cancelNewConversation()
        return
      }
      if (tab.conversationId != null) {
        selectConversation(tab.conversationId, tab.agentType)
      } else {
        const workingDir = tab.workingDir ?? folder?.path
        if (!workingDir) {
          clearSelection()
          cancelNewConversation()
          return
        }
        startNewConversation(tab.agentType, workingDir)
      }
    },
    [
      folder?.path,
      selectConversation,
      clearSelection,
      startNewConversation,
      cancelNewConversation,
    ]
  )

  const openTab = useCallback(
    (
      conversationId: number,
      agentType: AgentType,
      pin = false,
      title?: string
    ) => {
      let activateTabId: string | undefined

      setTabs((prev) => {
        const existingIndex = findTabIndexForConversation(
          prev,
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

        // Resolve title from conversations list (via ref)
        const resolvedTitle =
          title ??
          conversationsRef.current.find(
            (c) => c.id === conversationId && c.agent_type === agentType
          )?.title ??
          t("untitledConversation")

        const tabId = makeConversationTabId(agentType, conversationId)
        activateTabId = tabId
        const newTab: TabItemInternal = {
          id: tabId,
          kind: "conversation",
          conversationId,
          agentType,
          title: resolvedTitle,
          isPinned: pin,
        }

        if (pin) {
          return [...prev, newTab]
        }

        // Preview (not pinned): replace existing preview tab
        const previewIndex = prev.findIndex((t) => !t.isPinned)
        if (previewIndex >= 0) {
          const updated = [...prev]
          updated[previewIndex] = newTab
          return updated
        }

        return [...prev, newTab]
      })

      if (activateTabId) {
        setActiveTabId(activateTabId)
      }
      selectConversation(conversationId, agentType)
      activateConversationPane()
    },
    [activateConversationPane, selectConversation, t]
  )

  const makeReplacementDraftTab = useCallback(
    (preferred?: TabItemInternal): TabItemInternal => ({
      id: makeNewConversationTabId(),
      kind: "conversation",
      conversationId: null,
      agentType: preferred?.agentType ?? "codex",
      title: t("newConversation"),
      isPinned: true,
      workingDir: preferred?.workingDir ?? folder?.path,
    }),
    [folder?.path, t]
  )

  const [isTileMode, setIsTileMode] = useState(false)

  const closeTab = useCallback(
    (tabId: string) => {
      let neighborToSync: TabItemInternal | undefined

      setTabs((prev) => {
        const index = prev.findIndex((t) => t.id === tabId)
        if (index < 0) return prev

        const closingTab = prev[index]
        const next = prev.filter((t) => t.id !== tabId)

        if (next.length === 0) {
          const replacementTab = makeReplacementDraftTab(closingTab)
          neighborToSync = replacementTab
          return [replacementTab]
        }

        // If closing the active tab, pick a neighbor to activate
        if (tabId === activeTabIdRef.current) {
          // Prefer right neighbor, then left
          const newIndex = Math.min(index, next.length - 1)
          neighborToSync = next[newIndex]
        }

        return next
      })

      // Sync folder context outside the updater to avoid
      // updating FolderProvider state during TabProvider render
      if (neighborToSync) {
        setActiveTabId(neighborToSync.id)
        syncFolderContext(neighborToSync)
        activateConversationPane()
      }
    },
    [activateConversationPane, makeReplacementDraftTab, syncFolderContext]
  )

  const closeConversationTab = useCallback(
    (conversationId: number, agentType: AgentType) => {
      const target = rawTabsRef.current.find(
        (tab) =>
          tab.conversationId === conversationId && tab.agentType === agentType
      )
      if (!target) return
      closeTab(target.id)
    },
    [closeTab]
  )

  const closeOtherTabs = useCallback(
    (tabId: string) => {
      setTabs((prev) => {
        const kept = prev.filter((t) => t.id === tabId)
        return kept.length === prev.length ? prev : kept
      })
      setIsTileMode(false)

      const tab = rawTabsRef.current.find((t) => t.id === tabId)
      if (tab) {
        setActiveTabId(tabId)
        syncFolderContext(tab)
      }
    },
    [syncFolderContext]
  )

  const closeAllTabs = useCallback(() => {
    const seedTab =
      rawTabsRef.current.find(
        (t) => t.conversationId == null && t.workingDir
      ) ??
      rawTabsRef.current.find((t) => t.id === activeTabIdRef.current) ??
      rawTabsRef.current[0]

    const replacementTab = makeReplacementDraftTab(seedTab)
    setTabs([replacementTab])
    setIsTileMode(false)
    setActiveTabId(replacementTab.id)
    syncFolderContext(replacementTab)
    activateConversationPane()
  }, [activateConversationPane, makeReplacementDraftTab, syncFolderContext])

  const switchTab = useCallback(
    (tabId: string) => {
      const tab = rawTabsRef.current.find((t) => t.id === tabId)
      if (!tab) return

      setActiveTabId(tabId)
      syncFolderContext(tab)
      activateConversationPane()
    },
    [activateConversationPane, syncFolderContext]
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
    (agentType: AgentType, workingDir: string) => {
      const existingTab = rawTabsRef.current.find(
        (t) => t.conversationId == null && t.agentType === agentType
      )

      if (existingTab) {
        setActiveTabId(existingTab.id)
        syncFolderContext(existingTab)
        activateConversationPane()
        return
      }

      const tabId = makeNewConversationTabId()
      const newTab: TabItemInternal = {
        id: tabId,
        kind: "conversation",
        conversationId: null,
        agentType,
        title: t("newConversation"),
        isPinned: true,
        workingDir,
      }

      setTabs((prev) => [...prev, newTab])
      setActiveTabId(tabId)
      startNewConversation(agentType, workingDir)
      activateConversationPane()
    },
    [activateConversationPane, startNewConversation, syncFolderContext, t]
  )

  const bindConversationTab = useCallback(
    (
      tabId: string,
      conversationId: number,
      agentType: AgentType,
      title: string
    ) => {
      let nextActiveTabId: string | null = null
      setTabs((prev) =>
        prev.flatMap((tab) => {
          if (tab.id === tabId) {
            const nextTab = { ...tab, conversationId, agentType, title }
            return [nextTab]
          }

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
        const target = rawTabsRef.current.find(
          (tab) => tab.id === nextActiveTabId
        )
        if (target) {
          syncFolderContext(target)
        }
      }
    },
    [syncFolderContext]
  )

  const value = useMemo(
    () => ({
      tabs,
      activeTabId,
      isTileMode,
      openTab,
      closeTab,
      closeConversationTab,
      closeOtherTabs,
      closeAllTabs,
      switchTab,
      pinTab,
      toggleTileMode,
      openNewConversationTab,
      bindConversationTab,
      reorderTabs,
    }),
    [
      tabs,
      activeTabId,
      isTileMode,
      openTab,
      closeTab,
      closeConversationTab,
      closeOtherTabs,
      closeAllTabs,
      switchTab,
      pinTab,
      toggleTileMode,
      openNewConversationTab,
      bindConversationTab,
      reorderTabs,
    ]
  )

  return <TabContext.Provider value={value}>{children}</TabContext.Provider>
}
