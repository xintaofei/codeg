"use client"

/**
 * One native sub-agent row: the child's agent icon + type/description title,
 * a status badge, and (when the parser folded them in) the run duration —
 * clicking opens the child's own session transcript.
 *
 * The visual sibling of `DelegationRow`: same row chrome so the aux panel's
 * "sub-agents" section reads as one uniform list, but the data path is the
 * agent's OWN spawn (`Task` / `spawn_agent` / `spawn_subagent` / `task` /
 * `call_omo_agent`), not a codeg `delegate_to_agent`. The title and the
 * child-session resolution follow `AgentToolCallPart` — the message-area
 * capsule — so the row and the capsule describe the same child the same way.
 *
 * Clicking upserts the child handle as a hidden delegate conversation
 * (`open_native_subagent_session`) and opens it as an ORDINARY tab beside the
 * parent — the full chat window shows the child transcript, with the composer
 * gated by the connection's session (see `conversation-detail-panel`). If the
 * upsert fails the row falls back to the read-only `SubagentSessionDialog`,
 * the same transcript view without the tab.
 */

import { memo, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import type { AgentType } from "@/lib/types"
import { getAgentLabel } from "@/lib/custom-agents"
import { formatDuration } from "@/lib/delegation-status"
import {
  isAsyncLaunchAckText,
  parseBackgroundTaskMarker,
} from "@/lib/background-agent"
import {
  childSessionOfLaunch,
  parseSubAgentLaunchFields,
} from "@/lib/native-subagent-fields"
import { openNativeSubagentSession } from "@/lib/api"
import { useTabStore } from "@/contexts/tab-context"

import { AgentIcon } from "@/components/agent-icon"
import { StatusBadge } from "@/components/message/delegation-status-badge"
import { SubagentSessionDialog } from "@/components/message/subagent-session-dialog"
import type { NativeSubAgentSource } from "@/lib/delegation-sources"

export const NativeSubAgentRow = memo(function NativeSubAgentRow({
  source,
  /** The conversation's own agent: a native child is a child OF this agent,
   *  so the row's icon falls back to it whenever the child names no type of
   *  its own, and the child handle's agent type is derived from it. */
  parentAgentType,
  /** The conversation whose session details host this row — the tab the
   *  child opens next to, and the cascade parent recorded with it. */
  parentConversationId,
}: {
  source: NativeSubAgentSource
  parentAgentType: AgentType | null
  parentConversationId: number
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const tRow = useTranslations("Folder.chat.delegation")
  const [dialogOpen, setDialogOpen] = useState(false)
  const [opening, setOpening] = useState(false)
  const openTab = useTabStore((s) => s.openTab)
  const registerNativeChildTab = useTabStore((s) => s.registerNativeChildTab)
  const rawTabs = useTabStore((s) => s.rawTabs)

  const isRunning =
    source.state === "input-available" || source.state === "input-streaming"
  const isError = source.state === "output-error"

  // The SAME parser the message-area capsule uses, so the row and the capsule
  // describe the child identically.
  const launchFields = useMemo(
    () => parseSubAgentLaunchFields(source.input),
    [source.input]
  )
  const {
    subagentType,
    description,
    isCodexSubagentLaunch,
    codexSubagentState,
  } = launchFields

  // Background children settle later than their launching call, and a codex
  // launch-settles on the spawn ACK — both mean "output-available" says
  // nothing about the child. Mirror the capsule's background reading.
  const backgroundLifecycle = useMemo(
    () => parseBackgroundTaskMarker(source.output),
    [source.output]
  )
  const isLiveBackgroundLaunch =
    backgroundLifecycle === null &&
    source.state === "output-available" &&
    isAsyncLaunchAckText(source.output)
  const backgroundSettled = backgroundLifecycle?.status != null
  const backgroundFailed =
    backgroundSettled && backgroundLifecycle?.status !== "completed"
  // A codex child whose own end the harness reported (`SubAgentActivity`).
  const codexChildDone =
    isCodexSubagentLaunch && codexSubagentState === "completed"
  const codexChildInterrupted =
    isCodexSubagentLaunch && codexSubagentState === "interrupted"

  const agentStats = source.agentStats ?? null
  const duration = agentStats?.total_duration_ms
    ? formatDuration(agentStats.total_duration_ms)
    : null

  const childSession = useMemo(
    () =>
      childSessionOfLaunch(
        launchFields,
        source.meta,
        agentStats,
        parentAgentType
      ),
    [launchFields, source.meta, agentStats, parentAgentType]
  )

  const title = subagentType
    ? description
      ? `${subagentType}: ${description}`
      : subagentType
    : description || t("agentFallbackTitle")

  // StatusBadge speaks the delegation vocabulary; natives map onto the same
  // three meaningful states (a background/codex child still out is RUNNING
  // even though the launch call settled).
  const badgeStatus: "running" | "ok" | "err" =
    isRunning ||
    isLiveBackgroundLaunch ||
    (isCodexSubagentLaunch && !codexChildDone && !codexChildInterrupted)
      ? "running"
      : isError || backgroundFailed || codexChildInterrupted
        ? "err"
        : "ok"

  const iconAgentType: AgentType | null =
    childSession?.agentType ??
    (agentStats?.agent_type as AgentType | null | undefined) ??
    parentAgentType

  const clickable = childSession != null

  const openChild = async () => {
    if (!childSession || opening) return
    setOpening(true)
    try {
      const res = await openNativeSubagentSession(
        parentConversationId,
        childSession.sessionId,
        title
      )
      registerNativeChildTab(res.conversationId, parentConversationId)
      // Pin: an unpinned child would take (or evict) the group's preview slot
      // — the pair must sit side by side as stable tabs. Insert right after
      // the parent when its tab is open; append otherwise.
      const parentIndex = rawTabs.findIndex(
        (t) => t.conversationId === parentConversationId
      )
      openTab(
        res.folderId,
        res.conversationId,
        res.agentType,
        true,
        title,
        parentIndex >= 0 ? { index: parentIndex + 1 } : undefined
      )
    } catch {
      // Upsert failed (offline, deleted parent, …): the dialog still reads
      // the child's transcript straight off disk.
      setDialogOpen(true)
    } finally {
      setOpening(false)
    }
  }

  const rowBody = (
    <div className="min-w-0 flex-1 space-y-1">
      {/* Name line: icon chip + title, then duration + status. Same layout as
          DelegationRow so one section reads as one list. */}
      <div className="flex items-center gap-1.5">
        <span className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-border bg-background text-foreground">
          {iconAgentType ? (
            <AgentIcon agentType={iconAgentType} className="h-3.5 w-3.5" />
          ) : (
            <span className="h-1.5 w-1.5 rounded-sm bg-muted-foreground/60" />
          )}
        </span>
        <span
          className="min-w-0 truncate text-xs font-semibold text-foreground"
          title={title}
        >
          {title}
        </span>
        {duration && (
          <span className="shrink-0 font-mono text-2xs text-muted-foreground">
            {duration}
          </span>
        )}
        <StatusBadge status={badgeStatus} />
      </div>
      {iconAgentType && (
        <div className="truncate text-2xs text-muted-foreground">
          {getAgentLabel(iconAgentType)}
        </div>
      )}
    </div>
  )

  return (
    <>
      {clickable ? (
        <button
          type="button"
          data-testid="native-subagent-row"
          onClick={openChild}
          className="flex w-full items-center gap-2 rounded-lg border bg-transparent px-2 py-1.5 text-left transition-colors hover:bg-muted/60"
          title={tRow("openDetail")}
        >
          {rowBody}
        </button>
      ) : (
        <div
          data-testid="native-subagent-row"
          className="flex w-full items-center gap-2 rounded-lg border bg-transparent px-2 py-1.5"
        >
          {rowBody}
        </div>
      )}
      {childSession != null && (
        <SubagentSessionDialog
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          sessionId={childSession.sessionId}
          agentType={childSession.agentType}
          subagentType={subagentType}
          description={description}
          live={isRunning || isLiveBackgroundLaunch}
        />
      )}
    </>
  )
})
