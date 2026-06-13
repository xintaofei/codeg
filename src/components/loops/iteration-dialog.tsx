"use client"

/**
 * Viewer for a single loop iteration's agent session, opened from a `question`
 * inbox card so a person can answer the iteration's `ask_user_question` (and
 * watch it stream live).
 *
 * Mirrors `SubAgentSessionDialog`: it attaches read-only to a connection the
 * loop engine owns (via `connectAsViewer`, torn down with `disconnect` which
 * only detaches a viewer — it never kills the engine's agent) and bridges the
 * live stream into the runtime session with the shared `child-session-hooks`.
 * Unlike the sub-agent viewer it also surfaces the live `AskQuestionCard`, since
 * answering the question is the whole point of opening it. The answer flows
 * through the normal `answerQuestion` route on the iteration's connection; the
 * backend's `QuestionResolved` then clears the inbox card.
 */

import { useCallback, useEffect } from "react"
import { useTranslations } from "next-intl"

import { AgentIcon } from "@/components/agent-icon"
import { MessageListView } from "@/components/message/message-list-view"
import {
  useChildConnectionState,
  useChildLiveBridge,
} from "@/components/message/child-session-hooks"
import { AskQuestionCard } from "@/components/chat/ask-question-card"
import { PermissionDialog } from "@/components/chat/permission-dialog"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { AGENT_LABELS, type AgentType, type QuestionAnswer } from "@/lib/types"

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  conversationId: number
  connectionId: string | null
  agentType: AgentType | null
}

export function IterationDialog({
  open,
  onOpenChange,
  conversationId,
  connectionId,
  agentType,
}: Props) {
  const t = useTranslations("Loops.iteration")

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        closeButtonClassName="top-2 right-2"
        className="flex h-[85vh] w-full max-w-3xl flex-col gap-0 overflow-hidden rounded-2xl p-0 lg:max-w-4xl"
      >
        <DialogTitle className="sr-only">{t("title")}</DialogTitle>
        <DialogDescription className="sr-only">
          {t("description")}
        </DialogDescription>
        {open && conversationId > 0 ? (
          <IterationSessionBody
            conversationId={conversationId}
            connectionId={connectionId}
            agentType={agentType}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  )
}

function IterationSessionBody({
  conversationId,
  connectionId,
  agentType,
}: {
  conversationId: number
  connectionId: string | null
  agentType: AgentType | null
}) {
  const t = useTranslations("Loops.iteration")
  const { connectAsViewer, disconnect, answerQuestion, respondPermission } =
    useAcpActions()

  // Attach read-only to the engine-owned iteration connection for this dialog's
  // lifetime; detach (not disconnect-the-agent) on close. The body only mounts
  // while the dialog is open, so connectionId is stable across this effect.
  useEffect(() => {
    if (!connectionId) return
    void connectAsViewer(
      connectionId,
      connectionId,
      agentType ?? "claude_code",
      null
    )
    return () => {
      void disconnect(connectionId)
    }
  }, [connectionId, agentType, connectAsViewer, disconnect])

  const conn = useChildConnectionState(connectionId)
  const connStatus = conn?.status ?? null
  const isStreaming = connStatus === "prompting"

  const { refetchDetail, setLiveOwnsActiveTurn } = useConversationRuntime()

  // Viewer mode for this conversation: while a live connection is attached, strip
  // the persisted copy of the active reply so the stream never duplicates. With no
  // connection (a settled iteration opened from the list), there is no live reply
  // to own the turn — keep the persisted transcript whole. No kickoff text either:
  // the iteration's user turn (its briefing) is persisted.
  useEffect(() => {
    setLiveOwnsActiveTurn(conversationId, connectionId != null, null)
  }, [conversationId, connectionId, setLiveOwnsActiveTurn])

  // Single persisted-detail fetch on mount, `preserveLive: true` so the bridged
  // reply is never wiped (the projection dedups against the persisted copy).
  useEffect(() => {
    refetchDetail(conversationId, { preserveLive: true })
  }, [conversationId, refetchDetail])

  const { loading, error, acpLoadError } = useConversationDetail(
    conversationId,
    {
      enabled: false,
    }
  )
  const detailLoading = isStreaming ? false : loading

  useChildLiveBridge(conversationId, conn)

  const pendingPermission = conn?.pendingPermission ?? null
  const pendingAsk = conn?.pendingAskQuestion ?? null

  const onRespondPermission = useCallback(
    (requestId: string, optionId: string) => {
      if (!connectionId) return
      void respondPermission(connectionId, requestId, optionId)
    },
    [connectionId, respondPermission]
  )

  const onAnswerAsk = useCallback(
    (questionId: string, answer: QuestionAnswer) => {
      if (!connectionId) return
      return answerQuestion(connectionId, questionId, answer)
    },
    [connectionId, answerQuestion]
  )

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b border-border px-5 py-2.5 pr-12">
        <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border bg-background text-foreground">
          {agentType ? (
            <AgentIcon agentType={agentType} className="h-4 w-4" />
          ) : (
            <span className="h-2 w-2 rounded-sm bg-muted-foreground/60" />
          )}
        </span>
        <span className="min-w-0 flex-1 truncate text-sm font-semibold text-foreground">
          {agentType ? AGENT_LABELS[agentType] : t("title")}
        </span>
      </div>
      <div className="min-h-0 flex-1 px-4 py-3">
        <MessageListView
          conversationId={conversationId}
          agentType={agentType ?? "claude_code"}
          connStatus={connStatus}
          isActive={false}
          detailLoading={detailLoading}
          detailError={error}
          acpLoadError={acpLoadError}
          hideEmptyState={false}
          showMessageNav={false}
        />
      </div>
      {(pendingPermission ||
        (pendingAsk && pendingAsk.questions.length > 0)) && (
        <div className="max-h-[60%] shrink-0 overflow-y-auto border-t border-border px-4 py-3">
          {pendingPermission && (
            <PermissionDialog
              permission={pendingPermission}
              onRespond={onRespondPermission}
            />
          )}
          {pendingAsk && pendingAsk.questions.length > 0 && connectionId && (
            <AskQuestionCard question={pendingAsk} onAnswer={onAnswerAsk} />
          )}
        </div>
      )}
    </div>
  )
}
