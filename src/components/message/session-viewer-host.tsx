"use client"

/**
 * Owns the "查看会话" drawers for one transcript, ABOVE the virtual list.
 *
 * The cards that offer those viewers — `DelegatedSubThread` for a
 * `delegate_to_agent` call, `AgentToolCallPart` for a grok `spawn_subagent` —
 * render inside `VirtualizedMessageThread`'s rows. They used to hold the
 * viewer's open state and render the drawer themselves, which meant scrolling
 * the card out of virtua's buffer unmounted the row and took the open drawer
 * down with it. Hoisting the state and the drawer to `MessageListView` (which
 * renders the virtualizer, not a row inside it) is what decouples the two.
 *
 * Nesting is unaffected: the viewer's own transcript renders another
 * `MessageListView`, which provides its own host, so a grandchild viewer still
 * mounts inside the parent drawer's React tree and Base UI still stacks it.
 *
 * ONE request slot, not one per kind. Two viewers open at the same level would
 * be same-width siblings with no stacking relationship between them — one
 * flatly covering the other, which reads as a glitch. Opening a second viewer
 * therefore replaces the first.
 */

import * as React from "react"

import { SubAgentSessionDialog } from "@/components/message/sub-agent-session-dialog"
import { SubagentSessionDialog } from "@/components/message/subagent-session-dialog"
import {
  useDelegationCardModel,
  type DelegationCardSource,
} from "@/hooks/use-delegation-card-model"
import type { AgentType } from "@/lib/types"

/** A sub-agent delegated with `delegate_to_agent`, viewed through its child
 *  conversation. Carries the card's raw SOURCE rather than the resolved ids —
 *  see `DelegationViewer` below. */
interface DelegationRequest {
  kind: "delegation"
  source: DelegationCardSource
}

/** A sub-agent that ran as a standalone session of its own (grok
 *  `spawn_subagent`), viewed by re-reading its transcript from disk. */
interface AgentSessionRequest {
  kind: "agentSession"
  sessionId: string
  agentType: AgentType
  subagentType?: string | null
  description?: string | null
  /**
   * Whether to keep re-reading the child's transcript.
   *
   * A SNAPSHOT, taken when the card opened the viewer, and knowingly so: it is
   * derived from the tool call's state inside the card, and the card is
   * exactly what this host exists to stop depending on. The cost of it going
   * stale is bounded and one-directional — a child that finishes after the
   * card scrolls away keeps being re-read every couple of seconds until the
   * user closes the drawer, which is a no-op parse of a file that stopped
   * changing. The delegation branch has no equivalent problem (below).
   */
  live: boolean
}

export type SessionViewerRequest = DelegationRequest | AgentSessionRequest

interface SessionViewerHostValue {
  open: (request: SessionViewerRequest) => void
}

const SessionViewerHostContext =
  React.createContext<SessionViewerHostValue | null>(null)

/**
 * The host for the current transcript, or `null` when there is none.
 *
 * Null is a supported answer, not a failure: `ContentPartsRenderer` also
 * renders outside any `MessageListView` (the grok child transcript in
 * `subagent-session-dialog.tsx` renders parts directly), and those surfaces
 * are not virtualized, so a card there can keep owning its own viewer.
 */
export function useSessionViewerHost(): SessionViewerHostValue | null {
  return React.useContext(SessionViewerHostContext)
}

export function SessionViewerHost({ children }: { children: React.ReactNode }) {
  const [request, setRequest] = React.useState<SessionViewerRequest | null>(
    null
  )
  const [open, setOpen] = React.useState(false)

  const value = React.useMemo<SessionViewerHostValue>(
    () => ({
      open: (next) => {
        setRequest(next)
        setOpen(true)
      },
    }),
    []
  )

  // Closing only lowers the flag; the request stays so the drawer has content
  // to draw through its exit transition. Harmless afterwards — both viewers
  // gate their body (and their fetches) on `open`.
  return (
    <SessionViewerHostContext.Provider value={value}>
      {children}
      {request?.kind === "delegation" && (
        <DelegationViewer
          // A different delegation is a different child conversation, and the
          // viewer's whole live bridge is keyed to that. Remount rather than
          // re-point.
          key={request.source.parentToolUseId}
          source={request.source}
          open={open}
          onOpenChange={setOpen}
        />
      )}
      {request?.kind === "agentSession" && (
        <SubagentSessionDialog
          key={request.sessionId}
          open={open}
          onOpenChange={setOpen}
          sessionId={request.sessionId}
          agentType={request.agentType}
          subagentType={request.subagentType}
          description={request.description}
          live={request.live}
        />
      )}
    </SessionViewerHostContext.Provider>
  )
}

/**
 * Re-derives the delegation's live model here rather than taking the card's
 * word for it.
 *
 * `DelegationCardSource` is nothing but the tool call's own serializable
 * fields, and `useDelegationCardModel` turns those plus the live connection /
 * binding stores into the agent type, status and child ids. Running it here
 * means the viewer keeps tracking the child — a late binding, a reconnect that
 * moves `childConnectionId` — long after the card that opened it was scrolled
 * away and unmounted. A snapshot of the resolved ids would have frozen at
 * whatever was known at click time.
 */
function DelegationViewer({
  source,
  open,
  onOpenChange,
}: {
  source: DelegationCardSource
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { agentType, task, childConversationId, childConnectionId } =
    useDelegationCardModel(source)

  if (childConversationId == null) return null

  return (
    <SubAgentSessionDialog
      open={open}
      onOpenChange={onOpenChange}
      childConversationId={childConversationId}
      childConnectionId={childConnectionId}
      agentType={agentType}
      kickoffTask={task}
    />
  )
}
