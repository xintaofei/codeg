"use client"

import { useCallback, useImperativeHandle } from "react"
import type { CSSProperties, ReactNode, Ref } from "react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useStickToBottomContext } from "use-stick-to-bottom"
import {
  MessageThreadContent,
  type MessageThreadContentProps,
} from "@/components/ai-elements/message-thread"
import { cn } from "@/lib/utils"

export interface VirtualizedMessageThreadHandle {
  scrollToIndex: (
    index: number,
    options?: {
      align?: "start" | "center" | "end" | "auto"
      behavior?: "auto" | "smooth"
    }
  ) => void
  getScrollElement: () => HTMLElement | null
}

interface VirtualizedMessageThreadProps<T> {
  items: T[]
  getItemKey: (item: T, index: number) => string
  renderItem: (item: T, index: number) => ReactNode
  emptyState?: ReactNode
  estimateSize?: number
  overscan?: number
  className?: string
  rowContainerStyle?: CSSProperties
  contentClassName?: string
  contentProps?: Omit<MessageThreadContentProps, "children" | "className">
  ref?: Ref<VirtualizedMessageThreadHandle>
}

export function VirtualizedMessageThread<T>({
  items,
  getItemKey,
  renderItem,
  emptyState,
  estimateSize = 160,
  overscan = 8,
  className,
  rowContainerStyle,
  contentClassName,
  contentProps,
  ref,
}: VirtualizedMessageThreadProps<T>) {
  const { scrollRef } = useStickToBottomContext()

  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => estimateSize,
    overscan,
    useAnimationFrameWithResizeObserver: true,
    isScrollingResetDelay: 100,
    paddingStart: 16,
    paddingEnd: 16,
    gap: 32,
    getItemKey: (index) => {
      const item = items[index]
      return item ? getItemKey(item, index) : index
    },
  })

  useImperativeHandle(
    ref,
    () => ({
      scrollToIndex(index, options) {
        virtualizer.scrollToIndex(index, options)
      },
      getScrollElement() {
        return scrollRef.current
      },
    }),
    [scrollRef, virtualizer]
  )

  const renderVirtualRow = useCallback(
    (virtualItem: ReturnType<typeof virtualizer.getVirtualItems>[number]) => {
      const item = items[virtualItem.index]
      if (!item) return null

      return (
        <div
          key={virtualItem.key}
          ref={virtualizer.measureElement}
          data-index={virtualItem.index}
          className="absolute left-0 top-0 w-full"
          style={{
            transform: `translate3d(0, ${virtualItem.start}px, 0)`,
            willChange: "transform",
          }}
        >
          <div
            className={cn("mx-auto max-w-3xl px-4", className)}
            style={rowContainerStyle}
          >
            {renderItem(item, virtualItem.index)}
          </div>
        </div>
      )
    },
    [className, items, renderItem, rowContainerStyle, virtualizer]
  )

  return (
    <MessageThreadContent
      className={cn("mx-0 max-w-none p-0", contentClassName)}
      {...contentProps}
    >
      {items.length === 0 ? (
        (emptyState ?? null)
      ) : (
        <div
          className="relative w-full"
          style={{
            height: `${virtualizer.getTotalSize()}px`,
          }}
        >
          {virtualizer.getVirtualItems().map(renderVirtualRow)}
        </div>
      )}
    </MessageThreadContent>
  )
}
