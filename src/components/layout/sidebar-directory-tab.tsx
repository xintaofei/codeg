"use client"

import { memo, useMemo } from "react"
import { useTranslations } from "next-intl"
import { useFolderContext } from "@/contexts/folder-context"
import { useSessionLocatorContext } from "@/contexts/session-locator-context"
import { useTabContext } from "@/contexts/tab-context"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import { useSessionLocatorItems } from "@/hooks/use-session-locator-items"
import type {
  SessionLocatorPreview,
  SessionLocatorTarget,
} from "@/lib/session-locator"
import { cn } from "@/lib/utils"

function getPreviewText(
  preview: SessionLocatorPreview,
  t: ReturnType<typeof useTranslations>
): string {
  switch (preview.kind) {
    case "text":
      return preview.text
    case "tool_only":
      return t("toolOnly")
    case "attachment_only":
      return t("attachmentOnly")
    case "pending_reply":
      return t("pendingReply")
    default:
      return t("emptyPreview")
  }
}

const DirectoryRow = memo(function DirectoryRow({
  label,
  preview,
  onClick,
  disabled = false,
  active = false,
}: {
  label: string
  preview: string
  onClick?: () => void
  disabled?: boolean
  active?: boolean
}) {
  const rowClassName = cn(
    "w-full rounded-md px-3 py-2 text-left transition-colors",
    disabled
      ? "cursor-default text-muted-foreground"
      : active
        ? "cursor-pointer bg-sidebar-accent text-sidebar-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
        : "cursor-pointer hover:bg-sidebar-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
  )

  if (disabled) {
    return (
      <div className={rowClassName}>
        <div className="flex min-w-0 items-center gap-2">
          <span className="text-[10px] font-semibold uppercase tracking-[0.08em] text-muted-foreground/80">
            {label}
          </span>
        </div>
        <p className="mt-1 line-clamp-2 break-words text-[13px] leading-5 text-muted-foreground">
          {preview}
        </p>
      </div>
    )
  }

  return (
    <button type="button" className={rowClassName} onClick={onClick}>
      <div className="flex min-w-0 items-center gap-2">
        <span
          className={cn(
            "text-[10px] font-semibold uppercase tracking-[0.08em]",
            active
              ? "text-sidebar-accent-foreground/80"
              : "text-muted-foreground/80"
          )}
        >
          {label}
        </span>
      </div>
      <p
        className={cn(
          "mt-1 line-clamp-2 break-words text-[13px] leading-5",
          active ? "font-medium text-sidebar-accent-foreground" : "text-foreground"
        )}
      >
        {preview}
      </p>
    </button>
  )
})

function isSameTarget(
  left: SessionLocatorTarget | null,
  right: SessionLocatorTarget | null
) {
  if (!left || !right) return false
  return (
    left.role === right.role &&
    left.turnId === right.turnId &&
    left.partIndex === right.partIndex
  )
}

export const SidebarDirectoryTab = memo(function SidebarDirectoryTab() {
  const t = useTranslations("Folder.sidebar")
  const locatorT = useTranslations("Folder.chat.sessionLocatorOverlay")
  const { selectedConversation } = useFolderContext()
  const { tabs, activeTabId } = useTabContext()
  const { getActiveTarget, jumpToTarget } = useSessionLocatorContext()

  const activeConversationId = useMemo(() => {
    const activeTab =
      tabs.find((tab) => tab.id === activeTabId && tab.kind === "conversation") ??
      null

    return (
      activeTab?.runtimeConversationId ??
      activeTab?.conversationId ??
      selectedConversation?.id ??
      null
    )
  }, [activeTabId, selectedConversation?.id, tabs])

  const { loading } = useConversationDetail(activeConversationId ?? -1)
  const items = useSessionLocatorItems(activeConversationId)
  const activeTarget =
    activeConversationId != null
      ? getActiveTarget(activeConversationId)
      : null

  if (activeConversationId == null) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("directory.noConversation")}
      </div>
    )
  }

  if (loading && items.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("directory.loading")}
      </div>
    )
  }

  if (items.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("directory.empty")}
      </div>
    )
  }

  return (
    <div className="h-full min-h-0 overflow-y-auto px-2 py-2">
      {items.map((item, index) => {
        const userPreview = getPreviewText(item.user.preview, locatorT)
        const assistantTarget = item.assistant
        const assistantPreview = assistantTarget
          ? getPreviewText(assistantTarget.preview, locatorT)
          : locatorT("pendingReply")
        const isUserActive = isSameTarget(activeTarget, item.user)
        const isAssistantActive = isSameTarget(activeTarget, assistantTarget)

        return (
          <div
            key={item.id}
            className={cn(
              "py-1.5",
              index > 0 && "border-t border-border/60"
            )}
          >
            <DirectoryRow
              label={locatorT("userLabel")}
              preview={userPreview}
              onClick={() => jumpToTarget(activeConversationId, item.user)}
              active={isUserActive}
            />
            <DirectoryRow
              label={locatorT("assistantLabel")}
              preview={assistantPreview}
              onClick={
                assistantTarget
                  ? () => jumpToTarget(activeConversationId, assistantTarget)
                  : undefined
              }
              disabled={!assistantTarget}
              active={isAssistantActive}
            />
          </div>
        )
      })}
    </div>
  )
})
