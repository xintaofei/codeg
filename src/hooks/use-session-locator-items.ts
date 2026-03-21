"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import { adaptMessageTurns } from "@/lib/adapters/ai-elements-adapter"
import {
  buildSessionLocatorItems,
  type SessionLocatorItem,
  type SessionLocatorRawTurn,
} from "@/lib/session-locator"

export function useSessionLocatorItems(
  conversationId: number | null
): SessionLocatorItem[] {
  const sharedT = useTranslations("Folder.chat.shared")
  const { getTimelineTurns } = useConversationRuntime()
  const resolvedConversationId = conversationId ?? -1
  const timelineTurns = getTimelineTurns(resolvedConversationId)

  const adapterText = useMemo(
    () => ({
      attachedResources: sharedT("attachedResources"),
      toolCallFailed: sharedT("toolCallFailed"),
    }),
    [sharedT]
  )

  return useMemo(() => {
    if (conversationId == null || timelineTurns.length === 0) {
      return []
    }

    const allTurns = timelineTurns.map((item) => item.turn)
    const streamingIndices = new Set<number>()

    timelineTurns.forEach((item, index) => {
      if (item.phase === "streaming") {
        streamingIndices.add(index)
      }
    })

    const adaptedTurns = adaptMessageTurns(
      allTurns,
      adapterText,
      streamingIndices.size > 0 ? streamingIndices : undefined
    )

    const locatorRawTurns: SessionLocatorRawTurn[] = adaptedTurns.map(
      (message, threadIndex) => ({
        turnId: message.id,
        role: message.role === "tool" ? "assistant" : message.role,
        phase: timelineTurns[threadIndex]?.phase ?? "persisted",
        threadIndex,
        parts: message.content,
        resourceCount: message.userResources?.length ?? 0,
        imageCount: message.userImages?.length ?? 0,
      })
    )

    return buildSessionLocatorItems(
      locatorRawTurns.filter((turn) => {
        if (turn.role === "system") return false
        if (turn.phase === "streaming") return false
        if (turn.phase === "optimistic") return turn.role === "user"
        return true
      })
    )
  }, [adapterText, conversationId, timelineTurns])
}
