import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { SubAgentSessionDialog } from "./sub-agent-session-dialog"
import enMessages from "@/i18n/messages/en.json"
import type { ConnectionState } from "@/contexts/acp-connections-context"

// Runtime context — record dispatch calls so we can assert the bridge
// runs at the right moments without booting the real reducer.
const mockSetLiveMessage = vi.fn()
const mockCompleteTurn = vi.fn()
const mockRemoveConversation = vi.fn()
const mockFetchDetail = vi.fn()
const mockRefetchDetail = vi.fn()
const mockSetLiveOwnsActiveTurn = vi.fn()
const mockGetSession = vi.fn()
const mockGetTimelineTurns = vi.fn(() => [])
const mockRespondPermission = vi.fn()
// syncTurnMetadata returns a cancel function; hand back a spy so tests can
// assert both that the backfill is kicked off and that it's cancelled on close.
const mockSyncCancel = vi.fn()
const mockSyncTurnMetadata = vi.fn(() => mockSyncCancel)

vi.mock("@/contexts/conversation-runtime-context", async () => {
  const actual = await vi.importActual<
    typeof import("@/contexts/conversation-runtime-context")
  >("@/contexts/conversation-runtime-context")
  return {
    ...actual,
    useConversationRuntime: () => ({
      setLiveMessage: mockSetLiveMessage,
      completeTurn: mockCompleteTurn,
      removeConversation: mockRemoveConversation,
      fetchDetail: mockFetchDetail,
      refetchDetail: mockRefetchDetail,
      setLiveOwnsActiveTurn: mockSetLiveOwnsActiveTurn,
      getSession: mockGetSession,
      getTimelineTurns: mockGetTimelineTurns,
      syncTurnMetadata: mockSyncTurnMetadata,
      // Members that the body / list view may call but the bridge doesn't.
      appendOptimisticTurn: vi.fn(),
      setExternalId: vi.fn(),
      setSyncState: vi.fn(),
      setPendingCleanup: vi.fn(),
      setAcpLoadError: vi.fn(),
      getConversationIdByExternalId: vi.fn(),
    }),
  }
})

// Connection store — drives the child connection state subscription used
// by the bridge. Mutating `mockChildConnection` + calling `notifyStore()`
// simulates a STATE update from the connections reducer.
let mockChildConnection: ConnectionState | undefined = undefined
let storeCallbacks: Array<() => void> = []
function notifyStore() {
  for (const cb of storeCallbacks) cb()
}

vi.mock("@/contexts/acp-connections-context", async () => {
  const actual = await vi.importActual<
    typeof import("@/contexts/acp-connections-context")
  >("@/contexts/acp-connections-context")
  return {
    ...actual,
    useConnectionStore: () => ({
      subscribeKey: (_key: string, cb: () => void) => {
        storeCallbacks.push(cb)
        return () => {
          storeCallbacks = storeCallbacks.filter((c) => c !== cb)
        }
      },
      getConnection: () => mockChildConnection,
      getActiveKey: () => null,
      subscribeActiveKey: () => () => {},
    }),
    useAcpActions: () => ({ respondPermission: mockRespondPermission }),
  }
})

// PermissionDialog has its own dependency graph (parsePermissionToolCall,
// CodeBlock, UnifiedDiffPreview…). Stub it to a sentinel button that forwards
// the response so we can assert the dialog surfaces + routes the child's prompt.
vi.mock("@/components/chat/permission-dialog", () => ({
  PermissionDialog: ({
    permission,
    onRespond,
  }: {
    permission: { request_id: string } | null
    onRespond: (requestId: string, optionId: string) => void
  }) =>
    permission ? (
      <button
        data-testid="permission-dialog"
        onClick={() => onRespond(permission.request_id, "approve")}
      >
        permission for {permission.request_id}
      </button>
    ) : null,
}))

// useConversationDetail drives the persisted-detail fetch. We don't need
// to exercise the real fetch — just expose a controlled `loading` flag so
// tests can step through the detail-load lifecycle.
let mockDetailState: {
  detail: null
  loading: boolean
  error: string | null
  acpLoadError: string | null
} = {
  detail: null,
  loading: false,
  error: null,
  acpLoadError: null,
}
vi.mock("@/hooks/use-conversation-detail", () => ({
  useConversationDetail: () => mockDetailState,
}))

