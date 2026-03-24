"use client"

import { useMemo } from "react"
import type { CSSProperties, ReactNode, RefObject } from "react"
import { Virtualizer } from "virtua"
import { useStickToBottomContext } from "use-stick-to-bottom"
import {
  MessageThreadContent,
  type MessageThreadContentProps,
} from "@/components/ai-elements/message-thread"
import { cn } from "@/lib/utils"

interface VirtualizedMessageThreadProps<T> {
  /** Data to virtualise — each entry becomes one virtual row. */
  items: T[]
  /** Stable key for a given item (used as React key). */
  getItemKey: (item: T, index: number) => string
  /** Render the content of one row. */
  renderItem: (item: T, index: number) => ReactNode
  /** Shown when `items` is empty. */
  emptyState?: ReactNode
  /**
   * Hint for the initial height (px) of an unmeasured item.
   * Virtua auto-measures every item once mounted, so this only
   * affects the very first paint — omit it if you don't care.
   */
  itemSize?: number
  /** Vertical gap between items in px. @default 16 */
  gap?: number
  /** Vertical padding before the first / after the last item. @default 16 */
  padding?: number
  /** Extra className on every item's inner wrapper (the `max-w-3xl` div). */
  className?: string
  /** Extra className on the MessageThreadContent shell. */
  contentClassName?: string
  /** Extra props forwarded to MessageThreadContent. */
  contentProps?: Omit<MessageThreadContentProps, "children" | "className">
}

export function VirtualizedMessageThread<T>({
  items,
  getItemKey,
  renderItem,
  emptyState,
  itemSize,
  gap = 16,
  padding = 16,
  className,
  contentClassName,
  contentProps,
}: VirtualizedMessageThreadProps<T>) {
  const { scrollRef } = useStickToBottomContext()

  // Pre-compute the three possible padding styles so every render reuses
  // the same object references (avoids allocating per-item on each frame).
  const styles = useMemo(() => {
    const halfGap = gap / 2
    return {
      only: { paddingTop: padding, paddingBottom: padding } as CSSProperties,
      first: { paddingTop: padding, paddingBottom: halfGap } as CSSProperties,
      middle: { paddingTop: halfGap, paddingBottom: halfGap } as CSSProperties,
      last: { paddingTop: halfGap, paddingBottom: padding } as CSSProperties,
    }
  }, [gap, padding])

  const itemStyle = (index: number, total: number) => {
    if (total === 1) return styles.only
    if (index === 0) return styles.first
    if (index === total - 1) return styles.last
    return styles.middle
  }

  return (
    <MessageThreadContent
      className={cn("mx-0 max-w-none p-0", contentClassName)}
      scrollClassName="[overflow-anchor:none]"
      {...contentProps}
    >
      {items.length === 0 ? (
        (emptyState ?? null)
      ) : (
        <Virtualizer
          scrollRef={scrollRef as unknown as RefObject<HTMLElement | null>}
          itemSize={itemSize}
        >
          {items.map((item, index) => (
            <div
              key={getItemKey(item, index)}
              style={itemStyle(index, items.length)}
            >
              <div className={cn("mx-auto max-w-3xl px-4", className)}>
                {renderItem(item, index)}
              </div>
            </div>
          ))}
        </Virtualizer>
      )}
    </MessageThreadContent>
  )
}
