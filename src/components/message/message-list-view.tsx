"use client"

import { memo, useCallback, useEffect, useMemo, useRef } from "react"
import type { StickToBottomContext } from "use-stick-to-bottom"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import { ContentPartsRenderer } from "./content-parts-renderer"
import {
  adaptMessageTurns,
  type AdaptedContentPart,
  type UserImageDisplay,
  type UserResourceDisplay,
} from "@/lib/adapters/ai-elements-adapter"
import { TurnStats } from "./turn-stats"
import { LiveTurnStats } from "./live-turn-stats"
import { UserResourceLinks } from "./user-resource-links"
import { UserImageAttachments } from "./user-image-attachments"
import { useSessionStats } from "@/contexts/session-stats-context"
import {
  AgentPlanOverlay,
  getLatestPlanEntries,
} from "@/components/chat/agent-plan-overlay"
import { MessageThread } from "@/components/ai-elements/message-thread"
import { Message, MessageContent } from "@/components/ai-elements/message"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import {
  buildPlanKey,
  extractLatestPlanEntriesFromMessages,
} from "@/lib/agent-plan"
import type { AgentType, ConnectionStatus, SessionStats } from "@/lib/types"
import {
  VirtualizedMessageThread,
  type VirtualizedMessageThreadHandle,
} from "@/components/message/virtualized-message-thread"
import { useMessageHighlight } from "@/components/message/use-message-highlight"
import { useSessionLocatorContext } from "@/contexts/session-locator-context"
import { cn } from "@/lib/utils"
import { useStickToBottomContext } from "use-stick-to-bottom"

interface MessageListViewProps {
  conversationId: number
  agentType: AgentType
  connStatus?: ConnectionStatus | null
  isActive?: boolean
  sendSignal?: number
  sessionStats?: SessionStats | null
  detailLoading?: boolean
  detailError?: string | null
  hideEmptyState?: boolean
}

interface ResolvedMessageGroup {
  id: string
  role: "user" | "assistant" | "system"
  parts: AdaptedContentPart[]
  resources: UserResourceDisplay[]
  images: UserImageDisplay[]
  usage?: import("@/lib/types").TurnUsage | null
  duration_ms?: number | null
  model?: string | null
  models?: string[]
}

type ThreadRenderItem =
  | {
      key: string
      kind: "turn"
      group: ResolvedMessageGroup
      phase: "persisted" | "optimistic" | "streaming"
    }
  | {
      key: string
      kind: "typing"
    }

const HistoricalMessageGroup = memo(function HistoricalMessageGroup({
  group,
  dimmed = false,
  highlightedPartIndex = null,
  highlightTurn = false,
  highlightToken,
}: {
  group: ResolvedMessageGroup
  dimmed?: boolean
  highlightedPartIndex?: number | null
  highlightTurn?: boolean
  highlightToken?: number
}) {
  return (
    <div
      className={cn(
        "rounded-2xl",
        dimmed && "opacity-70",
        highlightTurn &&
          highlightToken !== undefined &&
          "session-locator-turn-highlight"
      )}
      data-turn-id={group.id}
      data-highlight-token={highlightToken}
    >
      <Message from={group.role}>
        {group.role === "user" && group.images.length > 0 ? (
          <UserImageAttachments images={group.images} className="self-end" />
        ) : null}
        <MessageContent>
          <ContentPartsRenderer
            parts={group.parts}
            role={group.role}
            highlightedPartIndex={highlightedPartIndex}
            highlightToken={highlightToken}
          />
        </MessageContent>
        {group.role === "user" && group.resources.length > 0 ? (
          <UserResourceLinks resources={group.resources} className="self-end" />
        ) : null}
      </Message>
      {group.role === "assistant" && (
        <TurnStats
          usage={group.usage}
          duration_ms={group.duration_ms}
          model={group.model}
          models={group.models}
        />
      )}
    </div>
  )
})

