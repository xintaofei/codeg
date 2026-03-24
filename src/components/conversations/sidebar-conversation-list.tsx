"use client"

import {
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type Ref,
} from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Virtualizer, type VirtualizerHandle } from "virtua"
import { CheckCheck, ChevronRight, Download, Loader2, Plus } from "lucide-react"
import { useFolderContext } from "@/contexts/folder-context"
import { useTabContext } from "@/contexts/tab-context"
import { useTaskContext } from "@/contexts/task-context"
import {
  importLocalConversations,
  updateConversationTitle,
  updateConversationStatus,
  deleteConversation,
} from "@/lib/tauri"
import type { ConversationStatus, DbConversationSummary } from "@/lib/types"
import { STATUS_ORDER, STATUS_COLORS } from "@/lib/types"
import { SidebarConversationCard } from "./sidebar-conversation-card"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from "@/components/ui/context-menu"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { cn } from "@/lib/utils"

function parseTimestamp(value: string): number {
  const timestamp = Date.parse(value)
  return Number.isNaN(timestamp) ? 0 : timestamp
}

function compareByUpdatedAtDesc(
  left: DbConversationSummary,
  right: DbConversationSummary
): number {
  const updatedDiff =
    parseTimestamp(right.updated_at) - parseTimestamp(left.updated_at)
  if (updatedDiff !== 0) return updatedDiff

  const createdDiff =
    parseTimestamp(right.created_at) - parseTimestamp(left.created_at)
  if (createdDiff !== 0) return createdDiff

  return right.id - left.id
}

type FlatItem =
  | { type: "header"; status: ConversationStatus; count: number }
  | { type: "conversation"; conversation: DbConversationSummary }

const CARD_HEIGHT = 62

const GroupHeader = memo(function GroupHeader({
  status,
  count,
  isOpen,
  onToggle,
  tStatus,
}: {
  status: ConversationStatus
  count: number
  isOpen: boolean
  onToggle: (status: ConversationStatus) => void
  tStatus: ReturnType<typeof useTranslations>
}) {
  return (
    <button
      onClick={() => onToggle(status)}
      className="flex items-center gap-1.5 w-full px-1.5 py-1.5 text-xs font-medium text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
    >
      <ChevronRight
        className={cn(
          "h-3.5 w-3.5 shrink-0 transition-transform",
          isOpen && "rotate-90"
        )}
      />
      <span
        className={cn("w-2 h-2 rounded-full shrink-0", STATUS_COLORS[status])}
      />
      <span>{tStatus(status)}</span>
      <span className="text-muted-foreground/60 tabular-nums">({count})</span>
    </button>
  )
})

