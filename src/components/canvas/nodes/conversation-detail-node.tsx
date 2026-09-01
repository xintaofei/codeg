"use client"

import { memo, useCallback } from "react"
import { NodeResizer, type Node, type NodeProps } from "@xyflow/react"
import { Minimize2, X } from "lucide-react"
import { useTranslations } from "next-intl"
import { AgentIcon } from "@/components/agent-icon"
import { ConversationStatusDot } from "@/components/conversations/conversation-status-dot"
import { formatConversationTitle } from "@/lib/conversation-title"
import type { AgentType, ConversationStatus } from "@/lib/types"
import { cn } from "@/lib/utils"
import {
  CanvasConversationSurface,
  type CanvasDraftTarget,
} from "../canvas-conversation-surface"
import {
  DRAG_HANDLE_CLASS,
  type ConversationCardData,
  type NewConversationTarget,
} from "../canvas-model"
import { ColorWash } from "../canvas-swatches"
import { useCanvasView } from "../canvas-view-context"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"

export type ConversationDetailFlowNode = Node<
  ConversationCardData,
  "conversationDetail"
>

export interface ConversationDraftData {
  draftId: string
  target: NewConversationTarget
  agentType: AgentType
  /** Set before the card has a row of its own; the send that mints the row
   *  carries it over, so the colour outlives the draft. */
  color?: string | null
  [key: string]: unknown
}

export type ConversationDraftFlowNode = Node<
  ConversationDraftData,
  "conversationDraft"
>

/**
 * Shared frame for both live-conversation cards: a titled window with a
 * resizer, sized entirely by the ReactFlow node wrapper.
 *
 * The title bar is the ONLY drag handle (`dragHandle` on the node points at
 * `DRAG_HANDLE_CLASS`), which buys the body the two things a conversation needs
 * and a canvas node normally forbids: text you can select and a composer you can
 * click into. ReactFlow's own stylesheet sets `user-select: none` and
 * `cursor: grab` on every `.react-flow__node`, so the body has to say
 * `select-text cursor-auto` out loud — the card is a window, not a tile.
 *
 * `onActivate` fires on the first interaction anywhere in the card: a card
 * restored from a previous visit renders its transcript but holds no ACP
 * connection until then.
 */
function DetailFrame({
  selected,
  title,
  icon,
  color,
  actions,
  onResizeEnd,
  onActivate,
  children,
}: {
  selected?: boolean
  title: React.ReactNode
  icon?: React.ReactNode
  /** The card's colour, same row and same wash as its collapsed form — a
   *  colour that vanished on expanding would read as having been lost. */
  color?: string | null
  actions: React.ReactNode
  onResizeEnd?: (geometry: {
    x: number
    y: number
    width: number
    height: number
  }) => void
  onActivate?: () => void
  children: React.ReactNode
}) {
  return (
    <div
      className={cn(
        // The whole conversation inside here renders in board units too (see
        // `canvas-board-units` in globals.css): a card on this board is drawn at
        // the board's scale, and the board is zoomed with its own control. The
        // menus it opens are portalled out and stay on the app's scale, which is
        // right — those are chrome, not board content.
        "canvas-board-units flex h-full w-full cursor-auto flex-col overflow-hidden rounded-2xl border bg-card transition-colors select-text",
        selected
          ? "border-primary ring-2 ring-primary/25"
          : "border-foreground/15"
      )}
      // Primary button only. Right-drag pans the board, and a pan that starts
      // over a restored card — or sweeps the pointer across several — must not
      // be read as "the user wants these conversations connected"; that would
      // spawn the very pile of agent processes dormancy exists to avoid.
      onPointerDownCapture={(e) => {
        if (e.button === 0) onActivate?.()
      }}
    >
      {/* Behind the whole window, clipped to its radius. The two rows below say
          `relative` for this: the wash is a positioned box and would otherwise
          paint over static siblings — i.e. over the entire conversation. */}
      <ColorWash color={color} className="rounded-2xl" opacity={0.08} />
      {onResizeEnd && (
        <NodeResizer
          isVisible={Boolean(selected)}
          minWidth={360}
          minHeight={320}
          lineClassName="!border-primary/40"
          handleClassName="!size-2 !rounded-sm !border-primary !bg-background"
          onResizeEnd={(_e, params) =>
            onResizeEnd({
              width: params.width,
              height: params.height,
              x: params.x,
              y: params.y,
            })
          }
        />
      )}
      <div
        className={cn(
          DRAG_HANDLE_CLASS,
          "relative flex h-9 shrink-0 cursor-grab items-center gap-1.5 border-b border-border/70 px-2.5 select-none active:cursor-grabbing"
        )}
      >
        {icon}
        <span className="min-w-0 flex-1 truncate text-[13px] font-semibold">
          {title}
        </span>
        {actions}
      </div>
      <div className="relative flex min-h-0 flex-1 flex-col">{children}</div>
    </div>
  )
}

const HEADER_BUTTON_CLASS =
  "nodrag inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"

/**
 * A pinned conversation card expanded into a live conversation: transcript,
 * composer and streaming reply, right on the board. The card owns a STABLE
 * connection key derived from its DB node id, so re-renders never re-key the
 * connection and a workspace tab on the same conversation stays a separate
 * surface that attaches to the same agent.
 */
