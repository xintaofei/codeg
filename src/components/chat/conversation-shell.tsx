import { useMemo, type ReactNode } from "react"
import { useTranslations } from "next-intl"
import type {
  AgentType,
  ConnectionStatus,
  PromptCapabilitiesInfo,
  PromptDraft,
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

interface ConversationShellProps {
  status: ConnectionStatus | null
  promptCapabilities: PromptCapabilitiesInfo
  defaultPath?: string
  agentName?: string
  error: string | null
  claudeApiRetry: ClaudeApiRetryState | null
  pendingPermission: PendingPermission | null
  pendingQuestion: PendingQuestion | null
  onFocus: () => void
  onSend: (draft: PromptDraft, modeId?: string | null) => void
  onCancel: () => void
  onRespondPermission: (requestId: string, optionId: string) => void
  onAnswerQuestion: (answer: string) => void
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
  queue?: QueuedMessage[]
  onEnqueue?: (draft: PromptDraft, modeId: string | null) => void
  onQueueReorder?: (items: QueuedMessage[]) => void
  onQueueEdit?: (id: string) => void
  onQueueDelete?: (id: string) => void
  editingItemId?: string | null
  editingDraftText?: string | null
  isEditingQueueItem?: boolean
  onSaveQueueEdit?: (draft: PromptDraft) => void
  onCancelQueueEdit?: () => void
  onForkSend?: (draft: PromptDraft, modeId?: string | null) => void
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
  onFocus,
  onSend,
  onCancel,
  onRespondPermission,
  onAnswerQuestion,
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
  queue,
  onEnqueue,
  onQueueReorder,
  onQueueEdit,
  onQueueDelete,
  editingItemId,
  editingDraftText,
  isEditingQueueItem,
  onSaveQueueEdit,
  onCancelQueueEdit,
  onForkSend,
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
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex-1 min-h-0">{children}</div>

      <PermissionDialog
        permission={pendingPermission}
        onRespond={onRespondPermission}
      />

      <QuestionDialog question={pendingQuestion} onAnswer={onAnswerQuestion} />

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
            queue={queue}
            onEnqueue={onEnqueue}
            onQueueReorder={onQueueReorder}
            onQueueEdit={onQueueEdit}
            onQueueDelete={onQueueDelete}
            editingItemId={editingItemId}
            editingDraftText={editingDraftText}
            isEditingQueueItem={isEditingQueueItem}
            onSaveQueueEdit={onSaveQueueEdit}
            onCancelQueueEdit={onCancelQueueEdit}
            onForkSend={onForkSend}
            onAddFeedback={onAddFeedback}
            feedbackAddDisabled={feedbackAddDisabled}
          />
        </div>
      )}

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
