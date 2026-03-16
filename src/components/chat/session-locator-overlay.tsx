"use client"

import { memo, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import type {
  SessionLocatorItem,
  SessionLocatorPreview,
  SessionLocatorTarget,
} from "@/lib/session-locator"
import { cn } from "@/lib/utils"
import { ChevronDownIcon, ChevronUpIcon, MapIcon } from "lucide-react"

interface SessionLocatorOverlayProps {
  items: SessionLocatorItem[]
  locatorKey?: string | null
  visible?: boolean
  defaultExpanded?: boolean
  className?: string
  onJumpToTarget: (target: SessionLocatorTarget) => void
}

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

const LocatorRow = memo(function LocatorRow({
  label,
  preview,
  ariaLabel,
  onClick,
  disabled = false,
}: {
  label: string
  preview: string
  ariaLabel?: string
  onClick?: () => void
  disabled?: boolean
}) {
  const rowClassName = cn(
    "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors",
    disabled
      ? "cursor-default text-muted-foreground"
      : "cursor-pointer hover:bg-muted/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
  )

  if (disabled) {
    return (
      <div className={rowClassName}>
        <Badge
          variant="outline"
          className="mt-0.5 h-5 shrink-0 text-[10px] uppercase"
        >
          {label}
        </Badge>
        <p className="min-w-0 flex-1 line-clamp-2 break-words text-sm leading-5 text-muted-foreground">
          {preview}
        </p>
      </div>
    )
  }

  return (
    <button
      type="button"
      className={rowClassName}
      onClick={onClick}
    >
      {ariaLabel ? <span className="sr-only">{ariaLabel} </span> : null}
      <Badge
        variant="outline"
        className="mt-0.5 h-5 shrink-0 text-[10px] uppercase"
      >
        {label}
      </Badge>
      <p className="min-w-0 flex-1 line-clamp-2 break-words text-sm leading-5 text-foreground">
        {preview}
      </p>
    </button>
  )
})

export const SessionLocatorOverlay = memo(function SessionLocatorOverlay({
  items,
  locatorKey,
  visible = true,
  defaultExpanded = false,
  className,
  onJumpToTarget,
}: SessionLocatorOverlayProps) {
  const t = useTranslations("Folder.chat.sessionLocatorOverlay")
  const hasItems = visible && items.length > 0
  const fallbackKey = useMemo(() => {
    if (items.length === 0) return null
    return items
      .map((item) => `${item.id}:${item.status}:${item.user.turnId}`)
      .join("|")
  }, [items])
  const currentLocatorKey = locatorKey ?? fallbackKey
  const currentLocatorStateKey =
    currentLocatorKey ?? "__session_locator__default__"
  const resolvedDefaultExpanded = defaultExpanded
  const [collapsedByLocatorKey, setCollapsedByLocatorKey] = useState<
    Record<string, boolean>
  >({})
  const isExpanded = !(
    collapsedByLocatorKey[currentLocatorStateKey] ?? !resolvedDefaultExpanded
  )

  if (!hasItems) {
    return null
  }

  if (!isExpanded) {
    return (
      <div className={cn("pointer-events-auto flex", className)}>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          className="cursor-pointer shadow-md bg-secondary/70 hover:bg-secondary"
          onClick={() =>
            setCollapsedByLocatorKey((prev) => ({
              ...prev,
              [currentLocatorStateKey]: false,
            }))
          }
        >
          <MapIcon className="h-4 w-4" />
          {t("collapsedSummary", { count: items.length })}
          <ChevronUpIcon className="h-4 w-4" />
        </Button>
      </div>
    )
  }

  return (
    <div
      className={cn("pointer-events-auto flex w-full sm:w-72", className)}
      data-locator-key={currentLocatorKey ?? undefined}
    >
      <div className="w-full max-w-full rounded-xl border bg-card/60 shadow-lg backdrop-blur transition-colors hover:bg-card/95 supports-[backdrop-filter]:bg-card/50 supports-[backdrop-filter]:hover:bg-card/85">
        <div className="flex items-center justify-between border-b px-3 py-2">
          <div className="flex min-w-0 items-center gap-2">
            <MapIcon className="h-4 w-4 text-muted-foreground" />
            <span className="truncate text-sm font-medium">{t("title")}</span>
            <Badge variant="secondary" className="h-5">
              {items.length}
            </Badge>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label={t("collapseAria")}
            onClick={() =>
              setCollapsedByLocatorKey((prev) => ({
                ...prev,
                [currentLocatorStateKey]: true,
              }))
            }
          >
            <ChevronDownIcon className="h-4 w-4" />
          </Button>
        </div>

        <div
          className="max-h-96 space-y-2 overflow-y-auto p-3"
          role="navigation"
          aria-label={t("title")}
        >
          {items.map((item) => {
            const userPreview = getPreviewText(item.user.preview, t)
            const assistantTarget = item.assistant
            const assistantPreview = assistantTarget
              ? getPreviewText(assistantTarget.preview, t)
              : t("pendingReply")

            return (
              <div
                key={item.id}
                className="rounded-lg border bg-transparent px-1.5 py-1.5"
              >
                <LocatorRow
                  label={t("userLabel")}
                  preview={userPreview}
                  ariaLabel={t("jumpToUserAria")}
                  onClick={() => onJumpToTarget(item.user)}
                />
                <LocatorRow
                  label={t("assistantLabel")}
                  preview={assistantPreview}
                  ariaLabel={t("jumpToAssistantAria")}
                  onClick={
                    assistantTarget
                      ? () => onJumpToTarget(assistantTarget)
                      : undefined
                  }
                  disabled={!assistantTarget}
                />
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
})
