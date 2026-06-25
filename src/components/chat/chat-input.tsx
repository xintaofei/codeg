"use client"

import { memo } from "react"
import { useTranslations } from "next-intl"
import type {
  AgentType,
  ConnectionStatus,
  PromptCapabilitiesInfo,
  PromptDraft,
  PromptInputBlock,
  SessionConfigOptionInfo,
  SessionModeInfo,
  AvailableCommandInfo,
} from "@/lib/types"
import type { QueuedMessage } from "@/hooks/use-message-queue"
import {
  MessageInput,
  type ComposerInjectContent,
} from "@/components/chat/message-input"
import { MessageQueueDisplay } from "@/components/chat/message-queue-display"
import { cn } from "@/lib/utils"

interface ChatInputProps {
  status: ConnectionStatus | null
  promptCapabilities: PromptCapabilitiesInfo
  defaultPath?: string
  agentName?: string
  onFocus?: () => void
  onSend: (draft: PromptDraft, modeId?: string | null) => void
  onCancel: () => void
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
  isActive?: boolean
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
  onAddFeedback?: () => void
  feedbackAddDisabled?: boolean
  /**
   * Keep the composer usable even while disconnected. Set for a folderless chat
   * draft: it has no working dir yet (so it never auto-connects), and the FIRST
   * send is precisely what lazily creates its conversation + scratch dir and
   * triggers the connection. Without this the composer would be permanently
   * disabled and the chat could never be started.
   */
  allowOfflineCompose?: boolean
  injectContent?: ComposerInjectContent | null
  onInjectConsumed?: () => void
  /** Drop the input's own horizontal padding when an ancestor already supplies
   *  the gutter (the welcome column wraps this in its own `px-4`). */
  flush?: boolean
  /** Use a taller minimum height for the composer. Set for the welcome
   *  (new-conversation) composer, which sits in a roomy empty state; active and
   *  historical conversations keep the compact default. */
  tall?: boolean
}

export const ChatInput = memo(function ChatInput({
  status,
  promptCapabilities,
  defaultPath,
  agentName,
  onFocus,
  onSend,
  onCancel,
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
  isActive,
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
  onAddFeedback,
  feedbackAddDisabled,
  allowOfflineCompose = false,
  injectContent,
  onInjectConsumed,
  flush = false,
  tall = false,
}: ChatInputProps) {
  const t = useTranslations("Folder.chat.chatInput")
  const isConnected = status === "connected"
  const isPrompting = status === "prompting"
  const isConnecting = status === "connecting"

  return (
    <div
      className={cn("pt-0 pb-1", !flush && "px-4")}
      onContextMenu={(event) => event.stopPropagation()}
    >
      {queue &&
        queue.length > 0 &&
        onQueueReorder &&
        onQueueEdit &&
        onQueueDelete && (
          <MessageQueueDisplay
            queue={queue}
            onReorder={onQueueReorder}
            onEdit={onQueueEdit}
            onDelete={onQueueDelete}
            editingItemId={editingItemId ?? null}
          />
        )}
      <MessageInput
        onSend={onSend}
        promptCapabilities={promptCapabilities}
        onFocus={onFocus}
        defaultPath={defaultPath}
        disabled={
          allowOfflineCompose
            ? false
            : (!isConnected && !isPrompting) || selectorsLoading
        }
        isPrompting={isPrompting}
        onCancel={onCancel}
        modes={modes}
        configOptions={configOptions}
        modeLoading={modeLoading}
        configOptionsLoading={configOptionsLoading}
        selectedModeId={selectedModeId}
        onModeChange={onModeChange}
        onConfigOptionChange={onConfigOptionChange}
        agentType={agentType}
        availableCommands={availableCommands}
        attachmentTabId={attachmentTabId}
        draftStorageKey={draftStorageKey}
        isActive={isActive}
        onEnqueue={onEnqueue}
        editingItemId={editingItemId}
        editingDraftText={editingDraftText}
        editingDraftBlocks={editingDraftBlocks}
        isEditingQueueItem={isEditingQueueItem}
        onSaveQueueEdit={onSaveQueueEdit}
        onCancelQueueEdit={onCancelQueueEdit}
        onForkSend={onForkSend}
        onAddFeedback={onAddFeedback}
        feedbackAddDisabled={feedbackAddDisabled}
        injectContent={injectContent}
        onInjectConsumed={onInjectConsumed}
        placeholder={
          isConnecting
            ? t("connecting")
            : isPrompting
              ? t("agentResponding", { agent: agentName ?? "Agent" })
              : t("sendMessage")
        }
        className={cn(tall ? "min-h-30" : "min-h-24", "max-h-60")}
      />
    </div>
  )
})

ChatInput.displayName = "ChatInput"
