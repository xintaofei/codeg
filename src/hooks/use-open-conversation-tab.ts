"use client"

import { useCallback } from "react"

import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { useTabContext } from "@/contexts/tab-context"
import { listAllConversations } from "@/lib/api"
import type { DbConversationSummary } from "@/lib/types"

function deriveTabTitleFromRole(title: string | null | undefined) {
  // Drop common list / heading prefixes from delegated task prompts so the tab
  // reads like a responsibility label instead of a raw prompt fragment.
  return title
    ?.split(/\r?\n/)
    .map((line) => line.trim())
    .map((line) =>
      line
        .replace(/^(?:[-*+]\s+|\d+[.)]\s+|#{1,6}\s+)/, "")
        .replace(/^(?:职责|角色|任务|role|task)\s*[:：]\s*/i, "")
        .trim()
    )
    .find(Boolean)
}

export function useOpenConversationTab() {
  const { conversations, folders, addFolderToWorkspaceById } = useAppWorkspace()
  const { openTab } = useTabContext()

  return useCallback(
    async (
      conversationId: number,
      options?: {
        placement?: "end" | "afterActive"
        title?: string | null
      }
    ): Promise<DbConversationSummary> => {
      const conversation =
        conversations.find((item) => item.id === conversationId) ??
        (
          await listAllConversations({
            include_children: true,
          })
        ).find((item) => item.id === conversationId)

      if (!conversation) {
        throw new Error(`Conversation ${conversationId} not found`)
      }

      if (!folders.some((folder) => folder.id === conversation.folder_id)) {
        await addFolderToWorkspaceById(conversation.folder_id)
      }

      const roleTitle = deriveTabTitleFromRole(options?.title)
      openTab(
        conversation.folder_id,
        conversation.id,
        conversation.agent_type,
        true,
        roleTitle ?? conversation.title ?? undefined,
        {
          placement: options?.placement ?? "afterActive",
          titleOverride: roleTitle != null,
        }
      )

      return conversation
    },
    [addFolderToWorkspaceById, conversations, folders, openTab]
  )
}
