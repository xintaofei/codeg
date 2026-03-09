"use client"

import { memo, useCallback, useEffect, useMemo, useRef } from "react"
import { useDbMessageDetail } from "@/hooks/use-db-message-detail"
import { ContentPartsRenderer } from "./content-parts-renderer"
import {
  adaptMessageTurns,
  type AdaptedMessage,
  type AdaptedContentPart,
  type MessageGroup,
  type UserImageDisplay,
  type UserResourceDisplay,
  groupAdaptedMessages,
  extractUserResourcesFromText,
} from "@/lib/adapters/ai-elements-adapter"
import { TurnStats } from "./turn-stats"
import { LiveTurnStats } from "./live-turn-stats"
import { UserResourceLinks } from "./user-resource-links"
import { UserImageAttachments } from "./user-image-attachments"
import { useSessionStats } from "@/contexts/session-stats-context"
import { LiveMessageBlock } from "@/components/chat/live-message-block"
import { AgentPlanOverlay } from "@/components/chat/agent-plan-overlay"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import { MessageThread } from "@/components/ai-elements/message-thread"
import { Message, MessageContent } from "@/components/ai-elements/message"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import {
  buildPlanKey,
  extractLatestPlanEntriesFromMessages,
} from "@/lib/agent-plan"
import type { ConnectionStatus } from "@/lib/types"
import { VirtualizedMessageThread } from "@/components/message/virtualized-message-thread"

interface MessageListViewProps {
  conversationId: number
  connStatus?: ConnectionStatus | null
  liveMessage?: LiveMessage | null
  pendingMessages?: AdaptedMessage[]
  onPendingClear?: () => void
  isActive?: boolean
}

interface ResolvedMessageGroup extends MessageGroup {
  parts: AdaptedContentPart[]
  resources: UserResourceDisplay[]
  images: UserImageDisplay[]
}

type ThreadRenderItem =
  | {
      key: string
      kind: "historical"
      group: ResolvedMessageGroup
    }
  | {
      key: string
      kind: "pending"
      group: ResolvedMessageGroup
    }
  | {
      key: string
      kind: "typing"
    }
  | {
      key: string
      kind: "live"
      message: LiveMessage
    }

function fallbackExtractUserResources(
  group: MessageGroup,
  attachedResourcesText: string
): {
  parts: AdaptedContentPart[]
  resources: UserResourceDisplay[]
  images: UserImageDisplay[]
} {
  if (group.role !== "user") {
    return {
      parts: group.parts,
      resources: group.userResources ?? [],
      images: group.userImages ?? [],
    }
  }

  const parsedResources: UserResourceDisplay[] = []
  const parsedParts: AdaptedContentPart[] = []

  for (const part of group.parts) {
    if (part.type !== "text") {
      parsedParts.push(part)
      continue
    }
    const extracted = extractUserResourcesFromText(part.text)
    if (extracted.resources.length > 0) {
      parsedResources.push(...extracted.resources)
      if (extracted.text.length > 0) {
        parsedParts.push({ type: "text", text: extracted.text })
      }
    } else {
      parsedParts.push(part)
    }
  }

  const resources = [...(group.userResources ?? []), ...parsedResources]
  const dedupedResources: UserResourceDisplay[] = []
  const seen = new Set<string>()
  for (const resource of resources) {
    const key = `${resource.name}::${resource.uri}`
    if (seen.has(key)) continue
    seen.add(key)
    dedupedResources.push(resource)
  }

  if (parsedParts.length === 0 && dedupedResources.length > 0) {
    parsedParts.push({ type: "text", text: attachedResourcesText })
  }

  return {
    parts: parsedParts,
    resources: dedupedResources,
    images: group.userImages ?? [],
  }
}

function resolveMessageGroup(
  group: MessageGroup,
  attachedResourcesText: string
): ResolvedMessageGroup {
  const resolved = fallbackExtractUserResources(group, attachedResourcesText)
  return {
    ...group,
    parts: resolved.parts,
    resources: resolved.resources,
    images: resolved.images,
  }
}