export const ConversationDetailNode = memo(function ConversationDetailNode({
  data,
  selected,
}: NodeProps<ConversationDetailFlowNode>) {
  const t = useTranslations("Canvas")
  const {
    setCardDetail,
    endNodeResize,
    contextKeyForPin,
    liveSurfaces,
    activateSurface,
    saveSelectionAsNote,
  } = useCanvasView()
  const conversation = data.conversation
  const pinDbId = data.pinDbId
  // Bound to this card before the early return so the identity only changes
  // when the card does — `MessageListView` keeps it across renders.
  const saveAsNote = useCallback(
    (text: string) => {
      if (pinDbId != null) void saveSelectionAsNote(pinDbId, text)
    },
    [saveSelectionAsNote, pinDbId]
  )

  if (!conversation || pinDbId == null) return null

  const contextKey = contextKeyForPin(pinDbId)
  const live = liveSurfaces.has(contextKey)
  const status = conversation.status as ConversationStatus
  return (
    <DetailFrame
      selected={selected}
      color={data.color}
      onActivate={live ? undefined : () => activateSurface(contextKey)}
      icon={
        <>
          <AgentIcon
            agentType={conversation.agent_type}
            className="size-3.5 shrink-0"
          />
          <ConversationStatusDot
            status={status}
            size="sm"
            className={cn(
              status === "in_progress" && "motion-safe:animate-pulse"
            )}
          />
        </>
      }
      title={
        conversation.title
          ? formatConversationTitle(conversation.title)
          : t("untitled")
      }
      actions={
        <button
          type="button"
          className={HEADER_BUTTON_CLASS}
          aria-label={t("collapseConversation")}
          title={t("collapseConversation")}
          onClick={() => setCardDetail(pinDbId, false)}
        >
          <Minimize2 className="size-3.5" />
        </button>
      }
      onResizeEnd={(geometry) => endNodeResize(pinDbId, geometry)}
    >
      <CanvasConversationSurface
        contextKey={contextKey}
        conversationId={conversation.id}
        agentType={conversation.agent_type}
        isActive={live}
        onSaveSelectionAsNote={saveAsNote}
        // A bound conversation shows its OWN folder and can't be moved — the
        // same rule a tab follows once it has a conversation.
        folderPickerOverride={{
          folderId: conversation.folder_id,
          editable: false,
          onSelectFolder: () => {},
          onSelectChatMode: () => {},
        }}
      />
    </DetailFrame>
  )
})

/**
 * An unsent conversation. Client-local until the first message: the row, and
 * the canvas node that pins it, are both created by that send (see
 * `materializeDraft`), so an abandoned draft leaves nothing behind — the same
 * contract draft TABS have with `opened_tabs`.
 */
export const ConversationDraftNode = memo(function ConversationDraftNode({
  data,
  selected,
}: NodeProps<ConversationDraftFlowNode>) {
  const t = useTranslations("Canvas")
  const {
    dismissDraft,
    sendingDrafts,
    setDraftSending,
    setDraftAgent,
    setDraftTarget,
    materializeDraft,
    draftSurfaceKey,
    liveSurfaces,
    activateSurface,
  } = useCanvasView()
  const target = data.target
  const contextKey = draftSurfaceKey(data.draftId)
  const live = liveSurfaces.has(contextKey)
  const creating = sendingDrafts.has(data.draftId)
  const targetFolderId = "folderId" in target ? target.folderId : null
  const folderPath = useAppWorkspaceStore((s) =>
    targetFolderId != null
      ? (s.allFolders.find((f) => f.id === targetFolderId)?.path ?? null)
      : null
  )

  const draftTarget: CanvasDraftTarget =
    targetFolderId != null
      ? {
          kind: "folder",
          folderId: targetFolderId,
          workingDir: folderPath ?? "",
        }
      : { kind: "chat" }

  return (
    <DetailFrame
      selected={selected}
      color={data.color}
      onActivate={live ? undefined : () => activateSurface(contextKey)}
      icon={
        <AgentIcon agentType={data.agentType} className="size-3.5 shrink-0" />
      }
      title={t("newConversation")}
      actions={
        // Gone, not disabled, while the first send is creating the row: there
        // is nothing left to discard once the conversation exists and the
        // prompt is on its way, and a dead-looking button invites the click
        // that used to strand both.
        creating ? null : (
          <button
            type="button"
            className={HEADER_BUTTON_CLASS}
            aria-label={t("discardDraft")}
            title={t("discardDraft")}
            onClick={() => dismissDraft(data.draftId)}
          >
            <X className="size-3.5" />
          </button>
        )
      }
    >
      <CanvasConversationSurface
        contextKey={contextKey}
        conversationId={null}
        agentType={data.agentType}
        isActive={live}
        onCreatingChange={(sending) => setDraftSending(data.draftId, sending)}
        // Unsent: switching folder just re-points where the first message will
        // go, so the chip stays live right up until the send.
        folderPickerOverride={{
          folderId: targetFolderId,
          editable: true,
          onSelectFolder: (folderId) =>
            setDraftTarget(data.draftId, { folderId }),
          onSelectChatMode: () => setDraftTarget(data.draftId, { chat: true }),
        }}
        onAgentTypeChange={(agentType) =>
          setDraftAgent(data.draftId, agentType)
        }
        draftTarget={draftTarget}
        onConversationCreated={(conversationId) =>
          void materializeDraft(data.draftId, conversationId)
        }
      />
    </DetailFrame>
  )
})
