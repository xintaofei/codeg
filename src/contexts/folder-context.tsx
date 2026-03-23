"use client"

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  useCallback,
  useRef,
  type ReactNode,
} from "react"
import { toErrorMessage } from "@/lib/app-error"
import { getFolder, listFolderConversations } from "@/lib/tauri"
import type {
  AgentType,
  AgentStats,
  DbConversationSummary,
  FolderDetail,
} from "@/lib/types"

interface SelectedConversation {
  id: number
  agentType: AgentType
}

interface NewConversationState {
  workingDir: string
}

interface FolderContextValue {
  folder: FolderDetail | null
  folderId: number
  folderLoading: boolean

  conversations: DbConversationSummary[]
  loading: boolean
  refreshing: boolean
  error: string | null

  selectedConversation: SelectedConversation | null
  selectConversation: (id: number, agentType: AgentType) => void
  clearSelection: () => void

  newConversation: NewConversationState | null
  startNewConversation: (workingDir: string) => void
  cancelNewConversation: () => void

  stats: AgentStats | null

  refreshConversations: () => void
}

const FolderContext = createContext<FolderContextValue | null>(null)

export function useFolderContext() {
  const ctx = useContext(FolderContext)
  if (!ctx) {
    throw new Error("useFolderContext must be used within FolderProvider")
  }
  return ctx
}

function computeStats(conversations: DbConversationSummary[]): AgentStats {
  const byAgent = new Map<AgentType, number>()
  let totalMessages = 0

  for (const s of conversations) {
    byAgent.set(s.agent_type, (byAgent.get(s.agent_type) ?? 0) + 1)
    totalMessages += s.message_count
  }

  return {
    total_conversations: conversations.length,
    total_messages: totalMessages,
    by_agent: Array.from(byAgent.entries()).map(([agent_type, count]) => ({
      agent_type,
      conversation_count: count,
    })),
  }
}

/** Module-level cache — survives component unmounts / page navigations. */
const cache = new Map<string, DbConversationSummary[]>()

interface FolderProviderProps {
  children: ReactNode
  folderId: number
  initialConversationId?: number | null
  initialAgentType?: AgentType | null
}

export function FolderProvider({
  children,
  folderId,
  initialConversationId,
  initialAgentType,
}: FolderProviderProps) {
  // Folder info
  const [folder, setFolder] = useState<FolderDetail | null>(null)
  const [folderLoading, setFolderLoading] = useState(true)

  // Conversations
  const cacheKey = String(folderId)
  const [conversations, setConversations] = useState<DbConversationSummary[]>(
    () => cache.get(cacheKey) ?? []
  )
  const [loading, setLoading] = useState(() => !cache.has(cacheKey))
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [selectedConversation, setSelectedConversation] =
    useState<SelectedConversation | null>(() => {
      if (initialConversationId != null && initialAgentType) {
        return { id: initialConversationId, agentType: initialAgentType }
      }
      return null
    })

  // Sync selection when URL params change (e.g. navigation)
  useEffect(() => {
    if (initialConversationId != null && initialAgentType) {
      setSelectedConversation({
        id: initialConversationId,
        agentType: initialAgentType,
      })
    }
  }, [initialConversationId, initialAgentType])
  const [newConversation, setNewConversation] =
    useState<NewConversationState | null>(null)

  const mountedRef = useRef(true)

  // Fetch folder info
  useEffect(() => {
    let cancelled = false
    setFolderLoading(true)
    getFolder(folderId)
      .then((f) => {
        if (!cancelled) {
          setFolder(f)
          setFolderLoading(false)
        }
      })
      .catch((err) => {
        console.error("[FolderProvider] getFolder failed:", err)
        if (!cancelled) setFolderLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [folderId])

  const fetchConversations = useCallback(async () => {
    const cached = cache.get(cacheKey)

    if (cached) {
      setConversations(cached)
      setLoading(false)
      setRefreshing(true)
    } else {
      setLoading(true)
    }

    try {
      setError(null)
      const data = await listFolderConversations({
        folder_id: folderId,
        status: null,
      })
      if (!mountedRef.current) return
      cache.set(cacheKey, data)
      setConversations(data)
    } catch (e) {
      if (!mountedRef.current) return
      if (!cached) {
        setError(toErrorMessage(e))
      }
    } finally {
      if (mountedRef.current) {
        setLoading(false)
        setRefreshing(false)
      }
    }
  }, [folderId, cacheKey])

  useEffect(() => {
    fetchConversations()
  }, [fetchConversations])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  const selectConversation = useCallback((id: number, agentType: AgentType) => {
    setSelectedConversation({ id, agentType })
    setNewConversation(null)
  }, [])

  const clearSelection = useCallback(() => {
    setSelectedConversation(null)
  }, [])

  const startNewConversation = useCallback((workingDir: string) => {
    setNewConversation({ workingDir })
    setSelectedConversation(null)
  }, [])

  const cancelNewConversation = useCallback(() => {
    setNewConversation(null)
  }, [])

  const refreshConversations = useCallback(() => {
    cache.delete(cacheKey)
    fetchConversations()
  }, [cacheKey, fetchConversations])

  const stats = useMemo(
    () => (conversations.length > 0 ? computeStats(conversations) : null),
    [conversations]
  )

  const value = useMemo<FolderContextValue>(
    () => ({
      folder,
      folderId,
      folderLoading,
      conversations,
      loading,
      refreshing,
      error,
      selectedConversation,
      selectConversation,
      clearSelection,
      newConversation,
      startNewConversation,
      cancelNewConversation,
      stats,
      refreshConversations,
    }),
    [
      folder,
      folderId,
      folderLoading,
      conversations,
      loading,
      refreshing,
      error,
      selectedConversation,
      selectConversation,
      clearSelection,
      newConversation,
      startNewConversation,
      cancelNewConversation,
      stats,
      refreshConversations,
    ]
  )

  return (
    <FolderContext.Provider value={value}>{children}</FolderContext.Provider>
  )
}
