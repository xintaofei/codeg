"use client"

import { memo, useMemo, useState, type ReactNode } from "react"
import { useTranslations } from "next-intl"
import { useShallow } from "zustand/react/shallow"
import type { AgentType, MessageTurn } from "@/lib/types"
import { useTabStore } from "@/contexts/tab-context"
import {
  selectTimelineTurns,
  useConversationRuntimeStore,
  type ConversationTimelineTurn,
} from "@/stores/conversation-runtime-store"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useConnectionStore } from "@/contexts/acp-connections-context"
import { useDelegation } from "@/contexts/delegation-context"
import { resolveActiveSessionDetails } from "@/components/conversations/active-session-details"
import { SessionDetailsContent } from "@/components/conversations/session-details-content"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import {
  createMessageTurnAdapter,
  type AdapterMessageText,
  type MessageTurnAdapter,
} from "@/lib/adapters/ai-elements-adapter"
import { extractLatestPlanEntriesFromMessages } from "@/lib/agent-plan"
import type { PlanEntryInfo } from "@/lib/types"
import {
  buildSubAgentSectionRows,
  extractDelegationSources,
  extractLiveDelegationSources,
  extractLiveNativeSubAgentSources,
  extractNativeSubAgentSources,
  type SubAgentSectionRow,
} from "@/lib/delegation-sources"
import type { DelegationCardSource } from "@/hooks/use-delegation-card-model"
import type { NativeSubAgentSource } from "@/lib/delegation-sources"
import { PlanEntryRow } from "@/components/chat/plan-entry-row"
import { DelegationRow } from "@/components/chat/delegation-row"
import { NativeSubAgentRow } from "@/components/chat/native-subagent-row"
import { BotIcon, ListTodoIcon } from "lucide-react"

// Stable empty references so the `useShallow` slices below stay
// reference-equal when there's no active session — otherwise a fresh `[]` each
// render would defeat the shallow compare and re-render on every unrelated
// streaming batch.
const EMPTY_TURNS: MessageTurn[] = []
const EMPTY_TIMELINE: ConversationTimelineTurn[] = []
const EMPTY_PLAN_ENTRIES: PlanEntryInfo[] = []
const EMPTY_SOURCES: DelegationCardSource[] = []
const EMPTY_NATIVE_SOURCES: NativeSubAgentSource[] = []
const EMPTY_SECTION_ROWS: SubAgentSectionRow[] = []

/**
 * The aux-panel "Session Details" tab. Shows the active conversation's metadata
 * and token usage (via the shared `SessionDetailsContent`); the delegated
 * sub-agents ("子代理") render INSIDE that content between the identifiers and
 * the token stats (its `beforeTokens` slot — children read as part of the
 * conversation's identity, not as an appendix after its stats), while the
 * agent plan ("任务") stays below as a trailing section.
 * The branch selector + command launcher that used to sit atop this tab now
 * live in the bottom status bar on both platforms, so the tab shows the
 * details alone.
 *
 * Details are resolved from live runtime state exactly the way the conversation
 * detail panel does it (`resolveActiveSessionDetails`), so no network fetch is
 * needed for the focused session. The two new sections share their data path
 * with the message-area floating overlays: the plan falls back from the live
 * plan block to the same `extractLatestPlanEntriesFromMessages` the overlay
 * receives; sub-agents merge every loaded turn's `delegate_to_agent` /
 * `resume_delegation` calls with the in-flight `liveMessage` calls and this
 * conversation's live delegation bindings (task-id identity, resume-tolerant).
 *
 * The runtime store is subscribed in TWO slices: the details fields (turn
 * boundaries only, `useShallow`-stable) and the streaming fields (liveMessage +
 * timeline, which the sink rewrites ~60/s). During that churn only the sections
 * below re-render — the metadata subtree sits behind a memo on stable props.
 */

// `SessionDetailsContent` is a plain function in its own module; memoizing it
// HERE (rather than exporting a memo from there, whose consumers' props are
// already stable at the call sites) keeps its token-metadata subtree out of
// every streaming batch the section subscriptions trigger.
const MemoSessionDetails = memo(SessionDetailsContent)

