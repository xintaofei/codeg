"use client"

import { useCallback, useEffect, useMemo, useRef } from "react"
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
  onNearTop?: () => void
  preserveScrollOnPrependKey?: number | string | null
}

export function VirtualizedMessageThread<T>({
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
  onNearTop,
  preserveScrollOnPrependKey,
}: VirtualizedMessageThreadProps<T>) {
  const { scrollRef } = useStickToBottomContext()
  const virtualizerHandleRef = useRef<VirtualizerHandle>(null)
  const beforePrependRef = useRef<{
    key: number | string | null | undefined
    scrollHeight: number
    scrollTop: number
  } | null>(null)
  const nearTopLoadKeyRef = useRef<number | string | null | undefined>(null)

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

  useEffect(() => {
    const el = scrollRef.current
    if (!el || !onNearTop) return
    const handleScroll = () => {
      if (el.scrollTop > 240) return
      if (nearTopLoadKeyRef.current === preserveScrollOnPrependKey) return
      nearTopLoadKeyRef.current = preserveScrollOnPrependKey
      beforePrependRef.current = {
        key: preserveScrollOnPrependKey,
        scrollHeight: el.scrollHeight,
        scrollTop: el.scrollTop,
      }
      onNearTop()
    }
    el.addEventListener("scroll", handleScroll, { passive: true })
    return () => el.removeEventListener("scroll", handleScroll)
  }, [onNearTop, preserveScrollOnPrependKey, scrollRef])

  useEffect(() => {
    const snapshot = beforePrependRef.current
    const el = scrollRef.current
    if (!snapshot || !el) return
    if (snapshot.key === preserveScrollOnPrependKey) return
    beforePrependRef.current = null
    nearTopLoadKeyRef.current = null
    const heightDelta = el.scrollHeight - snapshot.scrollHeight
    if (heightDelta <= 0) return
    el.scrollTop = snapshot.scrollTop + heightDelta
  }, [preserveScrollOnPrependKey, scrollRef])

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