const HistoricalMessageGroup = memo(function HistoricalMessageGroup({
  group,
}: {
  group: ResolvedMessageGroup
}) {
  return (
    <div>
      <Message from={group.role}>
        {group.role === "user" && group.images.length > 0 ? (
          <UserImageAttachments images={group.images} className="self-end" />
        ) : null}
        <MessageContent>
          <ContentPartsRenderer parts={group.parts} role={group.role} />
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

const PendingMessageGroup = memo(function PendingMessageGroup({
  group,
}: {
  group: ResolvedMessageGroup
}) {
  return (
    <div className="opacity-70">
      <Message from={group.role}>
        {group.role === "user" && group.images.length > 0 ? (
          <UserImageAttachments images={group.images} className="self-end" />
        ) : null}
        <MessageContent>
          <ContentPartsRenderer parts={group.parts} role={group.role} />
        </MessageContent>
        {group.role === "user" && group.resources.length > 0 ? (
          <UserResourceLinks resources={group.resources} className="self-end" />
        ) : null}
      </Message>
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

export function MessageListView({
  conversationId,
  connStatus,
  liveMessage,
  pendingMessages,
  onPendingClear,
  isActive = true,
}: MessageListViewProps) {
  const t = useTranslations("Folder.chat.messageList")
  const sharedT = useTranslations("Folder.chat.shared")
  const { detail, loading, error } = useDbMessageDetail(conversationId)
  const turnCount = detail?.turns.length ?? 0

  // 移除了 prompting 结束后的立即刷新
  // 原因：后端自动持久化可能有延迟，立即刷新会读到不完整数据
  // 现在通过清空 pending 来避免累积问题，等用户切换会话或手动刷新时再加载

  const prevTurnCountRef = useRef(turnCount)
  const prevConvIdRef = useRef(conversationId)
  useEffect(() => {
    if (prevConvIdRef.current !== conversationId) {
      prevConvIdRef.current = conversationId
      prevTurnCountRef.current = turnCount
      return
    }
    if (turnCount > prevTurnCountRef.current && onPendingClear) {
      onPendingClear()
    }
    prevTurnCountRef.current = turnCount
  }, [turnCount, onPendingClear, conversationId])

  const { setSessionStats } = useSessionStats()
  const sessionStats = detail?.session_stats ?? null

  useEffect(() => {
    if (isActive) {
      setSessionStats(sessionStats)
    }
  }, [isActive, sessionStats, setSessionStats])

  const shouldUseSmoothResize = !(isActive && !loading && detail)

  const messages = useMemo(
    () =>
      detail
        ? adaptMessageTurns(detail.turns, {
            attachedResources: sharedT("attachedResources"),
            toolCallFailed: sharedT("toolCallFailed"),
          })
        : [],
    [detail, sharedT]
  )

  const groups = useMemo(() => groupAdaptedMessages(messages), [messages])
  const historicalPlanEntries = useMemo(
    () => extractLatestPlanEntriesFromMessages(messages),
    [messages]
  )
  const historicalPlanKey = useMemo(
    () => buildPlanKey(historicalPlanEntries),
    [historicalPlanEntries]
  )

  const pendingGroups = useMemo(
    () =>
      pendingMessages?.length ? groupAdaptedMessages(pendingMessages) : [],
    [pendingMessages]
  )
  const attachedResourcesText = sharedT("attachedResources")

  const resolvedGroups = useMemo(
    () =>
      groups.map((group) => resolveMessageGroup(group, attachedResourcesText)),
    [groups, attachedResourcesText]
  )
  const resolvedPendingGroups = useMemo(
    () =>
      pendingGroups.map((group) =>
        resolveMessageGroup(group, attachedResourcesText)
      ),
    [pendingGroups, attachedResourcesText]
  )

  const showLiveMessage = Boolean(
    liveMessage &&
    (connStatus === "prompting" ||
      (liveMessage.content.length > 0 && resolvedPendingGroups.length > 0))
  )

  const threadItems = useMemo<ThreadRenderItem[]>(() => {
    const items: ThreadRenderItem[] = [
      ...resolvedGroups.map((group) => ({
        key: `history-${group.id}`,
        kind: "historical" as const,
        group,
      })),
      ...resolvedPendingGroups.map((group) => ({
        key: `pending-${group.id}`,
        kind: "pending" as const,
        group,
      })),
    ]

    if (resolvedPendingGroups.length > 0 && !showLiveMessage) {
      items.push({ key: "pending-typing", kind: "typing" })
    }

    if (showLiveMessage && liveMessage) {
      items.push({
        key: `live-${liveMessage.id}`,
        kind: "live",
        message: liveMessage,
      })
    }

    return items
  }, [resolvedGroups, resolvedPendingGroups, showLiveMessage, liveMessage])

  const renderThreadItem = useCallback((item: ThreadRenderItem) => {
    switch (item.kind) {
      case "historical":
        return <HistoricalMessageGroup group={item.group} />
      case "pending":
        return <PendingMessageGroup group={item.group} />
      case "typing":
        return <PendingTypingIndicator />
      case "live":
        return <LiveMessageBlock message={item.message} />
      default:
        return null
    }
  }, [])

  const emptyState = useMemo(
    () => (
      <div className="px-4 py-12 text-center">
        <p className="text-muted-foreground text-sm">
          {t("emptyConversation")}
        </p>
      </div>
    ),
    [t]
  )

  const agentPlanOverlayKey = liveMessage?.id ?? `history-${conversationId}`

  if (loading && !detail) {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          <span>{t("loading")}</span>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="p-6">
        <div className="text-center py-12">
          <p className="text-destructive text-sm">
            {t("error", { message: error })}
          </p>
        </div>
      </div>
    )
  }

  if (!detail) return null

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      <MessageThread
        className="flex-1 min-h-0"
        resize={shouldUseSmoothResize ? "smooth" : undefined}
      >
        <VirtualizedMessageThread
          items={threadItems}
          getItemKey={(item) => item.key}
          renderItem={renderThreadItem}
          emptyState={emptyState}
          estimateSize={180}
          overscan={10}
        />
      </MessageThread>
      {showLiveMessage && liveMessage && (
        <LiveTurnStats message={liveMessage} />
      )}
      <AgentPlanOverlay
        key={agentPlanOverlayKey}
        message={liveMessage ?? null}
        entries={historicalPlanEntries}
        planKey={historicalPlanKey}
        defaultExpanded={showLiveMessage}
      />
    </div>
  )
}
