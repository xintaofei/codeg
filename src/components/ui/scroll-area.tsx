"use client"

import { useMemo } from "react"
import {
  OverlayScrollbarsComponent,
  type OverlayScrollbarsComponentRef,
} from "overlayscrollbars-react"
import type { OverlayScrollbarsComponentProps } from "overlayscrollbars-react"

type ScrollAreaProps = {
  children: React.ReactNode
  className?: string
  x?: "scroll" | "hidden"
  y?: "scroll" | "hidden"
  onScroll?: (event: Event) => void
  /**
   * Receives the real scrollable viewport element once OverlayScrollbars has
   * initialized (and `null` on destroy). Needed when an external library — e.g.
   * a `virtua` Virtualizer — must bind to the actual scroll container rather
   * than the host. Because the component initializes with `defer`, reading the
   * viewport synchronously after mount is racy; this fires at the right time.
   */
  onViewportRef?: (element: HTMLElement | null) => void
  ref?: React.Ref<OverlayScrollbarsComponentRef>
}

const BASE_OPTIONS: OverlayScrollbarsComponentProps["options"] = {
  scrollbars: {
    theme: "os-theme-codeg",
    autoHide: "leave",
    clickScroll: true,
  },
}

export function ScrollArea({
  children,
  className,
  x = "hidden",
  y = "scroll",
  onScroll,
  onViewportRef,
  ref,
}: ScrollAreaProps) {
  const options = useMemo<OverlayScrollbarsComponentProps["options"]>(
    () => ({
      ...BASE_OPTIONS,
      overflow: { x, y },
    }),
    [x, y]
  )

  const events = useMemo<OverlayScrollbarsComponentProps["events"]>(
    () => ({
      ...(onScroll ? { scroll: (_instance, event) => onScroll(event) } : {}),
      ...(onViewportRef
        ? {
            initialized: (instance) =>
              onViewportRef(instance.elements().viewport),
            destroyed: () => onViewportRef(null),
          }
        : {}),
    }),
    [onScroll, onViewportRef]
  )

  return (
    <OverlayScrollbarsComponent
      ref={ref}
      className={className}
      options={options}
      events={events}
      defer
    >
      {children}
    </OverlayScrollbarsComponent>
  )
}