export function SessionDetailsTab() {
  const t = useTranslations("Folder.sessionDetails")
  const sharedT = useTranslations("Folder.chat.shared")
  const { isOpen, activeTab } = useAuxPanelContext()

  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)
  const activeConversationTab = useMemo(
    () =>
      tabs.find(
        (tab) => tab.id === activeTabId && tab.conversationId != null
      ) ?? null,
    [tabs, activeTabId]
  )

  // A brand-new conversation streams under its virtual `runtimeConversationId`
  // until it reconciles; key the live-session lookup on it first (mirrors the
  // detail panel, exercised by active-session-details.test.ts).
  const activeRuntimeId =
    activeConversationTab?.runtimeConversationId ??
    activeConversationTab?.conversationId ??
    null

  // Details slice: these fields change only at turn boundaries, so
  // `useShallow` keeps the slice reference-stable across streaming batches
  // (mirrors use-conversation-detail.ts).
  const detailsSlice = useConversationRuntimeStore(
    useShallow((s) => {
      const session =
        activeRuntimeId != null
          ? s.byConversationId.get(activeRuntimeId)
          : undefined
      return {
        detail: session?.detail ?? null,
        sessionStats: session?.sessionStats ?? null,
        localTurns: session?.localTurns ?? EMPTY_TURNS,
      }
    })
  )

  // The panel force-mounts this tab even when it's closed or on another tab,
  // so gate the streaming subscription on visibility: hidden, the tab does
  // zero adapt/bind work. (The details slice above always ran; it changes
  // only at turn boundaries.)
  const visible = isOpen && activeTab === "session_details"

  // Streaming slice: the timeline (persisted + promoted + in-flight turns) and
  // the raw liveMessage. Deliberately a SEPARATE subscription so the
  // ~60/s liveMessage churn never re-runs the details memo below; the
  // store memoizes the timeline per session object, so this slice flips
  // reference only when this conversation's session changes.
  const streamingSlice = useConversationRuntimeStore(
    useShallow((s) => {
      if (!visible || activeRuntimeId == null) {
        return { timeline: EMPTY_TIMELINE, liveMessage: null }
      }
      const session = s.byConversationId.get(activeRuntimeId)
      return {
        timeline: selectTimelineTurns(s, activeRuntimeId),
        liveMessage: session?.liveMessage ?? null,
      }
    })
  )

  const conversations = useAppWorkspaceStore((s) => s.conversations)

  const lookupRuntime = useMemo(
    () => (id: number) => (id === activeRuntimeId ? detailsSlice : null),
    [activeRuntimeId, detailsSlice]
  )
  const sessionDetails = useMemo(
    () =>
      resolveActiveSessionDetails(
        activeConversationTab,
        lookupRuntime,
        conversations
      ),
    [activeConversationTab, lookupRuntime, conversations]
  )
  const { summary, stats, model } = sessionDetails

  // `tab.id` doubles as the connection contextKey (`conversation-detail-panel`
  // passes it straight to `useConnectionLifecycle`), and `getConnection` is
  // keyed by contextKey. Read non-reactively: the id never churns.
  const connectionStore = useConnectionStore()
  const parentConnectionId =
    activeTabId != null
      ? (connectionStore.getConnection(activeTabId)?.connectionId ?? null)
      : null

  const { listAllBindings } = useDelegation()

  const adapterText = useMemo<AdapterMessageText>(
    () => ({
      attachedResources: sharedT("attachedResources"),
      toolCallFailed: sharedT("toolCallFailed"),
    }),
    [sharedT]
  )
  // Per-instance adapter: caches per-turn `AdaptedMessage` so unchanged
  // historical turns survive streaming-batch re-renders with stable refs.
  // Lazy useState (not useRef): the adapter is read during render, which the
  // react-hooks/refs rule forbids for refs (mirrors MessageListView).
  const [turnAdapter] = useState<MessageTurnAdapter>(() =>
    createMessageTurnAdapter()
  )

  const adaptedMessages = useMemo(() => {
    if (streamingSlice.timeline.length === 0) return []
    // Mirror MessageListView: mark streaming-phase turns so the adapter
    // never caches their partial state.
    const streamingIndices = new Set<number>()
    const inProgressByIndex = new Map<number, Set<string>>()
    streamingSlice.timeline.forEach((item, i) => {
      if (item.phase === "streaming") streamingIndices.add(i)
      if (item.inProgressToolCallIds && item.inProgressToolCallIds.size > 0) {
        inProgressByIndex.set(i, item.inProgressToolCallIds)
      }
    })
    return turnAdapter.adapt(
      streamingSlice.timeline.map((item) => item.turn),
      adapterText,
      streamingIndices.size > 0 ? streamingIndices : undefined,
      inProgressByIndex.size > 0 ? inProgressByIndex : undefined
    )
  }, [streamingSlice.timeline, adapterText, turnAdapter])

  const historicalPlanEntries = useMemo(() => {
    const nonStreaming = adaptedMessages.filter(
      (_, i) => streamingSlice.timeline[i].phase !== "streaming"
    )
    const entries = extractLatestPlanEntriesFromMessages(nonStreaming)
    return entries.length > 0 ? entries : EMPTY_PLAN_ENTRIES
  }, [adaptedMessages, streamingSlice.timeline])

  // Live plan wins (same precedence as the floating overlay): the runtime's
  // synthetic `plan` block on the in-flight message is the newest word from
  // the agent; the adapted turns are the fallback for history.
  const livePlanEntries = useMemo<PlanEntryInfo[]>(() => {
    const content = streamingSlice.liveMessage?.content
    if (!content) return EMPTY_PLAN_ENTRIES
    for (let i = content.length - 1; i >= 0; i -= 1) {
      const block = content[i]
      if (block.type === "plan") return block.entries
    }
    return EMPTY_PLAN_ENTRIES
  }, [streamingSlice.liveMessage])
  const planEntries =
    livePlanEntries.length > 0 ? livePlanEntries : historicalPlanEntries

  const turnDelegationSources = useMemo(() => {
    const out: DelegationCardSource[] = []
    for (const message of adaptedMessages) {
      out.push(...extractDelegationSources(message.content))
    }
    return out.length > 0 ? out : EMPTY_SOURCES
  }, [adaptedMessages])

  const liveDelegationSources = useMemo(
    () =>
      extractLiveDelegationSources(streamingSlice.liveMessage?.content ?? []),
    [streamingSlice.liveMessage]
  )

  // Every agent's OWN native children (Claude Task, codex spawn_agent, grok
  // spawn_subagent, Cursor task, OpenCode call_omo_agent), same predicate the
  // message-area capsule dispatch uses.
  const turnNativeSources = useMemo(() => {
    const out: NativeSubAgentSource[] = []
    for (const message of adaptedMessages) {
      out.push(...extractNativeSubAgentSources(message.content))
    }
    return out.length > 0 ? out : EMPTY_NATIVE_SOURCES
  }, [adaptedMessages])

  const liveNativeSources = useMemo(
    () =>
      extractLiveNativeSubAgentSources(
        streamingSlice.liveMessage?.content ?? []
      ),
    [streamingSlice.liveMessage]
  )

  const subAgentRows = useMemo(() => {
    if (
      turnDelegationSources.length === 0 &&
      liveDelegationSources.length === 0 &&
      turnNativeSources.length === 0 &&
      liveNativeSources.length === 0 &&
      parentConnectionId == null
    ) {
      return EMPTY_SECTION_ROWS
    }
    return buildSubAgentSectionRows(
      turnDelegationSources,
      liveDelegationSources,
      listAllBindings(),
      parentConnectionId,
      turnNativeSources,
      liveNativeSources
    )
  }, [
    turnDelegationSources,
    liveDelegationSources,
    turnNativeSources,
    liveNativeSources,
    listAllBindings,
    parentConnectionId,
  ])

  const isStreaming = streamingSlice.liveMessage != null

  // The 子代理 section slots INSIDE the details content, between the
  // identifier grid and the token-usage section (children read as part of the
  // conversation's identity, not as an appendix after its stats). Memoized so
  // the ~60/s streaming churn doesn't hand MemoSessionDetails a fresh element
  // each batch; only the row data / parent identity flipping re-renders it.
  const parentConversationId = activeConversationTab?.conversationId
  const parentAgentType = summary?.agent_type ?? null
  const subAgentsSlot = useMemo(
    () =>
      parentConversationId != null ? (
        <SubAgentsSection
          rows={subAgentRows}
          parentAgentType={parentAgentType}
          parentConversationId={parentConversationId}
        />
      ) : null,
    [parentConversationId, parentAgentType, subAgentRows]
  )

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      {summary ? (
        <ScrollArea className="min-h-0 flex-1">
          <div className="p-3">
            <MemoSessionDetails
              summary={summary}
              stats={stats}
              model={model}
              active={isOpen && activeTab === "session_details"}
              beforeTokens={subAgentsSlot}
            />
            <PlanSection entries={planEntries} isStreaming={isStreaming} />
          </div>
        </ScrollArea>
      ) : (
        <div className="flex min-h-0 flex-1 items-center justify-center p-6 text-center text-sm text-muted-foreground">
          {t("noActiveSession")}
        </div>
      )}
    </div>
  )
}