// MessageListView pulls in the full runtime provider + virtualization
// stack. Stub it to a sentinel that records the props we care about,
// so the read-only-mode test can assert that no `onReload`/`onNewSession`/
// `sendSignal` are wired in.
vi.mock("@/components/message/message-list-view", () => ({
  MessageListView: (props: Record<string, unknown>) => (
    <div
      data-testid="message-list-view"
      data-conversation-id={String(props.conversationId)}
      data-is-active={String(props.isActive)}
      data-has-on-reload={String(props.onReload !== undefined)}
      data-has-on-new-session={String(props.onNewSession !== undefined)}
      data-has-send-signal={String(props.sendSignal !== undefined)}
      data-conn-status={
        props.connStatus === null || props.connStatus === undefined
          ? "null"
          : String(props.connStatus)
      }
    />
  ),
}))

function renderWithIntl(ui: React.ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

function makeConnState(overrides: Partial<ConnectionState>): ConnectionState {
  return {
    connectionId: "c1",
    contextKey: "ck1",
    agentType: "codex",
    workingDir: null,
    status: "connected",
    promptCapabilities: { image: false, audio: false, embedded_context: false },
    supportsFork: false,
    selectorsReady: true,
    sessionId: null,
    modes: null,
    configOptions: null,
    availableCommands: null,
    usage: null,
    liveMessage: null,
    pendingPermission: null,
    pendingQuestion: null,
    claudeApiRetry: null,
    error: null,
    loadError: null,
    lastAppliedSeq: 0,
    isDelegationChild: true,
    parentToolUseId: "pt-1",
    parentConnectionId: "p1",
    isViewer: false,
    pendingUserMessage: null,
    ...overrides,
  }
}

describe("SubAgentSessionDialog", () => {
  beforeEach(() => {
    mockSetLiveMessage.mockReset()
    mockCompleteTurn.mockReset()
    mockRemoveConversation.mockReset()
    mockFetchDetail.mockReset()
    mockRefetchDetail.mockReset()
    mockSetLiveOwnsActiveTurn.mockReset()
    mockGetSession.mockReset()
    mockGetTimelineTurns.mockClear()
    mockRespondPermission.mockReset()
    mockSyncCancel.mockReset()
    mockSyncTurnMetadata.mockClear()
    mockSyncTurnMetadata.mockReturnValue(mockSyncCancel)
    mockChildConnection = undefined
    storeCallbacks = []
    mockDetailState = {
      detail: null,
      loading: false,
      error: null,
      acpLoadError: null,
    }
  })

  it("renders nothing while closed — the body and bridge stay dormant", () => {
    renderWithIntl(
      <SubAgentSessionDialog
        open={false}
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    expect(screen.queryByTestId("message-list-view")).not.toBeInTheDocument()
    expect(mockSetLiveMessage).not.toHaveBeenCalled()
    expect(mockRemoveConversation).not.toHaveBeenCalled()
  })

  it("surfaces the child's pending permission and routes the response through the child connection id", () => {
    mockChildConnection = makeConnState({
      pendingPermission: {
        request_id: "req-7",
        tool_call: { title: "Run bash", kind: "execute" },
        options: [{ optionId: "approve", name: "Approve", kind: "allow_once" }],
      } as unknown as ConnectionState["pendingPermission"],
    })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    const dialog = screen.getByTestId("permission-dialog")
    expect(dialog).toHaveTextContent("permission for req-7")
    fireEvent.click(dialog)
    // Routed via the CHILD connection id (c1), not the parent.
    expect(mockRespondPermission).toHaveBeenCalledWith("c1", "req-7", "approve")
  })

  it("renders no permission dialog when the child has no pending permission", () => {
    mockChildConnection = makeConnState({ pendingPermission: null })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    expect(screen.queryByTestId("permission-dialog")).not.toBeInTheDocument()
  })

  it("renders a strictly read-only MessageListView (no input/send/reload props)", () => {
    mockChildConnection = makeConnState({ status: "connected" })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    const list = screen.getByTestId("message-list-view")
    // The body must not expose any user-driven entry point — no onReload,
    // no onNewSession, no sendSignal. The conversation panel uses these
    // to wire the input bar; their absence is the contract.
    expect(list).toHaveAttribute("data-has-on-reload", "false")
    expect(list).toHaveAttribute("data-has-on-new-session", "false")
    expect(list).toHaveAttribute("data-has-send-signal", "false")
    // isActive=false suppresses session-stats side effects on the active panel.
    expect(list).toHaveAttribute("data-is-active", "false")
    expect(list).toHaveAttribute("data-conversation-id", "99")
  })

  it("bridges conn.liveMessage to setLiveMessage while open and clears the runtime session on close", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    mockChildConnection = makeConnState({
      status: "prompting",
      liveMessage,
    })
    const { unmount } = renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // First mount forwards the current liveMessage with isLive=true so the
    // SET_LIVE_MESSAGE guard at acp-connections doesn't reject an active stream.
    expect(mockSetLiveMessage).toHaveBeenCalledWith(99, liveMessage, true)

    // Closing the dialog (body unmount) must wipe the entire runtime session
    // so a later reopen starts from a fresh fetchDetail — otherwise a
    // close-mid-stream / reopen-after-complete leaks stale state.
    unmount()
    expect(mockRemoveConversation).toHaveBeenCalledWith(99)
  })

  it("marks the session as liveOwnsActiveTurn on open so getTimelineTurns filters persisted reply turns while a live reply is present", () => {
    mockChildConnection = makeConnState({ status: "prompting" })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // The dialog always marks the session so the render-time projection can
    // suppress the persisted copy of the reply while a live/local reply exists.
    // No kickoffTask prop → null kickoff text.
    expect(mockSetLiveOwnsActiveTurn).toHaveBeenCalledWith(99, true, null)
  })

  it("forwards the kickoff task text so the user turn can be synthesized before the transcript lands", () => {
    mockChildConnection = makeConnState({ status: "prompting" })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
        kickoffTask="check the failing tests"
      />
    )
    // The card's known task is handed to the runtime so getTimelineTurns can
    // show the kickoff immediately, independent of the async JSONL parse.
    expect(mockSetLiveOwnsActiveTurn).toHaveBeenCalledWith(
      99,
      true,
      "check the failing tests"
    )
  })

  it("fetches on open with preserveLive:true so the bridged live reply survives the detail load", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    mockChildConnection = makeConnState({
      status: "prompting",
      liveMessage,
    })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // The mount-time fetch must use preserveLive:true so an in-flight or
    // already-bridged live reply is not wiped when the detail lands.
    expect(mockRefetchDetail).toHaveBeenCalledWith(99, { preserveLive: true })
    // The live stream is still bridged for display, with isLive=true so the
    // SET_LIVE_MESSAGE guard accepts the active stream.
    expect(mockSetLiveMessage).toHaveBeenCalledWith(99, liveMessage, true)
  })

  it("does not refetch on the streaming → settled edge — the promoted local reply is kept, never replaced from the lagging DB", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    mockChildConnection = makeConnState({ status: "prompting", liveMessage })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // Ignore the mount-time preserveLive fetch; this asserts the settle edge.
    mockRefetchDetail.mockClear()

    // Child finishes the turn → status drops to connected. There must be NO
    // settle-edge refetch: a plain load would wipe the just-promoted local
    // reply, and the DB transcript may still lag (Codex Important #2). The
    // reply is owned by completeTurn's promoted localTurn instead.
    mockChildConnection = makeConnState({ status: "connected", liveMessage })
    act(() => {
      notifyStore()
    })
    expect(mockRefetchDetail).not.toHaveBeenCalled()
  })

  it("dispatches completeTurn on prompting → connected transition (turn promotion)", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    mockChildConnection = makeConnState({
      status: "prompting",
      liveMessage,
    })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    expect(mockCompleteTurn).not.toHaveBeenCalled()

    // Status transitions: child finished the turn, dropped back to connected.
    mockChildConnection = makeConnState({
      status: "connected",
      liveMessage,
    })
    act(() => {
      notifyStore()
    })
    expect(mockCompleteTurn).toHaveBeenCalledWith(99, liveMessage)
  })

  it("adopts a retained final reply when reopened onto an already-settled child (reopen-after-completion DB lag)", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    // Child already finished while the dialog was closed: status is settled
    // (connected) but the connection still carries the final liveMessage for
    // its post-completion grace window. There is no streaming→settled edge to
    // promote it, and the persisted transcript may still lag.
    mockChildConnection = makeConnState({ status: "connected", liveMessage })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // The retained reply is bridged as live (bypassing the reconnect-replay
    // guard, since a one-shot child's liveMessage is unambiguously its reply)
    // and promoted to a completed local turn so it survives the DB lag.
    expect(mockSetLiveMessage).toHaveBeenCalledWith(99, liveMessage, true)
    expect(mockCompleteTurn).toHaveBeenCalledWith(99, liveMessage)
  })

  it("does not adopt-promote when the settled child has no retained reply", () => {
    // Settled child, no liveMessage → nothing to adopt; the DB transcript is
    // authoritative (cold open of a fully-detached child).
    mockChildConnection = makeConnState({
      status: "connected",
      liveMessage: null,
    })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    expect(mockCompleteTurn).not.toHaveBeenCalled()
  })

  it("kicks off syncTurnMetadata after the streaming → settled transition so the reply's token stats backfill", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    mockChildConnection = makeConnState({ status: "prompting", liveMessage })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // No backfill while still streaming — only after the turn settles.
    expect(mockSyncTurnMetadata).not.toHaveBeenCalled()

    mockChildConnection = makeConnState({ status: "connected", liveMessage })
    act(() => {
      notifyStore()
    })
    // completeTurn promotes the reply WITHOUT usage/duration/model (those come
    // from the DB parser); syncTurnMetadata is the delayed roundtrip that
    // patches them in so the post-stream stats row fills.
    expect(mockSyncTurnMetadata).toHaveBeenCalledWith(99)
  })

  it("kicks off syncTurnMetadata when adopting a retained reply on reopen-after-completion", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    // Reopened onto an already-settled child still holding its final reply.
    mockChildConnection = makeConnState({ status: "connected", liveMessage })
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // The adopt path promotes the retained reply, so its stats must backfill too.
    expect(mockSyncTurnMetadata).toHaveBeenCalledWith(99)
  })

  it("cancels the in-flight metadata sync when the dialog closes", () => {
    const liveMessage = {
      id: "live-1",
      role: "assistant" as const,
      content: [],
      startedAt: Date.now(),
    }
    mockChildConnection = makeConnState({ status: "prompting", liveMessage })
    const { unmount } = renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    mockChildConnection = makeConnState({ status: "connected", liveMessage })
    act(() => {
      notifyStore()
    })
    // Sync started → its cancel handle must run on close so a late DB roundtrip
    // can't patch a session that's been torn down.
    expect(mockSyncCancel).not.toHaveBeenCalled()
    unmount()
    expect(mockSyncCancel).toHaveBeenCalled()
  })

  it("does not call setLiveMessage while the dialog is closed", () => {
    mockChildConnection = makeConnState({
      status: "prompting",
      liveMessage: {
        id: "live-1",
        role: "assistant",
        content: [],
        startedAt: Date.now(),
      },
    })
    renderWithIntl(
      <SubAgentSessionDialog
        open={false}
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    expect(mockSetLiveMessage).not.toHaveBeenCalled()
    expect(mockCompleteTurn).not.toHaveBeenCalled()
  })

  it("does not duplicate the task body or a 'Read-only' badge in the dialog header — the outer card already shows them", () => {
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    expect(
      screen.queryByText("check the failing tests")
    ).not.toBeInTheDocument()
    expect(screen.queryByText("Read-only")).not.toBeInTheDocument()
  })

  it("forces a fresh refetchDetail on every settled open so an in-flight fetch from a previous (now-closed) dialog can't surface stale state", () => {
    // Child is idle (undefined connection ⇒ not "prompting"), so the gated
    // fetch effect runs. First open: the body mounts, refetchDetail must fire
    // even though no session exists yet.
    const props = {
      open: true,
      onOpenChange: () => {},
      childConversationId: 99,
      childConnectionId: "c1",
      agentType: "codex" as const,
    }
    const { unmount } = renderWithIntl(<SubAgentSessionDialog {...props} />)
    expect(mockRefetchDetail).toHaveBeenCalledWith(99, { preserveLive: true })
    const firstCallCount = mockRefetchDetail.mock.calls.length

    // Close the dialog BEFORE any fetchDetail / refetchDetail response has
    // resolved. The cleanup wipes the runtime session via
    // removeConversation, but the in-flight fetch is not cancelled — its
    // later success would resurrect the session with stale detail.
    unmount()
    expect(mockRemoveConversation).toHaveBeenCalledWith(99)

    // Second open: body re-mounts. refetchDetail MUST fire again so the
    // resurrected stale session (if any) is overwritten with the latest DB
    // state. The dialog disables useConversationDetail's auto-fetch, so this
    // gated refetch is the sole fetch path.
    renderWithIntl(<SubAgentSessionDialog {...props} />)
    expect(mockRefetchDetail.mock.calls.length).toBeGreaterThan(firstCallCount)
    expect(mockRefetchDetail).toHaveBeenLastCalledWith(99, {
      preserveLive: true,
    })
  })

  it("invokes onOpenChange when the user closes the dialog via the close button", () => {
    const onOpenChange = vi.fn()
    renderWithIntl(
      <SubAgentSessionDialog
        open
        onOpenChange={onOpenChange}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // Radix Dialog's built-in close button is rendered with an accessible
    // "Close" label; clicking it should drive onOpenChange(false).
    const closeButton = screen.getByRole("button", { name: /close/i })
    fireEvent.click(closeButton)
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })
})