const PendingTypingIndicator = memo(function PendingTypingIndicator() {
  return (
    <Message from="assistant">
      <MessageContent>
        <div className="flex items-center gap-1.5 py-1">
          <span className="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground/60 animate-[pulse_1.4s_ease-in-out_infinite]" />
          <span className="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground/60 animate-[pulse_1.4s_ease-in-out_0.2s_infinite]" />
          <span className="inline-block h-1.5 w-1.5 rounded-full bg-muted-foreground/60 animate-[pulse_1.4s_ease-in-out_0.4s_infinite]" />
        </div>
      </MessageContent>
    </Message>
  )
})

const AutoScrollOnSend = memo(function AutoScrollOnSend({
  signal,
}: {
  signal: number
}) {
  const { scrollToBottom } = useStickToBottomContext()
  const lastSignalRef = useRef(signal)

  useEffect(() => {
    if (signal === lastSignalRef.current) return
    lastSignalRef.current = signal

    scrollToBottom()
    const rafId = requestAnimationFrame(() => {
      scrollToBottom()
    })
    return () => {
      cancelAnimationFrame(rafId)
    }
  }, [scrollToBottom, signal])

  return null
})

export function MessageListView({
  conversationId,
  agentType,
  connStatus,
  isActive = true,
  sendSignal = 0,
  sessionStats = null,
  detailLoading = false,
  detailError = null,
  hideEmptyState = false,
}: MessageListViewProps) {
  const t = useTranslations("Folder.chat.messageList")
  const sharedT = useTranslations("Folder.chat.shared")
  const { getSession, getTimelineTurns } = useConversationRuntime()
  const session = getSession(conversationId)
  const liveMessage = session?.liveMessage ?? null
  const timelineTurns = getTimelineTurns(conversationId)

  const { setSessionStats } = useSessionStats()
  const rootRef = useRef<HTMLDivElement>(null)
  const threadRef = useRef<VirtualizedMessageThreadHandle | null>(null)
  const stickToBottomContextRef = useRef<StickToBottomContext | null>(null)
  const { registerJumpHandler } = useSessionLocatorContext()
  const { highlightedTarget, jumpToTarget } = useMessageHighlight({
    rootRef,
    threadRef,
    stopAutoStick: () => stickToBottomContextRef.current?.stopScroll(),
  })

  useEffect(
    () => registerJumpHandler(conversationId, jumpToTarget),
    [conversationId, jumpToTarget, registerJumpHandler]
  )

  useEffect(() => {
    if (isActive) {
      setSessionStats(sessionStats)
    }
  }, [isActive, sessionStats, setSessionStats])

  const shouldUseSmoothResize = !(
    isActive &&
    !detailLoading &&
    timelineTurns.length
  )

  const adapterText = useMemo(
    () => ({
      attachedResources: sharedT("attachedResources"),
      toolCallFailed: sharedT("toolCallFailed"),
    }),
    [sharedT]
  )

  const sessionSyncState = session?.syncState ?? "idle"

  const { threadItems, nonStreamingAdapted } = useMemo(() => {
    const allTurns = timelineTurns.map((item) => item.turn)
    const streamingIndices = new Set<number>()
    timelineTurns.forEach((item, i) => {
      if (item.phase === "streaming") streamingIndices.add(i)
    })
    const allAdapted = adaptMessageTurns(
      allTurns,
      adapterText,
      streamingIndices.size > 0 ? streamingIndices : undefined
    )

    // Collect non-streaming adapted messages for plan extraction
    const nonStreaming = allAdapted.filter(
      (_, index) => timelineTurns[index].phase !== "streaming"
    )

    // Map each adapted message directly to a render item (1:1).
    // Backend group_into_turns() already ensures each turn is a complete unit.
    const items: ThreadRenderItem[] = allAdapted.map((msg, i) => {
      const phase = timelineTurns[i].phase
      const role = msg.role === "tool" ? "assistant" : msg.role
      return {
        key: `${phase}-${msg.id}-${i}`,
        kind: "turn" as const,
        group: {
          id: msg.id,
          role,
          parts: msg.content,
          resources: msg.userResources ?? [],
          images: msg.userImages ?? [],
          usage: msg.usage,
          duration_ms: msg.duration_ms,
          model: msg.model,
        },
        phase,
      }
    })

    const lastPhase = timelineTurns[timelineTurns.length - 1]?.phase ?? null
    if (
      lastPhase === "optimistic" &&
      (connStatus === "prompting" || sessionSyncState === "awaiting_persist")
    ) {
      items.push({ key: "pending-typing", kind: "typing" })
    }

    return { threadItems: items, nonStreamingAdapted: nonStreaming }
  }, [adapterText, connStatus, sessionSyncState, timelineTurns])

  const historicalPlanEntries = useMemo(
    () => extractLatestPlanEntriesFromMessages(nonStreamingAdapted),
    [nonStreamingAdapted]
  )
  const historicalPlanKey = useMemo(
    () => buildPlanKey(historicalPlanEntries),
    [historicalPlanEntries]
  )
  const livePlanEntries = useMemo(
    () => getLatestPlanEntries(liveMessage ?? null),
    [liveMessage]
  )

  const renderThreadItem = useCallback(
    (item: ThreadRenderItem) => {
      switch (item.kind) {
        case "turn": {
          const isHighlightedTurn = highlightedTarget?.turnId === item.group.id
          const highlightedPartIndex = isHighlightedTurn
            ? highlightedTarget.partIndex
            : null
          const shouldHighlightWholeTurn =
            isHighlightedTurn && highlightedPartIndex === null
          return (
            <HistoricalMessageGroup
              group={item.group}
              dimmed={item.phase === "optimistic"}
              highlightedPartIndex={highlightedPartIndex}
              highlightTurn={shouldHighlightWholeTurn}
              highlightToken={
                isHighlightedTurn ? highlightedTarget.token : undefined
              }
            />
          )
        }
        case "typing":
          return <PendingTypingIndicator />
        default:
          return null
      }
    },
    [highlightedTarget]
  )

  const emptyState = useMemo(
    () =>
      hideEmptyState ? null : (
        <div className="px-4 py-12 text-center">
          <p className="text-muted-foreground text-sm">
            {t("emptyConversation")}
          </p>
        </div>
      ),
    [hideEmptyState, t]
  )

  const agentPlanOverlayKey = liveMessage?.id ?? `history-${conversationId}`
  const hasRenderableContent = threadItems.length > 0 || Boolean(liveMessage)
  const hasAgentPlanOverlay =
    livePlanEntries.length > 0 || historicalPlanEntries.length > 0

  if (detailLoading && !hasRenderableContent) {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          <span>{t("loading")}</span>
        </div>
      </div>
    )
  }

  if (detailError && !hasRenderableContent) {
    return (
      <div className="p-6">
        <div className="text-center py-12">
          <p className="text-destructive text-sm">
            {t("error", { message: detailError })}
          </p>
        </div>
      </div>
    )
  }

  return (
    <div ref={rootRef} className="relative flex h-full min-h-0 flex-col">
      <MessageThread
        className="flex-1 min-h-0"
        contextRef={stickToBottomContextRef}
        resize={shouldUseSmoothResize ? "smooth" : undefined}
      >
        <AutoScrollOnSend signal={sendSignal} />
        <VirtualizedMessageThread
          ref={threadRef}
          items={threadItems}
          getItemKey={(item) => item.key}
          renderItem={renderThreadItem}
          emptyState={emptyState}
          estimateSize={180}
          overscan={10}
        />
      </MessageThread>
      {liveMessage && connStatus === "prompting" && (
        <LiveTurnStats
          message={liveMessage}
          agentType={agentType}
          isStreaming={connStatus === "prompting"}
        />
      )}
      {hasAgentPlanOverlay && (
        <div className="pointer-events-none absolute inset-x-0 top-4 z-20 px-4 sm:px-8">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-start">
            <AgentPlanOverlay
              key={agentPlanOverlayKey}
              className="max-w-full sm:ml-auto"
              message={liveMessage ?? null}
              entries={historicalPlanEntries}
              planKey={historicalPlanKey}
              defaultExpanded={connStatus === "prompting"}
            />
          </div>
        </div>
      )}
    </div>
  )
}
