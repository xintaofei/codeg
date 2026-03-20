"use client"

import { useCallback, useEffect, useRef, useState, type RefObject } from "react"
import type { SessionLocatorTarget } from "@/lib/session-locator"
import type { VirtualizedMessageThreadHandle } from "./virtualized-message-thread"

export interface HighlightedMessageTarget {
  turnId: string
  partIndex: number | null
  token: number
}

interface UseMessageHighlightOptions {
  rootRef: RefObject<HTMLElement | null>
  threadRef: RefObject<VirtualizedMessageThreadHandle | null>
  stopAutoStick?: () => void
}

const HIGHLIGHT_ANIMATION_DURATION_MS = 1800
const HIGHLIGHT_STATE_RESET_DELAY_MS = HIGHLIGHT_ANIMATION_DURATION_MS + 300
const TARGET_TOP_OFFSET_PX = 88
const TARGET_ALIGNMENT_TOLERANCE_PX = 12
const TARGET_VISIBILITY_PADDING_PX = 24
const TARGET_MAX_ALIGNMENT_ATTEMPTS = 36
const ALIGNMENT_FRAME_DELAY = 2

function scheduleFrameDelay(frameDelay: number, callback: () => void) {
  let remainingFrames = frameDelay

  const tick = () => {
    if (remainingFrames <= 0) {
      callback()
      return
    }

    remainingFrames -= 1
    requestAnimationFrame(tick)
  }

  requestAnimationFrame(tick)
}

export function useMessageHighlight({
  rootRef,
  threadRef,
  stopAutoStick,
}: UseMessageHighlightOptions) {
  const highlightTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const jumpTokenRef = useRef(0)
  const [highlightedTarget, setHighlightedTarget] =
    useState<HighlightedMessageTarget | null>(null)

  const clearHighlightTimeout = useCallback(() => {
    if (highlightTimeoutRef.current) {
      clearTimeout(highlightTimeoutRef.current)
      highlightTimeoutRef.current = null
    }
  }, [])

  useEffect(() => clearHighlightTimeout, [clearHighlightTimeout])

  const jumpToTarget = useCallback(
    (target: SessionLocatorTarget) => {
      jumpTokenRef.current += 1
      const activeJumpToken = jumpTokenRef.current

      stopAutoStick?.()

      const nextHighlight: HighlightedMessageTarget = {
        turnId: target.turnId,
        partIndex: target.partIndex,
        token: Date.now(),
      }

      clearHighlightTimeout()
      setHighlightedTarget(null)

      threadRef.current?.scrollToIndex(target.threadIndex, {
        align: "start",
        behavior: "auto",
      })

      let attempts = 0

      const finalizeHighlight = () => {
        if (jumpTokenRef.current !== activeJumpToken) return

        setHighlightedTarget(nextHighlight)
        clearHighlightTimeout()
        highlightTimeoutRef.current = setTimeout(() => {
          setHighlightedTarget((current) =>
            current?.token === nextHighlight.token ? null : current
          )
          highlightTimeoutRef.current = null
        }, HIGHLIGHT_STATE_RESET_DELAY_MS)
      }

      const alignTarget = () => {
        if (jumpTokenRef.current !== activeJumpToken) return

        const root = rootRef.current
        const scrollElement = threadRef.current?.getScrollElement()
        if (!root || !scrollElement) {
          finalizeHighlight()
          return
        }

        const turnElement = Array.from(
          root.querySelectorAll<HTMLElement>("[data-turn-id]")
        ).find((element) => element.dataset.turnId === target.turnId)

        const targetElement =
          target.partIndex === null
            ? turnElement
            : (turnElement?.querySelector<HTMLElement>(
                `[data-content-part-index="${target.partIndex}"]`
              ) ?? turnElement)

        if (targetElement) {
          const scrollRect = scrollElement.getBoundingClientRect()
          const targetRect = targetElement.getBoundingClientRect()
          const nextTop =
            scrollElement.scrollTop +
            (targetRect.top - scrollRect.top) -
            TARGET_TOP_OFFSET_PX
          const anchorTop = scrollRect.top + TARGET_TOP_OFFSET_PX
          const distanceFromAnchor = targetRect.top - anchorTop
          const isVisible =
            targetRect.bottom >=
              scrollRect.top + TARGET_VISIBILITY_PADDING_PX &&
            targetRect.top <= scrollRect.bottom - TARGET_VISIBILITY_PADDING_PX
          const isAligned =
            Math.abs(distanceFromAnchor) <= TARGET_ALIGNMENT_TOLERANCE_PX

          if (isVisible && isAligned) {
            finalizeHighlight()
            return
          }

          scrollElement.scrollTo({
            top: Math.max(0, nextTop),
            behavior: "auto",
          })
        }

        attempts += 1
        if (attempts < TARGET_MAX_ALIGNMENT_ATTEMPTS) {
          scheduleFrameDelay(ALIGNMENT_FRAME_DELAY, alignTarget)
          return
        }

        if (process.env.NODE_ENV !== "production") {
          console.warn("[session-locator] Unable to fully align jump target", {
            partIndex: target.partIndex,
            threadIndex: target.threadIndex,
            turnId: target.turnId,
          })
        }

        finalizeHighlight()
      }

      scheduleFrameDelay(ALIGNMENT_FRAME_DELAY, alignTarget)
    },
    [clearHighlightTimeout, rootRef, stopAutoStick, threadRef]
  )

  return {
    highlightedTarget,
    jumpToTarget,
  }
}