const PendingReviewHeader = memo(function PendingReviewHeader({
  count,
  isOpen,
  onToggle,
  reviewConversationCount,
  completingReview,
  onCompleteReview,
  tStatus,
  t,
}: {
  count: number
  isOpen: boolean
  onToggle: (status: ConversationStatus) => void
  reviewConversationCount: number
  completingReview: boolean
  onCompleteReview: () => void
  tStatus: ReturnType<typeof useTranslations>
  t: ReturnType<typeof useTranslations>
}) {
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <button
          onClick={() => onToggle("pending_review")}
          className="flex items-center gap-1.5 w-full px-1.5 py-1.5 text-xs font-medium text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
        >
          <ChevronRight
            className={cn(
              "h-3.5 w-3.5 shrink-0 transition-transform",
              isOpen && "rotate-90"
            )}
          />
          <span
            className={cn(
              "w-2 h-2 rounded-full shrink-0",
              STATUS_COLORS.pending_review
            )}
          />
          <span>{tStatus("pending_review")}</span>
          <span className="text-muted-foreground/60 tabular-nums">
            ({count})
          </span>
        </button>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem
          disabled={reviewConversationCount === 0 || completingReview}
          onSelect={onCompleteReview}
        >
          <CheckCheck className="h-4 w-4" />
          {t("completeAllSessions")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
})

export interface SidebarConversationListHandle {
  scrollToActive: () => void
  expandAll: () => void
  collapseAll: () => void
}

export function SidebarConversationList({
  ref,
}: {
  ref?: Ref<SidebarConversationListHandle>
}) {
  const t = useTranslations("Folder.sidebar")
  const tStatus = useTranslations("Folder.statusLabels")
  const tCommon = useTranslations("Folder.common")
  const {
    folder,
    conversations,
    loading,
    refreshing,
    error,
    selectedConversation,
    folderId,
    refreshConversations,
  } = useFolderContext()

  const { openTab, closeConversationTab, openNewConversationTab } =
    useTabContext()
  const { addTask, updateTask } = useTaskContext()

  const [importing, setImporting] = useState(false)
  const [completeReviewOpen, setCompleteReviewOpen] = useState(false)
  const [completingReview, setCompletingReview] = useState(false)
  const [groupExpanded, setGroupExpanded] = useState<
    Record<ConversationStatus, boolean>
  >({
    in_progress: true,
    pending_review: true,
    completed: false,
    cancelled: false,
  })

  const scrollContainerRef = useRef<HTMLDivElement>(null)

  const scrollToActiveRef = useRef<() => void>(() => {})
  const pendingScrollRef = useRef(false)
  const virtualizerRef = useRef<VirtualizerHandle>(null)

  useImperativeHandle(ref, () => ({
    scrollToActive() {
      scrollToActiveRef.current()
    },
    expandAll() {
      setGroupExpanded({
        in_progress: true,
        pending_review: true,
        completed: true,
        cancelled: true,
      })
    },
    collapseAll() {
      setGroupExpanded({
        in_progress: false,
        pending_review: false,
        completed: false,
        cancelled: false,
      })
    },
  }))

  const grouped = useMemo(() => {
    const map = new Map<ConversationStatus, DbConversationSummary[]>()
    for (const conv of conversations) {
      const status = conv.status as ConversationStatus
      const list = map.get(status)
      if (list) {
        list.push(conv)
      } else {
        map.set(status, [conv])
      }
    }
    for (const list of map.values()) {
      list.sort(compareByUpdatedAtDesc)
    }
    return map
  }, [conversations])

  const flatItems = useMemo<FlatItem[]>(() => {
    const items: FlatItem[] = []
    for (const status of STATUS_ORDER) {
      const list = grouped.get(status)
      if (!list || list.length === 0) continue
      items.push({ type: "header", status, count: list.length })
      if (groupExpanded[status]) {
        for (const conv of list) {
          items.push({ type: "conversation", conversation: conv })
        }
      }
    }
    return items
  }, [grouped, groupExpanded])

  const reviewConversations = useMemo(
    () => grouped.get("pending_review") ?? [],
    [grouped]
  )
  const reviewConversationCount = reviewConversations.length

  useEffect(() => {
    scrollToActiveRef.current = () => {
      if (!selectedConversation) return
      const targetId = selectedConversation.id
      const targetAgent = selectedConversation.agentType
      const conv = conversations.find(
        (c) => c.id === targetId && c.agent_type === targetAgent
      )
      if (!conv) return
      const status = conv.status as ConversationStatus
      if (!groupExpanded[status]) {
        setGroupExpanded((prev) => ({ ...prev, [status]: true }))
        pendingScrollRef.current = true
        return
      }
      const index = flatItems.findIndex(
        (item) =>
          item.type === "conversation" &&
          item.conversation.id === targetId &&
          item.conversation.agent_type === targetAgent
      )
      if (index >= 0) {
        virtualizerRef.current?.scrollToIndex(index, {
          align: "center",
          smooth: true,
        })
      }
    }

    if (pendingScrollRef.current) {
      pendingScrollRef.current = false
      scrollToActiveRef.current()
    }
  }, [selectedConversation, flatItems, conversations, groupExpanded])

  const toggleGroup = useCallback((status: ConversationStatus) => {
    setGroupExpanded((prev) => ({ ...prev, [status]: !prev[status] }))
  }, [])

  const handleOpenCompleteReview = useCallback(
    () => setCompleteReviewOpen(true),
    []
  )

  const handleSelect = useCallback(
    (id: number, agentType: string) => {
      openTab(id, agentType as Parameters<typeof openTab>[1], false)
    },
    [openTab]
  )

  const handleDoubleClick = useCallback(
    (id: number, agentType: string) => {
      openTab(id, agentType as Parameters<typeof openTab>[1], true)
    },
    [openTab]
  )

  const handleRename = useCallback(
    async (id: number, newTitle: string) => {
      await updateConversationTitle(id, newTitle)
      refreshConversations()
    },
    [refreshConversations]
  )

  const handleDelete = useCallback(
    async (id: number, agentType: string) => {
      await deleteConversation(id)
      closeConversationTab(id, agentType as Parameters<typeof openTab>[1])
      refreshConversations()
    },
    [closeConversationTab, refreshConversations]
  )

  const handleStatusChange = useCallback(
    async (id: number, status: ConversationStatus) => {
      await updateConversationStatus(id, status)
      refreshConversations()
    },
    [refreshConversations]
  )

  const handleNewConversation = useCallback(() => {
    if (!folder) return
    openNewConversationTab(folder.path)
  }, [folder, openNewConversationTab])

  const handleImport = useCallback(async () => {
    if (importing) return
    setImporting(true)
    const taskId = `import-${folderId}-${Date.now()}`
    addTask(taskId, t("importLocalSessions"))
    updateTask(taskId, { status: "running" })
    try {
      const result = await importLocalConversations(folderId)
      updateTask(taskId, { status: "completed" })
      refreshConversations()
      if (result.imported > 0) {
        toast.success(
          t("toasts.importedSessions", {
            imported: result.imported,
            skipped: result.skipped,
          })
        )
      } else {
        toast.info(t("toasts.noNewSessionsFound", { skipped: result.skipped }))
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      updateTask(taskId, { status: "failed", error: msg })
      toast.error(t("toasts.importFailed", { message: msg }))
    } finally {
      setImporting(false)
    }
  }, [importing, folderId, addTask, updateTask, refreshConversations, t])

  const handleCompleteAllReview = useCallback(async () => {
    if (completingReview || reviewConversationCount === 0) return
    setCompletingReview(true)
    try {
      await Promise.all(
        reviewConversations.map((conversation) =>
          updateConversationStatus(conversation.id, "completed")
        )
      )
      refreshConversations()
      toast.success(
        t("toasts.reviewCompleted", { count: reviewConversationCount })
      )
      setCompleteReviewOpen(false)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      toast.error(t("toasts.completeReviewFailed", { message: msg }))
    } finally {
      setCompletingReview(false)
    }
  }, [
    completingReview,
    reviewConversationCount,
    reviewConversations,
    refreshConversations,
    t,
  ])

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {(loading || refreshing) && (
        <div className="flex items-center justify-center py-1">
          <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
        </div>
      )}

      {loading && !refreshing ? (
        <div className="px-3 space-y-1.5 overflow-hidden">
          {Array.from({ length: 6 }).map((_, i) => (
            <Skeleton key={i} className="h-14 w-full rounded-md" />
          ))}
        </div>
      ) : error ? (
        <div className="flex-1 flex items-center justify-center px-3">
          <p className="text-destructive text-xs">
            {t("error", { message: error })}
          </p>
        </div>
      ) : conversations.length === 0 ? (
        <ContextMenu>
          <ContextMenuTrigger asChild>
            <div className="flex-1 flex flex-col items-center justify-center px-3 gap-3">
              <p className="text-muted-foreground text-xs text-center">
                {t("noConversationsFound")}
              </p>
              <Button
                variant="outline"
                size="sm"
                disabled={importing}
                onClick={handleImport}
              >
                {importing ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" />
                ) : (
                  <Download className="h-3.5 w-3.5 mr-1.5" />
                )}
                {importing ? t("importing") : t("importLocalSessions")}
              </Button>
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem onSelect={handleNewConversation}>
              <Plus className="h-4 w-4" />
              {t("newConversation")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem disabled={importing} onSelect={handleImport}>
              <Download className="h-4 w-4" />
              {importing ? t("importing") : t("importLocalSessions")}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      ) : (
        <ContextMenu>
          <ContextMenuTrigger asChild>
            <div
              ref={scrollContainerRef}
              className={cn(
                "flex-1 min-h-0 overflow-y-auto px-1.5",
                "[overflow-anchor:none]",
                "[&::-webkit-scrollbar]:w-1.5",
                "[&::-webkit-scrollbar-thumb]:rounded-full",
                "[&::-webkit-scrollbar-thumb]:bg-border"
              )}
            >
              <Virtualizer ref={virtualizerRef} itemSize={CARD_HEIGHT}>
                {flatItems.map((item) => {
                  const key =
                    item.type === "header"
                      ? `header-${item.status}`
                      : `conv-${item.conversation.id}`
                  return (
                    <div key={key}>
                      {item.type === "header" ? (
                        item.status === "pending_review" ? (
                          <PendingReviewHeader
                            count={item.count}
                            isOpen={groupExpanded[item.status]}
                            onToggle={toggleGroup}
                            reviewConversationCount={reviewConversationCount}
                            completingReview={completingReview}
                            onCompleteReview={handleOpenCompleteReview}
                            tStatus={tStatus}
                            t={t}
                          />
                        ) : (
                          <GroupHeader
                            status={item.status}
                            count={item.count}
                            isOpen={groupExpanded[item.status]}
                            onToggle={toggleGroup}
                            tStatus={tStatus}
                          />
                        )
                      ) : (
                        <SidebarConversationCard
                          conversation={item.conversation}
                          isSelected={
                            selectedConversation?.agentType ===
                              item.conversation.agent_type &&
                            selectedConversation?.id === item.conversation.id
                          }
                          onSelect={handleSelect}
                          onDoubleClick={handleDoubleClick}
                          onRename={handleRename}
                          onDelete={handleDelete}
                          onStatusChange={handleStatusChange}
                          onNewConversation={handleNewConversation}
                          onImport={handleImport}
                          importing={importing}
                        />
                      )}
                    </div>
                  )
                })}
              </Virtualizer>
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem onSelect={handleNewConversation}>
              <Plus className="h-4 w-4" />
              {t("newConversation")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem disabled={importing} onSelect={handleImport}>
              <Download className="h-4 w-4" />
              {importing ? t("importing") : t("importLocalSessions")}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      )}
      <AlertDialog
        open={completeReviewOpen}
        onOpenChange={(open) =>
          !completingReview && setCompleteReviewOpen(open)
        }
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("completeAllReviewTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("completeAllReviewDescription", {
                count: reviewConversationCount,
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={completingReview}>
              {tCommon("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={completingReview || reviewConversationCount === 0}
              onClick={handleCompleteAllReview}
            >
              {completingReview ? t("completing") : tCommon("confirm")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
