"use client"

/**
 * One sub-agent row: agent icon + label, task id, status badge, and the task
 * summary — clicking opens the child conversation.
 *
 * Shared between `SubAgentOverlay` (the floating card over the message area,
 * rendered inside a `MessageListView` so `useSessionViewerHost` resolves and
 * both entry points drive one viewer) and the aux panel's session-details
 * "sub-agents" section (outside any `MessageListView`, where the host is
 * null and the row falls back to the local `SubAgentSessionDialog`).
 */

import { memo, useState } from "react"
import { getAgentLabel } from "@/lib/custom-agents"
import { useTranslations } from "next-intl"

import { AgentIcon } from "@/components/agent-icon"
import { StatusBadge } from "@/components/message/delegation-status-badge"
import { SubAgentSessionDialog } from "@/components/message/sub-agent-session-dialog"
import { useSessionViewerHost } from "@/components/message/session-viewer-host"
import {
  useDelegationCardModel,
  type DelegationCardSource,
} from "@/hooks/use-delegation-card-model"

export const DelegationRow = memo(function DelegationRow({
  source,
}: {
  source: DelegationCardSource
}) {
  const t = useTranslations("Folder.chat.delegation")
  // Same host as the inline card — so the two entry points share one viewer,
  // and neither depends on its own row surviving. `null` = rendered outside a
  // `MessageListView`; keep the local drawer then.
  const viewerHost = useSessionViewerHost()
  const [dialogOpen, setDialogOpen] = useState(false)
  const {
    agentType,
    task,
    taskId,
    status,
    errorCode,
    childConversationId,
    childConnectionId,
  } = useDelegationCardModel(source)

  // Unlike the inline DelegatedSubThread (which falls through to the generic
  // tool renderer when nothing resolves), the list always renders one row
  // per real delegation so the collapsed count never disagrees with the list,
  // and meta/output-only states (e.g. after a refresh) still surface. Rows
  // degrade gracefully: unknown agent → neutral dot + "Sub-agent" label,
  // missing child id → non-clickable.
  const clickable = childConversationId != null

  const rowBody = (
    <div className="min-w-0 flex-1 space-y-1">
      {/* Name line: small icon inline with the name, then task id + status. */}
      <div className="flex items-center gap-1.5">
        <span className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-border bg-background text-foreground">
          {agentType ? (
            <AgentIcon agentType={agentType} className="h-3.5 w-3.5" />
          ) : (
            <span className="h-1.5 w-1.5 rounded-sm bg-muted-foreground/60" />
          )}
        </span>
        <span className="min-w-0 truncate text-xs font-semibold text-foreground">
          {agentType ? getAgentLabel(agentType) : t("unknownAgent")}
        </span>
        {taskId && (
          <span
            className="shrink-0 font-mono text-2xs text-muted-foreground"
            title={taskId}
          >
            #{taskId.slice(0, 8)}
          </span>
        )}
        <StatusBadge status={status} errorCode={errorCode} />
      </div>
      {task && (
        <div className="truncate text-2xs text-muted-foreground">{task}</div>
      )}
    </div>
  )

  return (
    <>
      {clickable ? (
        <button
          type="button"
          data-testid="sub-agent-row"
          onClick={() =>
            viewerHost
              ? viewerHost.open({ kind: "delegation", source })
              : setDialogOpen(true)
          }
          className="flex w-full items-center gap-2 rounded-lg border bg-transparent px-2 py-1.5 text-left transition-colors hover:bg-muted/60"
          // No aria-label: let the row content (agent name + task) name the
          // button so screen readers can tell rows apart. `title` stays for the
          // pointer tooltip.
          title={t("openDetail")}
        >
          {rowBody}
        </button>
      ) : (
        <div
          data-testid="sub-agent-row"
          className="flex w-full items-center gap-2 rounded-lg border bg-transparent px-2 py-1.5"
        >
          {rowBody}
        </div>
      )}
      {viewerHost == null && childConversationId != null && (
        <SubAgentSessionDialog
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          childConversationId={childConversationId}
          childConnectionId={childConnectionId}
          agentType={agentType}
          kickoffTask={task}
        />
      )}
    </>
  )
})
