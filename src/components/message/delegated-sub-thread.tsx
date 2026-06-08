"use client"

/**
 * Inline header for a delegated child sub-session under the parent's
 * `delegate_to_agent` ToolCallBlock. Renders as a self-contained card —
 * never falls through the generic tool-call shell — so users see "Agent
 * delegating: task" instead of "mcp__codeg-delegate__delegate_to_agent: codex".
 *
 * The card is intentionally a status + navigation affordance ONLY: it does not
 * render the child's output inline and does not expand. The child's result is
 * delivered to the LLM via `get_delegation_status` and to the user by opening
 * the child session ("查看会话" → SubAgentSessionDialog, which also hosts the
 * child's permission prompts). When the child is awaiting a permission decision
 * the status badge reflects it, cueing the user to open the session.
 *
 * All agent-type / task / status / child-id resolution lives in
 * `useDelegationCardModel` (shared with the top-right `SubAgentOverlay`), so the
 * card and the overlay never disagree about a sub-agent.
 */

import { useState } from "react"
import { Eye } from "lucide-react"
import { useTranslations } from "next-intl"

import { AgentIcon } from "@/components/agent-icon"
import { AGENT_LABELS } from "@/lib/types"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"
import { StatusBadge } from "@/components/message/delegation-status-badge"
import { SubAgentSessionDialog } from "@/components/message/sub-agent-session-dialog"
import { useDelegationCardModel } from "@/hooks/use-delegation-card-model"

interface Props {
  parentToolUseId: string
  /** Raw JSON arguments the LLM sent to `delegate_to_agent`. Used to
   *  surface the task and agent_type before the broker's
   *  DelegationStarted event lands (or when binding never arrives — e.g.
   *  the wider session was reloaded with an inline child still around). */
  input?: string | null
  output?: string | null
  errorText?: string | null
  state?: ToolCallState
  /**
   * ACP extensibility metadata on this tool call. Read as a tertiary
   * fallback after the live `DelegationContext` binding when the parent UI
   * re-mounted on a page refresh and the live `delegation_started` event was
   * already consumed (lost): the snapshot's
   * `ToolCallState.meta["codeg.delegation"]` carries enough to re-bind the
   * card to the child conversation.
   */
  meta?: Record<string, unknown> | null
}

export function DelegatedSubThread({
  parentToolUseId,
  input,
  output,
  errorText,
  state,
  meta,
}: Props) {
  const t = useTranslations("Folder.chat.delegation")
  const [dialogOpen, setDialogOpen] = useState(false)
  const {
    agentType,
    task,
    taskId,
    status,
    errorCode,
    childConversationId,
    childConnectionId,
    hasModel,
  } = useDelegationCardModel({
    parentToolUseId,
    input,
    output,
    errorText,
    state,
    meta,
  })

  // A snapshot replay with an empty/unparseable input AND no live binding has
  // no useful card to draw — fall through to the standard renderer instead of
  // an "unknown sub-agent" stub. Placed AFTER all hooks so hook order is stable.
  if (!hasModel) {
    return null
  }

  return (
    <div
      data-testid="delegated-sub-thread"
      className="@container/delegcard rounded-lg border border-border bg-card"
    >
      <div className="flex w-full items-stretch rounded-lg overflow-hidden">
        <div className="flex flex-1 min-w-0 items-center gap-3 px-3 py-2.5 text-left">
          <span className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-border bg-background text-foreground">
            {agentType ? (
              <AgentIcon agentType={agentType} className="h-5 w-5" />
            ) : (
              <span className="h-2.5 w-2.5 rounded-sm bg-muted-foreground/60" />
            )}
          </span>
          <div className="min-w-0 flex-1 space-y-0.5">
            <div className="flex items-center gap-2">
              <span className="text-sm font-semibold text-foreground">
                {agentType ? AGENT_LABELS[agentType] : t("unknownAgent")}
              </span>
              {taskId && (
                <span
                  className="shrink-0 font-mono text-xs text-muted-foreground"
                  title={taskId}
                >
                  #{taskId.slice(0, 8)}
                </span>
              )}
              <StatusBadge status={status} errorCode={errorCode} />
            </div>
            {task && (
              <div className="text-xs text-muted-foreground whitespace-pre-wrap break-words line-clamp-1">
                {task}
              </div>
            )}
          </div>
        </div>
        {childConversationId != null && (
          <button
            type="button"
            onClick={() => setDialogOpen(true)}
            className="shrink-0 flex items-center gap-1.5 px-3 border-l border-border text-xs font-medium text-foreground/80 hover:bg-muted/60 hover:text-foreground transition-colors"
            title={t("openDetail")}
            aria-label={t("openDetail")}
          >
            <Eye className="h-3.5 w-3.5" />
            <span className="hidden @[24rem]/delegcard:inline">
              {t("openDetail")}
            </span>
          </button>
        )}
      </div>
      {childConversationId != null && (
        <SubAgentSessionDialog
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          childConversationId={childConversationId}
          childConnectionId={childConnectionId}
          agentType={agentType}
          kickoffTask={task}
        />
      )}
    </div>
  )
}
