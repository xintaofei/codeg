import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { SubAgentSessionSheet } from "./sub-agent-session-sheet"
import enMessages from "@/i18n/messages/en.json"
import type { ConnectionState } from "@/contexts/acp-connections-context"

// Runtime context — record dispatch calls so we can assert the bridge
// runs at the right moments without booting the real reducer.
const mockSetLiveMessage = vi.fn()
const mockCompleteTurn = vi.fn()
const mockRemoveConversation = vi.fn()
const mockFetchDetail = vi.fn()
const mockRefetchDetail = vi.fn()
const mockGetSession = vi.fn()
const mockGetTimelineTurns = vi.fn(() => [])
const mockRespondPermission = vi.fn()

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
      getSession: mockGetSession,
      getTimelineTurns: mockGetTimelineTurns,
      // Members that the body / list view may call but the bridge doesn't.
      syncTurnMetadata: vi.fn(),
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
// the response so we can assert the sheet surfaces + routes the child's prompt.
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
    ...overrides,
  }
}

describe("SubAgentSessionSheet", () => {
  beforeEach(() => {
    mockSetLiveMessage.mockReset()
    mockCompleteTurn.mockReset()
    mockRemoveConversation.mockReset()
    mockFetchDetail.mockReset()
    mockRefetchDetail.mockReset()
    mockGetSession.mockReset()
    mockGetTimelineTurns.mockClear()
    mockRespondPermission.mockReset()
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
      <SubAgentSessionSheet
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
      <SubAgentSessionSheet
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
      <SubAgentSessionSheet
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
      <SubAgentSessionSheet
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
      <SubAgentSessionSheet
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

    // Closing the sheet (body unmount) must wipe the entire runtime session
    // so a later reopen starts from a fresh fetchDetail — otherwise a
    // close-mid-stream / reopen-after-complete leaks stale state.
    unmount()
    expect(mockRemoveConversation).toHaveBeenCalledWith(99)
  })

  it("re-bridges liveMessage after detail loading transitions true → false (recovers from FETCH_DETAIL_SUCCESS wipe)", () => {
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
    mockDetailState = {
      detail: null,
      loading: true,
      error: null,
      acpLoadError: null,
    }
    const { rerender } = renderWithIntl(
      <SubAgentSessionSheet
        open
        onOpenChange={() => {}}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    expect(mockSetLiveMessage).toHaveBeenCalledTimes(1)
    expect(mockSetLiveMessage).toHaveBeenNthCalledWith(1, 99, liveMessage, true)

    // Simulate FETCH_DETAIL_SUCCESS landing — loading flips to false. The
    // re-bridge effect must re-dispatch setLiveMessage so the in-flight
    // stream survives the reducer's `liveMessage: null` write.
    mockDetailState = {
      detail: null,
      loading: false,
      error: null,
      acpLoadError: null,
    }
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <SubAgentSessionSheet
          open
          onOpenChange={() => {}}
          childConversationId={99}
          childConnectionId="c1"
          agentType="codex"
        />
      </NextIntlClientProvider>
    )
    // At least one additional setLiveMessage(99, liveMessage, true) must have
    // fired — recovers from the detail-load wipe.
    const calls = mockSetLiveMessage.mock.calls.filter(
      ([cid, lm, isLive]) => cid === 99 && lm === liveMessage && isLive === true
    )
    expect(calls.length).toBeGreaterThanOrEqual(2)
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
      <SubAgentSessionSheet
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

  it("does not call setLiveMessage while the sheet is closed", () => {
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
      <SubAgentSessionSheet
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

  it("does not duplicate the task body or a 'Read-only' badge in the sheet header — the outer card already shows them", () => {
    renderWithIntl(
      <SubAgentSessionSheet
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

  it("forces a fresh refetchDetail on every open so an in-flight fetch from a previous (now-closed) sheet can't surface stale state", () => {
    // First open: the body mounts, refetchDetail must fire even though no
    // session exists yet.
    const props = {
      open: true,
      onOpenChange: () => {},
      childConversationId: 99,
      childConnectionId: "c1",
      agentType: "codex" as const,
    }
    const { unmount } = renderWithIntl(<SubAgentSessionSheet {...props} />)
    expect(mockRefetchDetail).toHaveBeenCalledWith(99)
    const firstCallCount = mockRefetchDetail.mock.calls.length

    // Close the sheet BEFORE any fetchDetail / refetchDetail response has
    // resolved. The cleanup wipes the runtime session via
    // removeConversation, but the in-flight fetch is not cancelled — its
    // later success would resurrect the session with stale detail.
    unmount()
    expect(mockRemoveConversation).toHaveBeenCalledWith(99)

    // Second open: body re-mounts. refetchDetail MUST fire again so the
    // resurrected stale session (if any) is overwritten with the latest
    // DB state. Without this, useConversationDetail's auto-fetch would
    // skip on the session.detail active-data guard.
    renderWithIntl(<SubAgentSessionSheet {...props} />)
    expect(mockRefetchDetail.mock.calls.length).toBeGreaterThan(firstCallCount)
    expect(mockRefetchDetail).toHaveBeenLastCalledWith(99)
  })

  it("invokes onOpenChange when the user closes the sheet via the close button", () => {
    const onOpenChange = vi.fn()
    renderWithIntl(
      <SubAgentSessionSheet
        open
        onOpenChange={onOpenChange}
        childConversationId={99}
        childConnectionId="c1"
        agentType="codex"
      />
    )
    // Radix Sheet's built-in close button is rendered with an accessible
    // "Close" label; clicking it should drive onOpenChange(false).
    const closeButton = screen.getByRole("button", { name: /close/i })
    fireEvent.click(closeButton)
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })
})
