"use client"

import type { ComponentProps } from "react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { ArrowDownIcon, DownloadIcon } from "lucide-react"
import { useTranslations } from "next-intl"
import { useCallback, useEffect } from "react"
import { StickToBottom, useStickToBottomContext } from "use-stick-to-bottom"

/**
 * Keeps the thread pinned when the SCROLL VIEWPORT changes height. Renders
 * nothing; mounted by `MessageThread` itself because it consumes the
 * stick-to-bottom context and so has to live inside the provider.
 *
 * `use-stick-to-bottom` observes only the CONTENT element — a resize there is
 * new transcript, and it re-sticks. Nothing observes the scroll element, yet
 * the siblings BELOW the thread resize it constantly: the live-turn stats bar
 * mounting when a turn starts, an ask-question / plan-approval card or a
 * failure strip opening in the composer dock, the composer growing with the
 * draft — and the terminal panel, which the workspace shell force-collapses for
 * the duration of a full-page route (tasks / automations / …) and restores on
 * the way back (see `app/workspace/layout.tsx`). Both directions are broken
 * without this:
 *
 * - The viewport SHRINKS: `scrollHeight` and `scrollTop` are both untouched, so
 *   the maximum scroll offset grows out from under the thread. No scroll event
 *   and no content resize fire, so the library never learns, and the tail of
 *   the turn stays below the fold. Inside the library's 70px "near bottom"
 *   slack the scroll-to-bottom button stays hidden too — which is what makes it
 *   read as a thread that is already at the bottom and still cut off.
 * - The viewport GROWS: the maximum scroll offset drops below the current
 *   position and the browser clamps it, firing a real scroll event with a LOWER
 *   `scrollTop`. The library reads that as the user scrolling up, escapes the
 *   lock, and stops following the stream for the rest of the turn.
 *
 * Only a thread that was already at the bottom is re-pinned, so a user who
 * scrolled back into history is never yanked forward.
 */
const StickThroughViewportResize = () => {
  const { scrollRef, scrollToBottom, state } = useStickToBottomContext()

  useEffect(() => {
    const viewport = scrollRef.current
    if (!viewport) return

    let previousHeight: number | undefined
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return
      const { height } = entry.contentRect
      // First delivery reports the current size, not a change — same guard the
      // library uses on its own content observer.
      const difference = height - (previousHeight ?? height)
      previousHeight = height
      if (!difference || !state.isAtBottom) return

      // The library's shield around its own content resizes, reused for ours:
      // it makes `handleScroll` ignore the scroll events this resize provokes
      // (the clamp above, and the re-pin below) rather than reading them as the
      // user leaving the bottom. Released a frame later, and only if no later
      // resize has claimed it since — again mirroring the library.
      state.resizeDifference = difference
      requestAnimationFrame(() => {
        setTimeout(() => {
          if (state.resizeDifference === difference) {
            state.resizeDifference = 0
          }
        }, 1)
      })

      // `preserveScrollPosition` suppresses only the redundant "we're at the
      // bottom" state write — the guard above already established that. A
      // growth resize lands at the target on its own, so this is then a no-op.
      scrollToBottom({ animation: "instant", preserveScrollPosition: true })
    })

    observer.observe(viewport)
    return () => {
      observer.disconnect()
    }
  }, [scrollRef, scrollToBottom, state])

  return null
}

export type MessageThreadProps = ComponentProps<typeof StickToBottom>

export const MessageThread = ({
  className,
  children,
  ...props
}: MessageThreadProps) => (
  <StickToBottom
    className={cn("relative flex-1 overflow-y-hidden", className)}
    initial="instant"
    resize="smooth"
    role="log"
    {...props}
  >
    {(context) => (
      <>
        <StickThroughViewportResize />
        {typeof children === "function" ? children(context) : children}
      </>
    )}
  </StickToBottom>
)

export type MessageThreadContentProps = ComponentProps<
  typeof StickToBottom.Content
>

export const MessageThreadContent = ({
  className,
  ...props
}: MessageThreadContentProps) => (
  <StickToBottom.Content
    className={cn("flex flex-col gap-8 p-4", className)}
    {...props}
  />
)

export type MessageThreadEmptyStateProps = ComponentProps<"div"> & {
  title?: string
  description?: string
  icon?: React.ReactNode
}

export const MessageThreadEmptyState = ({
  className,
  title,
  description,
  icon,
  children,
  ...props
}: MessageThreadEmptyStateProps) => {
  const t = useTranslations("Folder.chat.messageThread")
  return (
    <div
      className={cn(
        "flex size-full flex-col items-center justify-center gap-3 p-8 text-center",
        className
      )}
      {...props}
    >
      {children ?? (
        <>
          {icon && <div className="text-muted-foreground">{icon}</div>}
          <div className="space-y-1">
            <h3 className="font-medium text-sm">{title ?? t("emptyTitle")}</h3>
            {(description ?? t("emptyDescription")) && (
              <p className="text-muted-foreground text-sm">
                {description ?? t("emptyDescription")}
              </p>
            )}
          </div>
        </>
      )}
    </div>
  )
}

export type MessageThreadScrollButtonProps = ComponentProps<typeof Button>

export const MessageThreadScrollButton = ({
  className,
  ...props
}: MessageThreadScrollButtonProps) => {
  const { isAtBottom, scrollToBottom } = useStickToBottomContext()

  const handleScrollToBottom = useCallback(() => {
    scrollToBottom()
  }, [scrollToBottom])

  return (
    !isAtBottom && (
      <Button
        className={cn(
          "absolute bottom-4 left-[50%] translate-x-[-50%] rounded-full bg-background/90 hover:bg-muted/90",
          className
        )}
        onClick={handleScrollToBottom}
        size="icon"
        type="button"
        variant="outline"
        {...props}
      >
        <ArrowDownIcon className="size-4" />
      </Button>
    )
  )
}

export interface ThreadMessage {
  role: "user" | "assistant" | "system" | "data" | "tool"
  content: string
}

export type MessageThreadDownloadProps = Omit<
  ComponentProps<typeof Button>,
  "onClick"
> & {
  messages: ThreadMessage[]
  filename?: string
  formatMessage?: (message: ThreadMessage, index: number) => string
}

const defaultFormatMessage = (message: ThreadMessage): string => {
  const roleLabel = message.role.charAt(0).toUpperCase() + message.role.slice(1)
  return `**${roleLabel}:** ${message.content}`
}

export const messagesToMarkdown = (
  messages: ThreadMessage[],
  formatMessage: (
    message: ThreadMessage,
    index: number
  ) => string = defaultFormatMessage
): string => messages.map((msg, i) => formatMessage(msg, i)).join("\n\n")

export const MessageThreadDownload = ({
  messages,
  filename = "conversation.md",
  formatMessage = defaultFormatMessage,
  className,
  children,
  ...props
}: MessageThreadDownloadProps) => {
  const handleDownload = useCallback(() => {
    const markdown = messagesToMarkdown(messages, formatMessage)
    const blob = new Blob([markdown], { type: "text/markdown" })
    const url = URL.createObjectURL(blob)
    const link = document.createElement("a")
    link.href = url
    link.download = filename
    document.body.append(link)
    link.click()
    link.remove()
    URL.revokeObjectURL(url)
  }, [messages, filename, formatMessage])

  return (
    <Button
      className={cn(
        "absolute top-4 right-4 rounded-full dark:bg-background dark:hover:bg-muted",
        className
      )}
      onClick={handleDownload}
      size="icon"
      type="button"
      variant="outline"
      {...props}
    >
      {children ?? <DownloadIcon className="size-4" />}
    </Button>
  )
}
