"use client"

import { memo, useMemo, useState, type CSSProperties } from "react"
import { useTranslations } from "next-intl"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import type {
  SessionLocatorItem,
  SessionLocatorPreview,
  SessionLocatorTarget,
} from "@/lib/session-locator"
import { cn } from "@/lib/utils"
import {
  Bot,
  ChevronRightIcon,
  ChevronUpIcon,
  MapIcon,
  User,
} from "lucide-react"

interface MessageNavigatorOverlayProps {
  items: SessionLocatorItem[]
  locatorKey?: string | null
  visible?: boolean
  defaultExpanded?: boolean
  className?: string
  panelWidthPx?: number
  panelMaxHeightPx?: number
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
    default:
      return t("emptyPreview")
  }
}

const NavigatorRow = memo(function NavigatorRow({
  role,
  preview,
  ariaLabel,
  onClick,
  disabled = false,
}: {
  role: "user" | "assistant"
  preview: string
  ariaLabel?: string
  onClick?: () => void
  disabled?: boolean
}) {
  const rowClassName = cn(
    "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors",
    disabled
      ? "cursor-default text-muted-foreground"
      : "cursor-pointer hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
  )

  if (disabled) {
    return (
      <div className={rowClassName}>
        <Badge
          variant="outline"
          className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center p-0"
        >
          {role === "user" ? (
            <User className="h-3 w-3" />
          ) : (
            <Bot className="h-3 w-3" />
          )}
        </Badge>
        <p className="min-w-0 flex-1 line-clamp-2 break-words text-sm leading-5 text-muted-foreground">
          {preview}
        </p>
      </div>
    )
  }

  return (
    <button type="button" className={rowClassName} onClick={onClick}>
      {ariaLabel ? <span className="sr-only">{ariaLabel} </span> : null}
      <Badge
        variant="outline"
        className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center p-0"
      >
        {role === "user" ? (
          <User className="h-3 w-3" />
        ) : (
          <Bot className="h-3 w-3" />
        )}
      </Badge>
      <p className="min-w-0 flex-1 line-clamp-2 break-words text-sm leading-5 text-foreground">
        {preview}
      </p>
    </button>
  )
})

export const MessageNavigatorOverlay = memo(function MessageNavigatorOverlay({
  items,
  locatorKey,
  visible = true,
  defaultExpanded = true,
  className,
  panelWidthPx,
  panelMaxHeightPx,
  onJumpToTarget,
}: MessageNavigatorOverlayProps) {
  const t = useTranslations("Folder.chat.messageNavigator")
  const hasItems = visible && items.length > 0
  const fallbackKey = useMemo(() => {
    if (items.length === 0) return null
    return items
      .map((item) => `${item.id}:${item.status}:${item.user.turnId}`)
      .join("|")
  }, [items])
  const currentLocatorKey = locatorKey ?? fallbackKey
  const currentLocatorStateKey =
    currentLocatorKey ?? "__message_navigator__default__"
  const [collapsedByLocatorKey, setCollapsedByLocatorKey] = useState<
    Record<string, boolean>
  >({})
  const hasStoredCollapsedState = Object.prototype.hasOwnProperty.call(
    collapsedByLocatorKey,
    currentLocatorStateKey
  )

  const panelStyle: CSSProperties | undefined = panelWidthPx
    ? {
        width: `${panelWidthPx}px`,
        maxWidth: "100%",
        ...(panelMaxHeightPx ? { maxHeight: `${panelMaxHeightPx}px` } : null),
      }
    : panelMaxHeightPx
      ? { maxHeight: `${panelMaxHeightPx}px` }
      : undefined
  const isExpanded = hasStoredCollapsedState
    ? !collapsedByLocatorKey[currentLocatorStateKey]
    : defaultExpanded

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
          className="h-8 w-44 justify-between gap-2 cursor-pointer shadow-md bg-secondary/70 hover:bg-secondary"
          onClick={() =>
            setCollapsedByLocatorKey((prev) => ({
              ...prev,
              [currentLocatorStateKey]: false,
            }))
          }
        >
          <MapIcon className="h-4 w-4" />
          <span className="min-w-0 flex-1 truncate text-center">
            {t("collapsedSummary", { count: items.length })}
          </span>
          <ChevronUpIcon className="h-4 w-4" />
        </Button>
      </div>
    )
  }

  return (
    <div
      className={cn(
        "pointer-events-auto flex min-h-0 max-h-full min-w-0",
        className
      )}
      style={panelStyle}
      data-locator-key={currentLocatorKey ?? undefined}
    >
      <div className="flex min-h-0 max-h-full w-full flex-col rounded-xl border bg-card/60 shadow-lg backdrop-blur transition-colors hover:bg-card/95 supports-[backdrop-filter]:bg-card/50 supports-[backdrop-filter]:hover:bg-card/85">
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
            <ChevronRightIcon className="h-4 w-4" />
          </Button>
        </div>

        <div
          className="min-h-0 flex-1 space-y-2 overflow-y-auto p-3"
          role="navigation"
          aria-label={t("title")}
        >
          {items.map((item) => {
            const userPreview = getPreviewText(item.user.preview, t)
            const assistantTarget = item.assistant

            return (
              <div
                key={item.id}
                className="rounded-lg border bg-transparent px-1.5 py-1.5"
              >
                <NavigatorRow
                  role="user"
                  preview={userPreview}
                  ariaLabel={t("jumpToUserAria")}
                  onClick={() => onJumpToTarget(item.user)}
                />
                {assistantTarget ? (
                  <NavigatorRow
                    role="assistant"
                    preview={getPreviewText(assistantTarget.preview, t)}
                    ariaLabel={t("jumpToAssistantAria")}
                    onClick={() => onJumpToTarget(assistantTarget)}
                  />
                ) : null}
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
})
