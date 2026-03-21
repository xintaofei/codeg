"use client"

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
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
import { MessageNavigatorOverlay } from "@/components/chat/message-navigator-overlay"
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
import { useSessionLocatorItems } from "@/hooks/use-session-locator-items"
import { useTabContext } from "@/contexts/tab-context"
import { useMessageHighlight } from "@/components/message/use-message-highlight"
import { cn } from "@/lib/utils"
import { useStickToBottomContext } from "use-stick-to-bottom"

const MESSAGE_THREAD_MAX_WIDTH_PX = 768
const MESSAGE_THREAD_CONTENT_SHELL_MAX_WIDTH_PX = 800
const MESSAGE_NAVIGATOR_COLLAPSED_EXTRA_WIDTH_PX = 16
const MESSAGE_NAVIGATOR_EXPANDED_EXTRA_WIDTH_PX = 96
const MESSAGE_NAVIGATOR_COLLAPSED_THRESHOLD_PX =
  MESSAGE_THREAD_MAX_WIDTH_PX + MESSAGE_NAVIGATOR_COLLAPSED_EXTRA_WIDTH_PX
const MESSAGE_NAVIGATOR_EXPANDED_THRESHOLD_PX =
  MESSAGE_THREAD_MAX_WIDTH_PX + MESSAGE_NAVIGATOR_EXPANDED_EXTRA_WIDTH_PX
