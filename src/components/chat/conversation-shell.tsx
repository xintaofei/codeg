import { useMemo, type ReactNode } from "react"
import { useTranslations } from "next-intl"
import type {
  AgentType,
  ConnectionStatus,
  PendingPlanApprovalState,
  PendingQuestionState,
  PlanApprovalAnswer,
  PromptCapabilitiesInfo,
  PromptDraft,
  PromptInputBlock,
  QuestionAnswer,
  SessionConfigOptionInfo,
  SessionModeInfo,
  AvailableCommandInfo,
} from "@/lib/types"
import type {
  PendingPermission,
  PendingQuestion,
  ClaudeApiRetryState,
} from "@/contexts/acp-connections-context"
import type { QueuedMessage } from "@/hooks/use-message-queue"
import { Loader2 } from "lucide-react"
import { ChatInput } from "@/components/chat/chat-input"
import { PermissionDialog } from "@/components/chat/permission-dialog"
import { QuestionDialog } from "@/components/chat/question-dialog"
import { AskQuestionCard } from "@/components/chat/ask-question-card"
import { PlanApprovalCard } from "@/components/chat/plan-approval-card"

interface ConversationShellProps {
  status: ConnectionStatus | null
  promptCapabilities: PromptCapabilitiesInfo
  defaultPath?: string
  agentName?: string
  error: string | null
  claudeApiRetry: ClaudeApiRetryState | null
  pendingPermission: PendingPermission | null
  pendingQuestion: PendingQuestion | null
  /** Awaiting-answer multiple-choice `ask_user_question`. */
  pendingAskQuestion: PendingQuestionState | null
  /** Awaiting-decision Grok `exit_plan_mode` approval. */
  pendingPlanApproval: PendingPlanApprovalState | null
  onFocus: () => void
  onSend: (draft: PromptDraft, modeId?: string | null) => void
  onCancel: () => void
  onRespondPermission: (requestId: string, optionId: string) => void
  onAnswerQuestion: (answer: string) => void
  onAnswerAskQuestion: (
    questionId: string,
    answer: QuestionAnswer
  ) => void | Promise<void>
  onAnswerPlanApproval: (
    approvalId: string,
    answer: PlanApprovalAnswer
  ) => void | Promise<void>
  children: ReactNode
  modes?: SessionModeInfo[]
  configOptions?: SessionConfigOptionInfo[]
  modeLoading?: boolean
  configOptionsLoading?: boolean
  selectorsLoading?: boolean
  selectedModeId?: string | null
  onModeChange?: (modeId: string) => void
  onConfigOptionChange?: (configId: string, valueId: string) => void
  agentType?: AgentType | null
  availableCommands?: AvailableCommandInfo[] | null
  attachmentTabId?: string | null
  draftStorageKey?: string | null
  hideInput?: boolean
  /** Optional read-only live-feedback notes list rendered just above the
   *  composer (see `FeedbackNotesDisplay`). Renders nothing when there are no
   *  notes for the current turn. */
  feedbackList?: ReactNode
  /** Open the live-feedback dialog from the composer "+" menu (hidden when
   *  omitted / feature off). */
  onAddFeedback?: () => void
  /** Grey out the live-feedback "+" entry when a note can't be sent right now. */
  feedbackAddDisabled?: boolean
  isActive?: boolean
  /** Show the composer's flowing active-session border (tiled multi-session
   *  active tab only). Threaded straight through to the composer. */
  showActiveFlow?: boolean
  queue?: QueuedMessage[]
  onEnqueue?: (draft: PromptDraft, modeId: string | null) => void
  onQueueReorder?: (items: QueuedMessage[]) => void
  onQueueEdit?: (id: string) => void
  onQueueDelete?: (id: string) => void
  editingItemId?: string | null
  editingDraftText?: string | null
  editingDraftBlocks?: PromptInputBlock[] | null
  isEditingQueueItem?: boolean
  onSaveQueueEdit?: (draft: PromptDraft) => void
  onCancelQueueEdit?: () => void
  onForkSend?: (draft: PromptDraft, modeId?: string | null) => void
  /** Optional banner pinned to the top of the panel, above the message area
   *  (e.g. the "restart to apply" config-stale banner). Renders nothing when
   *  omitted. */
  topBanner?: ReactNode
}

