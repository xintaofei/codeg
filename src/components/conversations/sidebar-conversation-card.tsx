"use client"

import { memo, useState, useCallback } from "react"
import { Pencil, Trash2, Circle, Plus } from "lucide-react"
import { useTranslations } from "next-intl"
import type { DbConversationSummary, ConversationStatus } from "@/lib/types"
import { STATUS_ORDER } from "@/lib/types"
import { cn } from "@/lib/utils"
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubContent,
  ContextMenuSeparator,
} from "@/components/ui/context-menu"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog"
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
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ConversationStatusDot } from "./conversation-status-dot"
import { AgentIcon } from "@/components/agent-icon"

interface SidebarConversationCardProps {
  conversation: DbConversationSummary
  isSelected: boolean
  isOpenInTab?: boolean
  timeLabel?: string
  onSelect: (id: number, agentType: string) => void
  onDoubleClick?: (id: number, agentType: string) => void
  onRename: (id: number, newTitle: string) => Promise<void>
  onDelete: (id: number, agentType: string) => Promise<void>
  onStatusChange: (id: number, status: ConversationStatus) => Promise<void>
  onNewConversation?: () => void
}

export const SidebarConversationCard = memo(function SidebarConversationCard({
  conversation,
  isSelected,
  isOpenInTab = false,
  timeLabel,
  onSelect,
  onDoubleClick,
  onRename,
  onDelete,
  onStatusChange,
  onNewConversation,
}: SidebarConversationCardProps) {
  const t = useTranslations("Folder.conversationCard")
  const tSidebar = useTranslations("Folder.sidebar")
  const tStatus = useTranslations("Folder.statusLabels")
  const [renameOpen, setRenameOpen] = useState(false)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [renameValue, setRenameValue] = useState("")

  const handleClick = useCallback(() => {
    onSelect(conversation.id, conversation.agent_type)
  }, [onSelect, conversation.id, conversation.agent_type])

  const handleDblClick = useCallback(() => {
    onDoubleClick?.(conversation.id, conversation.agent_type)
  }, [onDoubleClick, conversation.id, conversation.agent_type])

  const handleRenameOpen = useCallback(() => {
    setRenameValue(conversation.title || "")
    setRenameOpen(true)
  }, [conversation.title])

  const handleRenameConfirm = useCallback(async () => {
    const trimmed = renameValue.trim()
    if (trimmed && trimmed !== conversation.title) {
      await onRename(conversation.id, trimmed)
    }
    setRenameOpen(false)
  }, [renameValue, conversation.id, conversation.title, onRename])

  const handleDeleteConfirm = useCallback(async () => {
    await onDelete(conversation.id, conversation.agent_type)
    setDeleteOpen(false)
  }, [conversation.id, conversation.agent_type, onDelete])

  const status = conversation.status as ConversationStatus
  const isRunning = status === "in_progress"
  const isCancelled = status === "cancelled"

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            className="relative h-[2rem] bg-sidebar"
            data-conv-key={`${conversation.agent_type}:${conversation.id}`}
          >
            <button
              data-conversation-id={conversation.id}
              onClick={handleClick}
              onDoubleClick={handleDblClick}
              className={cn(
                "relative flex h-[1.9375rem] w-full items-center gap-[0.625rem] text-left outline-none",
                "rounded-full text-sidebar-foreground",
                "transition-colors duration-[120ms]",
                "pr-[0.5rem] pl-7",
                isSelected
                  ? "bg-sidebar-primary/8"
                  : "hover:bg-[color-mix(in_oklab,var(--sidebar-accent),var(--sidebar-foreground)_2%)]"
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "pointer-events-none absolute z-0 bg-sidebar-border"
                )}
                style={{
                  top: "-0.0625rem",
                  bottom: "-0.0625rem",
                  left: "var(--conv-rail-axis, 0.875rem)",
                  width: "0.125rem",
                  transform: "translateX(-50%)",
                }}
              />
              <div
                className="pointer-events-none absolute top-1/2 z-10 flex items-center justify-center"
                style={{
                  left: "var(--conv-rail-axis, 0.875rem)",
                  width: "0.875rem",
                  height: "0.875rem",
                  transform: "translate(-50%, -50%)",
                }}
                aria-hidden
              >
                <AgentIcon
                  agentType={conversation.agent_type}
                  className="h-[0.75rem] w-[0.75rem]"
                />
                <ConversationStatusDot
                  status={status}
                  size="sm"
                  className="absolute -right-0.5 -bottom-0.5 ring-2 ring-sidebar"
                />
              </div>

              <span
                className={cn(
                  "relative min-w-0 flex-1 truncate text-[0.875rem] font-normal",
                  isOpenInTab && "text-primary"
                )}
              >
                {conversation.title || t("untitledConversation")}
              </span>

              {isRunning ? (
                <span
                  className={cn(
                    "relative inline-flex shrink-0 items-center justify-center",
                    "h-[0.9375rem] rounded-[0.3125rem] px-[0.25rem]",
                    "text-[0.625rem] font-semibold leading-none tracking-[0.01875rem]",
                    "bg-amber-500/20 text-amber-600 dark:bg-amber-400/20 dark:text-amber-400"
                  )}
                >
                  {tSidebar("statusRunningBadge")}
                </span>
              ) : isCancelled ? (
                <span
                  className={cn(
                    "relative inline-flex shrink-0 items-center justify-center",
                    "h-[0.9375rem] rounded-[0.3125rem] px-[0.25rem]",
                    "text-[0.625rem] font-semibold leading-none tracking-[0.01875rem]",
                    "bg-destructive/20 text-destructive"
                  )}
                >
                  {tSidebar("statusCancelledBadge")}
                </span>
              ) : timeLabel ? (
                <span
                  className={cn(
                    "relative shrink-0 tabular-nums",
                    "text-[0.71875rem]",
                    isSelected
                      ? "font-medium text-muted-foreground"
                      : "font-normal text-muted-foreground/70"
                  )}
                >
                  {timeLabel}
                </span>
              ) : null}
            </button>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          {onNewConversation && (
            <>
              <ContextMenuItem onSelect={onNewConversation}>
                <Plus className="h-4 w-4" />
                {t("newConversation")}
              </ContextMenuItem>
              <ContextMenuSeparator />
            </>
          )}
          <ContextMenuItem onSelect={handleRenameOpen}>
            <Pencil className="h-4 w-4" />
            {t("rename")}
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuSub>
            <ContextMenuSubTrigger>
              <Circle className="h-4 w-4" />
              {t("status")}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {STATUS_ORDER.filter((s) => s !== conversation.status).map(
                (s) => (
                  <ContextMenuItem
                    key={s}
                    onSelect={() => onStatusChange(conversation.id, s)}
                  >
                    <ConversationStatusDot status={s} />
                    {tStatus(s)}
                  </ContextMenuItem>
                )
              )}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSeparator />
          <ContextMenuItem
            variant="destructive"
            onSelect={() => setDeleteOpen(true)}
          >
            <Trash2 className="h-4 w-4" />
            {t("delete")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>

      <Dialog open={renameOpen} onOpenChange={setRenameOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("renameConversation")}</DialogTitle>
          </DialogHeader>
          <Input
            value={renameValue}
            onChange={(e) => setRenameValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.nativeEvent.isComposing || e.key === "Process") return
              if (e.key === "Enter") handleRenameConfirm()
            }}
            autoFocus
          />
          <DialogFooter>
            <Button variant="outline" onClick={() => setRenameOpen(false)}>
              {t("cancel")}
            </Button>
            <Button onClick={handleRenameConfirm}>{t("save")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog open={deleteOpen} onOpenChange={setDeleteOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("deleteConversationTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("deleteConversationDescription", {
                title: conversation.title || t("untitledConversation"),
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDeleteConfirm}>
              {t("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
})
