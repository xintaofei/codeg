"use client"

import { memo, useCallback, useEffect, useMemo, useRef } from "react"
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
  /**
   * Publishes the virtualizer scroll handle to an ancestor so siblings that
   * live outside the `MessageScrollProvider` subtree (e.g. the conversation
   * message navigator) can drive `scrollToIndex`.
   */
  scrollApiRef?: RefObject<MessageScrollContextValue | null>
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
  scrollApiRef,
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

  // Mirror the (stable) scroll handle into the caller-owned ref so a sibling
  // rendered outside this provider can call it. Runs once since the value is
  // referentially stable.
  useEffect(() => {
    if (!scrollApiRef) return
    scrollApiRef.current = scrollContextValue
    return () => {
      scrollApiRef.current = null
    }
  }, [scrollApiRef, scrollContextValue])

  // Make the scroll viewport focusable so the browser's native keyboard
  // scrolling (Arrow keys, PageUp/PageDown, Home/End, Space) works — matching
  // the sidebar conversation list, whose card <button>s are focusable and let
  // the browser scroll their scrollable ancestor. A left-click on
  // non-interactive transcript content focuses the viewport so the keys engage,
  // without stealing focus from interactive controls (links, buttons, inputs)
  // or breaking text selection (focus() doesn't clear a selection).
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    el.tabIndex = 0
    const clearPointerFocus = () => {
      el.removeAttribute("data-focus-origin")
    }
    const onPointerDown = (e: PointerEvent) => {
      // Ignore right-click and macOS ctrl-click (both open the context menu).
      if (e.button !== 0 || e.ctrlKey) return
      const target = e.target as HTMLElement | null
      // Don't steal focus from interactive/editable elements — they manage
      // their own focus (some do it in pointerdown). We deliberately do NOT
      // match a bare `[tabindex]` here: the viewport itself has tabIndex=0, so
      // an ancestor match would suppress focusing on every transcript click.
      if (
        target?.closest(
          'a[href],button,input,textarea,select,summary,[contenteditable]:not([contenteditable="false"]),[role="button"],[role="link"],[role="checkbox"],[role="switch"],[role="radio"],[role="tab"],[role="textbox"],[role="menuitem"],[role="option"],[role="combobox"],[role="slider"]'
        )
      )
        return
      el.setAttribute("data-focus-origin", "pointer")
      el.focus({ preventScroll: true })
    }
    el.addEventListener("pointerdown", onPointerDown)
    el.addEventListener("blur", clearPointerFocus)
    // Once the user drives the viewport with the keyboard (Arrow/Page/Home/End
    // to scroll), drop the pointer-origin marker so the focus ring reappears —
    // keeping the keyboard focus indicator visible per WCAG 2.4.7. The ring is
    // only suppressed for the mouse click that focused the viewport, not for
    // subsequent keyboard use.
    el.addEventListener("keydown", clearPointerFocus)
    return () => {
      el.removeEventListener("pointerdown", onPointerDown)
      el.removeEventListener("blur", clearPointerFocus)
      el.removeEventListener("keydown", clearPointerFocus)
      clearPointerFocus()
    }
  }, [scrollRef])

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
        scrollClassName="scrollbar-thin overscroll-contain [overflow-anchor:none] outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset data-[focus-origin=pointer]:focus-visible:ring-0"
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