const OVERLAY_PANEL_MIN_WIDTH_PX = 288
const OVERLAY_PANEL_MAX_WIDTH_PX = 448
const OVERLAY_PANEL_GUTTER_PADDING_PX = 12
const OVERLAY_PANEL_HORIZONTAL_INSET_PX = 32
const OVERLAY_STACK_VERTICAL_PADDING_PX = 32
const OVERLAY_STACK_GAP_PX = 12
const SHIFTED_MESSAGE_MIN_WIDTH_PX = 640
const SHIFTED_MESSAGE_MIN_LEFT_MARGIN_PX = 24
const SHIFTED_MESSAGE_MAX_LEFT_SHIFT_PX = 141
const OVERLAY_PANEL_TARGET_RIGHT_GAP_PX =
  OVERLAY_PANEL_MAX_WIDTH_PX +
  OVERLAY_PANEL_GUTTER_PADDING_PX +
  OVERLAY_PANEL_HORIZONTAL_INSET_PX

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
  const [containerSize, setContainerSize] = useState({ width: 0, height: 0 })
  const { isTileMode, tabs } = useTabContext()
  const sessionLocatorItems = useSessionLocatorItems(conversationId)
  const { highlightedTarget, jumpToTarget } = useMessageHighlight({
    rootRef,
    threadRef,
    stopAutoStick: () => stickToBottomContextRef.current?.stopScroll(),
  })

  useEffect(() => {
    const container = rootRef.current
    if (!container) return

    const updateSize = (nextWidth: number, nextHeight: number) => {
      setContainerSize((prev) => {
        if (
          Math.abs(prev.width - nextWidth) < 1 &&
          Math.abs(prev.height - nextHeight) < 1
        ) {
          return prev
        }

        return {
          width: nextWidth,
          height: nextHeight,
        }
      })
    }

    updateSize(container.clientWidth, container.clientHeight)

    const observer = new ResizeObserver((entries) => {
      updateSize(
        entries[0]?.contentRect.width ?? container.clientWidth,
        entries[0]?.contentRect.height ?? container.clientHeight
      )
    })

    observer.observe(container)
    return () => {
      observer.disconnect()
    }
  }, [detailError, detailLoading, liveMessage, timelineTurns.length])

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
  const isEffectivelyTiled = isTileMode && tabs.length > 1
  const containerWidth = containerSize.width
  const containerHeight = containerSize.height
  const shiftedThreadLayout = useMemo(() => {
    if (containerWidth <= 0) {
      return {
        overlayPanelWidthPx: OVERLAY_PANEL_MAX_WIDTH_PX,
        rowStyle: undefined,
      }
    }

    const contentShellWidth = Math.min(
      containerWidth,
      MESSAGE_THREAD_CONTENT_SHELL_MAX_WIDTH_PX
    )
    const centeredGap = Math.max(0, (containerWidth - contentShellWidth) / 2)
    const requiredExtraRightGap = Math.max(
      0,
      OVERLAY_PANEL_TARGET_RIGHT_GAP_PX - centeredGap
    )
    const leftShiftBudget = Math.min(
      SHIFTED_MESSAGE_MAX_LEFT_SHIFT_PX,
      Math.max(0, centeredGap - SHIFTED_MESSAGE_MIN_LEFT_MARGIN_PX)
    )
    const leftShift = Math.floor(
      Math.min(leftShiftBudget, requiredExtraRightGap)
    )
    const remainingExtraRightGap = Math.max(
      0,
      requiredExtraRightGap - leftShift
    )
    const widthReduction = Math.floor(
      Math.min(
        Math.max(0, contentShellWidth - SHIFTED_MESSAGE_MIN_WIDTH_PX),
        remainingExtraRightGap
      )
    )
    const targetWidth = Math.max(
      SHIFTED_MESSAGE_MIN_WIDTH_PX,
      Math.floor(contentShellWidth - widthReduction)
    )
    const marginLeft = Math.max(
      SHIFTED_MESSAGE_MIN_LEFT_MARGIN_PX,
      Math.floor(centeredGap - leftShift)
    )
    const renderedTargetWidth = Math.min(containerWidth, targetWidth)
    const rightGap = Math.max(
      0,
      containerWidth - marginLeft - renderedTargetWidth
    )
    const overlayPanelWidthPx = Math.max(
      OVERLAY_PANEL_MIN_WIDTH_PX,
      Math.min(
        OVERLAY_PANEL_MAX_WIDTH_PX,
        Math.floor(
          rightGap -
            OVERLAY_PANEL_GUTTER_PADDING_PX -
            OVERLAY_PANEL_HORIZONTAL_INSET_PX
        )
      )
    )
    const shouldApplyShiftedLayout = leftShift > 0 || widthReduction > 0

    return {
      overlayPanelWidthPx,
      rowStyle: shouldApplyShiftedLayout
        ? {
            width: `${targetWidth}px`,
            maxWidth: "100%",
            marginLeft: `${marginLeft}px`,
            marginRight: "auto",
          }
        : undefined,
    }
  }, [containerWidth])
  const overlayPanelWidthPx = shiftedThreadLayout.overlayPanelWidthPx
  const shiftedThreadRowStyle = shiftedThreadLayout.rowStyle
  const overlayAvailableHeightPx = useMemo(
    () =>
      Math.max(
        0,
        containerHeight -
          OVERLAY_STACK_VERTICAL_PADDING_PX -
          OVERLAY_STACK_GAP_PX
      ),
    [containerHeight]
  )
  const canShowNavigatorCollapsed =
    sessionLocatorItems.length > 0 &&
    (isEffectivelyTiled ||
      containerWidth >= MESSAGE_NAVIGATOR_COLLAPSED_THRESHOLD_PX)
  const showMessageNavigator = canShowNavigatorCollapsed
  const expandMessageNavigatorByDefault =
    !hasAgentPlanOverlay &&
    containerWidth >= MESSAGE_NAVIGATOR_EXPANDED_THRESHOLD_PX
  const splitOverlayHeights = hasAgentPlanOverlay && showMessageNavigator
  const planPanelMaxHeightPx = splitOverlayHeights
    ? Math.floor(overlayAvailableHeightPx * 0.4)
    : overlayAvailableHeightPx || undefined
  const navigatorPanelMaxHeightPx = splitOverlayHeights
    ? Math.floor(overlayAvailableHeightPx * 0.6)
    : overlayAvailableHeightPx || undefined

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
          rowContainerStyle={shiftedThreadRowStyle}
        />
      </MessageThread>
      {liveMessage && connStatus === "prompting" && (
        <LiveTurnStats
          message={liveMessage}
          agentType={agentType}
          isStreaming={connStatus === "prompting"}
        />
      )}
      {(hasAgentPlanOverlay || sessionLocatorItems.length > 0) && (
        <div className="pointer-events-none absolute inset-x-0 top-4 bottom-4 z-20 px-4 sm:px-8">
          <div className="flex h-full min-h-0 flex-col items-end gap-3">
            {hasAgentPlanOverlay && (
              <AgentPlanOverlay
                key={agentPlanOverlayKey}
                className="max-w-full"
                message={liveMessage ?? null}
                entries={historicalPlanEntries}
                planKey={historicalPlanKey}
                defaultExpanded={connStatus === "prompting"}
                panelWidthPx={overlayPanelWidthPx}
                panelMaxHeightPx={planPanelMaxHeightPx}
              />
            )}
            {sessionLocatorItems.length > 0 && (
              <MessageNavigatorOverlay
                className="max-w-full"
                items={sessionLocatorItems}
                locatorKey={`conversation-${conversationId}`}
                visible={showMessageNavigator}
                onJumpToTarget={jumpToTarget}
                defaultExpanded={expandMessageNavigatorByDefault}
                panelWidthPx={overlayPanelWidthPx}
                panelMaxHeightPx={navigatorPanelMaxHeightPx}
              />
            )}
          </div>
        </div>
      )}
    </div>
  )
}