export function ConversationShell({
  status,
  promptCapabilities,
  defaultPath,
  agentName,
  error,
  claudeApiRetry,
  pendingPermission,
  pendingQuestion,
  pendingAskQuestion,
  pendingPlanApproval,
  onFocus,
  onSend,
  onCancel,
  onRespondPermission,
  onAnswerQuestion,
  onAnswerAskQuestion,
  onAnswerPlanApproval,
  children,
  modes,
  configOptions,
  modeLoading = false,
  configOptionsLoading = false,
  selectorsLoading = false,
  selectedModeId,
  onModeChange,
  onConfigOptionChange,
  agentType,
  availableCommands,
  attachmentTabId,
  draftStorageKey,
  hideInput = false,
  feedbackList,
  onAddFeedback,
  feedbackAddDisabled,
  isActive,
  showActiveFlow,
  queue,
  onEnqueue,
  onQueueReorder,
  onQueueEdit,
  onQueueDelete,
  editingItemId,
  editingDraftText,
  editingDraftBlocks,
  isEditingQueueItem,
  onSaveQueueEdit,
  onCancelQueueEdit,
  onForkSend,
  topBanner,
}: ConversationShellProps) {
  const tAcp = useTranslations("Folder.chat.acpConnections")
  const retryLineText = useMemo(() => {
    const retry = claudeApiRetry
    if (!retry) return null

    const retryAttempt =
      retry.attempt !== null && retry.attempt !== undefined
        ? Math.trunc(retry.attempt)
        : null
    const retryMax =
      retry.maxRetries !== null && retry.maxRetries !== undefined
        ? Math.trunc(retry.maxRetries)
        : null
    const retryDelaySeconds =
      retry.retryDelayMs !== null && retry.retryDelayMs !== undefined
        ? (retry.retryDelayMs / 1000).toFixed(1)
        : null
    const errorLabel = retry.error ?? tAcp("claudeApiRetry.fallbackError")
    const statusLabel =
      retry.errorStatus !== null && retry.errorStatus !== undefined
        ? tAcp("claudeApiRetry.httpStatus", {
            status: Math.trunc(retry.errorStatus),
          })
        : ""
    const retryLabel =
      retryAttempt !== null && retryMax !== null
        ? tAcp("claudeApiRetry.retryingWithMax", {
            attempt: retryAttempt,
            max: retryMax,
          })
        : retryAttempt !== null
          ? tAcp("claudeApiRetry.retryingAttempt", {
              attempt: retryAttempt,
            })
          : tAcp("claudeApiRetry.retrying")
    const delayLabel =
      retryDelaySeconds !== null
        ? tAcp("claudeApiRetry.nextRetryIn", {
            seconds: retryDelaySeconds,
          })
        : null

    return delayLabel !== null
      ? tAcp("claudeApiRetry.lineWithDelay", {
          error: errorLabel,
          status: statusLabel,
          retry: retryLabel,
          delay: delayLabel,
        })
      : tAcp("claudeApiRetry.line", {
          error: errorLabel,
          status: statusLabel,
          retry: retryLabel,
        })
  }, [claudeApiRetry, tAcp])

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      {topBanner}
      <div className="flex-1 min-h-0">{children}</div>

      <PermissionDialog
        permission={pendingPermission}
        onRespond={onRespondPermission}
      />

      <QuestionDialog question={pendingQuestion} onAnswer={onAnswerQuestion} />

      {/* Composer dock. The ask-question card sits in normal flow just above the
          feedback list and input — like the permission/question dialogs — so it
          shrinks the message list instead of covering it, while staying aligned
          to the input width. */}
      <div>
        {pendingAskQuestion && pendingAskQuestion.questions.length > 0 && (
          <div className="mx-auto w-full max-w-3xl px-4">
            <AskQuestionCard
              question={pendingAskQuestion}
              onAnswer={onAnswerAskQuestion}
            />
          </div>
        )}
        {pendingPlanApproval && (
          <div className="mx-auto w-full max-w-3xl px-4">
            {/* key on approval_id so the card always remounts (fresh in-flight /
                feedback state) if the slot is ever reused for a new approval. */}
            <PlanApprovalCard
              key={pendingPlanApproval.approval_id}
              approval={pendingPlanApproval}
              onAnswer={onAnswerPlanApproval}
            />
          </div>
        )}

        {!hideInput && feedbackList && (
          <div className="mx-auto w-full max-w-3xl px-4">{feedbackList}</div>
        )}

        {!hideInput && (
          <div className="mx-auto w-full max-w-3xl">
            <ChatInput
              status={status}
              promptCapabilities={promptCapabilities}
              defaultPath={defaultPath}
              agentName={agentName}
              onFocus={onFocus}
              onSend={onSend}
              onCancel={onCancel}
              modes={modes}
              configOptions={configOptions}
              modeLoading={modeLoading}
              configOptionsLoading={configOptionsLoading}
              selectorsLoading={selectorsLoading}
              selectedModeId={selectedModeId}
              onModeChange={onModeChange}
              onConfigOptionChange={onConfigOptionChange}
              agentType={agentType}
              availableCommands={availableCommands}
              attachmentTabId={attachmentTabId}
              draftStorageKey={draftStorageKey}
              isActive={isActive}
              showActiveFlow={showActiveFlow}
              queue={queue}
              onEnqueue={onEnqueue}
              onQueueReorder={onQueueReorder}
              onQueueEdit={onQueueEdit}
              onQueueDelete={onQueueDelete}
              editingItemId={editingItemId}
              editingDraftText={editingDraftText}
              editingDraftBlocks={editingDraftBlocks}
              isEditingQueueItem={isEditingQueueItem}
              onSaveQueueEdit={onSaveQueueEdit}
              onCancelQueueEdit={onCancelQueueEdit}
              onForkSend={onForkSend}
              onAddFeedback={onAddFeedback}
              feedbackAddDisabled={feedbackAddDisabled}
            />
          </div>
        )}
      </div>

      {retryLineText && (
        <div className="border-t border-destructive/20 bg-destructive/5 px-4 py-2 text-xs text-destructive">
          <div className="flex items-center gap-2 font-medium">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            <span className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
              {retryLineText}
            </span>
          </div>
        </div>
      )}

      {error && (
        <div className="px-4 py-2 text-xs text-destructive bg-destructive/5 border-t border-destructive/20">
          {error}
        </div>
      )}
    </div>
  )
}
