"use client"

import { memo, useCallback, useMemo, useRef } from "react"
import type { CSSProperties, ReactNode, RefObject } from "react"
import { Virtualizer, type VirtualizerHandle } from "virtua"
import { useStickToBottomContext } from "use-stick-to-bottom"
import {
  MessageThreadContent,
  type MessageThreadContentProps,
} from "@/components/ai-elements/message-thread"
import { cn } from "@/lib/utils"
import {
  MessageScrollProvider,
  type MessageScrollContextValue,
} from "@/components/message/message-scroll-context"

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
  /**
   * Pixels of overscan around the viewport (virtua `bufferSize`).
   * Larger values reduce blank flashes during fast scroll on tall rows
   * at the cost of more off-screen reconciliation. @default 800
   */
  bufferSize?: number
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

function VirtualizedMessageThreadImpl<T>({
  items,
  getItemKey,
  renderItem,
  emptyState,
  itemSize,
  bufferSize = 800,
  gap = 16,
  padding = 16,
  className,
  contentClassName,
  contentProps,
}: VirtualizedMessageThreadProps<T>) {
  const { scrollRef } = useStickToBottomContext()
  const virtualizerHandleRef = useRef<VirtualizerHandle>(null)

  const scrollToIndex = useCallback<MessageScrollContextValue["scrollToIndex"]>(
    (index, opts) => {
      virtualizerHandleRef.current?.scrollToIndex(index, opts)
    },
    []
  )
  const scrollContextValue = useMemo<MessageScrollContextValue>(
    () => ({ scrollToIndex }),
    [scrollToIndex]
  )

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
    <MessageScrollProvider value={scrollContextValue}>
      <MessageThreadContent
        className={cn("mx-0 max-w-none p-0", contentClassName)}
        scrollClassName="scrollbar-thin overscroll-contain [overflow-anchor:none]"
        {...contentProps}
      >
        {items.length === 0 ? (
          (emptyState ?? null)
        ) : (
          <Virtualizer
            ref={virtualizerHandleRef}
            scrollRef={scrollRef as unknown as RefObject<HTMLElement | null>}
            itemSize={itemSize}
            bufferSize={bufferSize}
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
    </MessageScrollProvider>
  )
}

// Memoized so a cross-tab broadcast render of MessageListView with an
// unchanged `items` reference (see getTimelineTurns memoization) skips the
// per-row React element creation entirely. The streaming tab's `items`
// reference changes every flush, so it re-renders as before. `getItemKey` /
// `renderItem` are stabilized by the caller; default shallow prop comparison
// is sufficient. The `as` cast preserves the generic call signature that
// `memo` would otherwise erase.
export const VirtualizedMessageThread = memo(
  VirtualizedMessageThreadImpl
) as typeof VirtualizedMessageThreadImpl