/**
 * Shared section-heading style: small uppercase label (same treatment as
 * `SessionDetailsContent`'s token-usage heading), leading icon, right-aligned
 * count.
 */
function SectionHeading({
  icon,
  label,
  count,
}: {
  icon: ReactNode
  label: string
  count: string
}) {
  return (
    <div className="mb-2 flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-muted-foreground">
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <span className="shrink-0 text-xs font-normal normal-case">{count}</span>
    </div>
  )
}

function PlanSection({
  entries,
  isStreaming,
}: {
  entries: PlanEntryInfo[]
  isStreaming: boolean
}) {
  const t = useTranslations("Folder.sessionDetails")
  if (entries.length === 0) return null
  const completed = entries.filter((e) => e.status === "completed").length
  return (
    <div className="mt-4 border-t pt-4">
      <SectionHeading
        icon={<ListTodoIcon className="size-3.5 shrink-0" />}
        label={t("tasksHeading")}
        count={`${completed}/${entries.length}`}
      />
      <div className="space-y-2">
        {entries.map((entry, index) => (
          <PlanEntryRow
            key={`${entry.content}-${index}`}
            entry={entry}
            isStreaming={isStreaming}
          />
        ))}
      </div>
    </div>
  )
}

function SubAgentsSection({
  rows,
  parentAgentType,
  parentConversationId,
}: {
  rows: SubAgentSectionRow[]
  parentAgentType: AgentType | null
  parentConversationId: number
}) {
  const t = useTranslations("Folder.sessionDetails")
  if (rows.length === 0) return null
  // No `mt-4` here: as the details content's `beforeTokens` slot the section
  // is a grid sibling whose top spacing comes from that container's `space-y-5`
  // (same treatment as the token-usage / timestamps sections, whose
  // `border-t pt-4` this mirrors).
  return (
    <div className="min-w-0 border-t pt-4">
      <SectionHeading
        icon={<BotIcon className="size-3.5 shrink-0" />}
        label={t("subAgentsHeading")}
        count={String(rows.length)}
      />
      <div className="space-y-1.5">
        {rows.map((row) =>
          row.kind === "delegation" ? (
            <DelegationRow
              key={`d:${row.item.source.parentToolUseId}:${row.item.source.taskIdHint ?? ""}`}
              source={row.item.source}
            />
          ) : (
            <NativeSubAgentRow
              key={`n:${row.source.toolCallId}`}
              source={row.source}
              parentAgentType={parentAgentType}
              parentConversationId={parentConversationId}
            />
          )
        )}
      </div>
    </div>
  )
}
