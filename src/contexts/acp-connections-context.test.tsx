import { useEffect } from "react"
import { act, render } from "@testing-library/react"
import { useTranslations } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  AcpConnectionsProvider,
  buildNotificationTitle,
  notificationSummary,
  useAcpActions,
  useConnectionStore,
} from "@/contexts/acp-connections-context"
import { parsePermissionToolCall } from "@/lib/permission-request"
import { subscribe } from "@/lib/platform"
import { saveConfigPreference } from "@/lib/selector-prefs-storage"
import type { AttachHandlers } from "@/lib/transport/types"
import type {
  EventEnvelope,
  LiveSessionSnapshot,
  SessionConfigOptionInfo,
} from "@/lib/types"

// Shared spies + a stub EventStream. `vi.hoisted` runs before the mock
// factories so they can close over this state. Mocking `getEventStream` to a
// non-null stub forces the "web / attach" transport path: the mount listener
// effect sets `listenerReadyRef` synchronously (so `waitForListenerReady` is a
// no-op) and `connectAsViewer` / the owner spawn both route through
// `stream.attach`.
const h = vi.hoisted(() => {
  const attach = vi.fn(() => ({ detach: vi.fn() }))
  const stream = { attach }
  return {
    attach,
    stream,
    // getEventStream() returns this — default the web/attach stub; set to null
    // per-test to exercise the desktop firehose path.
    eventStreamValue: stream as { attach: typeof attach } | null,
    actions: null as unknown as ReturnType<typeof useAcpActions> | null,
    store: null as unknown as ReturnType<typeof useConnectionStore> | null,
    // api spies
    acpGetAgentStatus: vi.fn(),
    acpFindConnectionForConversation: vi.fn(),
    acpConnect: vi.fn(),
    acpDisconnect: vi.fn(),
    acpGetSessionSnapshot: vi.fn(),
    buildDelegationSeedEnvelopes: vi.fn(() => []),
    denormalizeSnapshot: vi.fn(),
    // Stable across renders so tests can assert on what the error handler
    // routes to the status-bar alert vs. to the OS notification.
    pushAlert: vi.fn(),
    sendSystemNotification: vi.fn(async () => undefined),
  }
})

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn(async () => () => {}),
  getEventStream: () => h.eventStreamValue,
}))

vi.mock("@/lib/delegation-seed", () => ({
  buildDelegationSeedEnvelopes: h.buildDelegationSeedEnvelopes,
}))

vi.mock("@/contexts/alert-context", () => ({
  useAlertContext: () => ({ pushAlert: h.pushAlert }),
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: { path: "/tmp/x", name: "x" } }),
}))

vi.mock("@/lib/notification", () => ({
  sendSystemNotification: h.sendSystemNotification,
}))

vi.mock("@/lib/selector-prefs-storage", () => ({
  getSavedPrefsForConnect: () => ({ modeId: undefined, configValues: {} }),
  saveModePreference: vi.fn(),
  saveConfigPreference: vi.fn(),
}))

vi.mock("@/lib/snapshot-denormalize", () => ({
  denormalizeSnapshot: h.denormalizeSnapshot,
}))

vi.mock("@/lib/api", () => ({
  acpGetAgentStatus: h.acpGetAgentStatus,
  acpFindConnectionForConversation: h.acpFindConnectionForConversation,
  acpConnect: h.acpConnect,
  acpDisconnect: h.acpDisconnect,
  acpGetSessionSnapshot: h.acpGetSessionSnapshot,
  acpPrompt: vi.fn(),
  acpSetMode: vi.fn(),
  acpSetConfigOption: vi.fn(),
  acpCancel: vi.fn(),
  acpRespondPermission: vi.fn(),
  acpTouchConnection: vi.fn(),
  // Imported by the conversation runtime store (a real dependency of the
  // provider via the background-activity bridge). The settled path no longer
  // refetches (it flips the launch card in-memory); reject any stray call so a
  // regression that reintroduces a settle-triggered refetch fails loudly.
  getFolderConversation: vi.fn(async () => {
    throw new Error("detail not seeded in this suite")
  }),
}))

function Probe() {
  const actions = useAcpActions()
  const store = useConnectionStore()
  // Capture in an effect (not during render) so the lint rule that forbids
  // mutating external state mid-render stays happy; mountProvider flushes
  // effects before any test reads h.actions.
  useEffect(() => {
    h.actions = actions
    h.store = store
  }, [actions, store])
  return null
}

async function mountProvider() {
  render(
    <AcpConnectionsProvider>
      <Probe />
    </AcpConnectionsProvider>
  )
  await act(async () => {})
}

const TAB = "conv-1-claude_code-42"

describe("notificationSummary", () => {
  it("normalizes whitespace and bounds notification-center payloads", () => {
    expect(notificationSummary("  result\n\nready  ")).toBe("result ready")
    const summary = notificationSummary("x".repeat(400))
    expect(summary).toHaveLength(300)
    expect(summary).toMatch(/\.\.\.$/)
  })

  it("formats workspace, conversation window, and result status", () => {
    expect(
      buildNotificationTitle("Codeg 二开", "任务状态通知", "Codex 已完成")
    ).toBe("Codeg 二开 / 任务状态通知 - Codex 已完成")
  })
})

beforeEach(() => {
  h.attach.mockClear()
  h.store = null
  h.eventStreamValue = h.stream
  h.buildDelegationSeedEnvelopes.mockClear()
  h.acpGetAgentStatus.mockReset()
  h.acpFindConnectionForConversation.mockReset()
  h.acpConnect.mockReset()
  h.acpDisconnect.mockReset()
  h.acpGetSessionSnapshot.mockReset()
  h.denormalizeSnapshot.mockReset()
  h.denormalizeSnapshot.mockReturnValue({
    connectionId: "owner-conn",
    status: "connected",
    sessionId: null,
    modes: null,
    configOptions: null,
    availableCommands: null,
    usage: null,
    liveMessage: null,
    pendingPermission: null,
    pendingAskQuestion: null,
    pendingUserMessage: null,
    promptCapabilities: null,
    selectorsReady: false,
    supportsFork: false,
    configStale: false,
    configStaleKind: null,
    lastError: null,
    eventSeq: 0,
    activeDelegations: [],
  })
  // Agent is installed + available so the connect preflight passes.
  h.acpGetAgentStatus.mockResolvedValue({
    agent_type: "claude_code",
    enabled: true,
    available: true,
    installed_version: "1.0.0",
    is_acp_adapter: true,
  })
  h.acpConnect.mockResolvedValue("spawned-conn")
  h.acpDisconnect.mockResolvedValue(undefined)
  h.acpGetSessionSnapshot.mockResolvedValue(null)
})

function latestAttachHandlers(): AttachHandlers {
  const calls = h.attach.mock.calls as unknown as Array<
    [unknown, unknown, AttachHandlers]
  >
  const call = calls[calls.length - 1]
  expect(call).toBeTruthy()
  if (!call) throw new Error("expected attach handlers")
  return call[2]
}

function emitAcpEvent(handlers: AttachHandlers, envelope: EventEnvelope) {
  act(() => {
    handlers.onEvent(envelope)
  })
}

function hydrateSnapshot(
  handlers: AttachHandlers,
  snapshot: LiveSessionSnapshot
) {
  act(() => {
    handlers.onSnapshot(snapshot, snapshot.event_seq)
  })
}

describe("AcpConnectionsProvider cross-client viewer lifecycle", () => {
  it("attaches as a viewer (no spawn) when a live connection is discovered", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "owner-conn",
      event_seq: 5,
    })
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })

    // Discovery ran for the conversation (with the sessionId + agentType
    // fallback), and we attached to the owner's connection instead of spawning.
    expect(h.acpFindConnectionForConversation).toHaveBeenCalledWith(
      42,
      "sess-1",
      "claude_code"
    )
    expect(h.acpConnect).not.toHaveBeenCalled()
    // COLD attach: a viewer has applied no prior events, so it must request a
    // full snapshot (sinceSeq undefined) — NOT the discovered event_seq, which
    // could yield only a post-cursor replay and miss all earlier live state.
    expect(h.attach).toHaveBeenCalledWith(
      "owner-conn",
      { sinceSeq: undefined },
      expect.anything()
    )
  })

  it("spawns + owns when no live connection is discovered", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })

    expect(h.acpFindConnectionForConversation).toHaveBeenCalledWith(
      42,
      "sess-1",
      "claude_code"
    )
    expect(h.acpConnect).toHaveBeenCalledTimes(1)
    expect(h.attach).toHaveBeenCalledWith(
      "spawned-conn",
      expect.anything(),
      expect.anything()
    )
  })

  it("skips discovery entirely when no persisted conversationId is given", async () => {
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    expect(h.acpFindConnectionForConversation).not.toHaveBeenCalled()
    expect(h.acpConnect).toHaveBeenCalledTimes(1)
  })

  it("viewer teardown detaches WITHOUT acpDisconnect (never kills the owner's agent)", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "owner-conn",
      event_seq: 0,
    })
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    expect(h.acpConnect).not.toHaveBeenCalled()

    await act(async () => {
      await h.actions!.disconnect(TAB)
    })

    // The critical safety property: a viewer must never disconnect the backend
    // connection — it belongs to another client.
    expect(h.acpDisconnect).not.toHaveBeenCalled()
  })

  it("replacing a viewer (changed params) detaches WITHOUT acpDisconnect", async () => {
    // A re-connect at the same tab with a different workingDir hits the
    // replace-existing path. If the existing entry is a viewer, that path must
    // NOT acpDisconnect the owner's connection.
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "owner-conn",
      event_seq: 0,
    })
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/other", "sess-1", 42)
    })

    expect(h.acpDisconnect).not.toHaveBeenCalled()
  })

  it("owner teardown DOES acpDisconnect its own connection", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    expect(h.acpConnect).toHaveBeenCalledTimes(1)

    await act(async () => {
      await h.actions!.disconnect(TAB)
    })

    expect(h.acpDisconnect).toHaveBeenCalledWith("spawned-conn")
  })

  it("desktop viewer torn down DURING snapshot fetch does not seed delegations or route", async () => {
    // Desktop firehose path (no EventStream). If the viewer's tab disconnects
    // while acpGetSessionSnapshot is in flight, the resumed attach must NOT
    // hydrate / seed child delegation streams / install reverse-map routing for
    // a viewer that no longer exists.
    h.eventStreamValue = null
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "owner-conn",
      event_seq: 0,
    })
    let resolveSnapshot: (v: unknown) => void = () => {}
    h.acpGetSessionSnapshot.mockImplementation(
      () =>
        new Promise((res) => {
          resolveSnapshot = res
        })
    )
    await mountProvider()

    // Start the viewer connect; it suspends on the pending snapshot AFTER
    // dispatching CONNECTION_CREATED (the entry now exists in the store).
    let connectPromise: Promise<void> | undefined
    await act(async () => {
      connectPromise = h.actions!.connect(TAB, "claude_code", "/tmp/x", "s", 42)
    })
    // Tear the viewer down while the snapshot is still in flight.
    await act(async () => {
      await h.actions!.disconnect(TAB)
    })
    // Snapshot resolves only AFTER teardown; the resumed attach must bail.
    await act(async () => {
      resolveSnapshot({ connection_id: "owner-conn" })
      await connectPromise
    })

    expect(h.buildDelegationSeedEnvelopes).not.toHaveBeenCalled()
    // And teardown never killed the owner's connection.
    expect(h.acpDisconnect).not.toHaveBeenCalled()
  })
})

// Single-clicking a sidebar conversation opens a PREVIEW tab; the next
// single-click replaces it. That release must never end a turn the user only
// clicked in to watch — an owner's acpDisconnect kills the agent CLI mid-turn,
// which the agent writes into its transcript as an interrupted request.
describe("AcpConnectionsProvider preview-tab release (disconnectIfIdle)", () => {
  async function connectOwner(): Promise<AttachHandlers> {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    return latestAttachHandlers()
  }

  it("keeps a PROMPTING owner alive when its preview tab is replaced", async () => {
    const handlers = await connectOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })

    await act(async () => {
      await h.actions!.disconnectIfIdle(TAB)
    })

    expect(h.acpDisconnect).not.toHaveBeenCalled()
    // Left in the store, still streaming: the idle sweep reclaims it once the
    // turn settles (the tab is gone, so nothing else keeps it alive).
    expect(h.store!.getConnection(TAB)?.status).toBe("prompting")
  })

  it("keeps an owner with outstanding background work alive", async () => {
    const handlers = await connectOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "connected",
    })
    // Turn is over, but launched sub-agents / background shells are not:
    // disconnecting would kill the agent CLI and that work with it.
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "background_activity",
      session_id: "sess-1",
      turns: [],
      outstanding: 1,
      settled: [],
      watermark: 0,
    })
    expect(h.store!.getConnection(TAB)?.backgroundOutstanding).toBe(1)

    await act(async () => {
      await h.actions!.disconnectIfIdle(TAB)
    })

    expect(h.acpDisconnect).not.toHaveBeenCalled()
  })

  it("disconnects an IDLE owner right away (the reclaim this release exists for)", async () => {
    const handlers = await connectOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "connected",
    })

    await act(async () => {
      await h.actions!.disconnectIfIdle(TAB)
    })

    expect(h.acpDisconnect).toHaveBeenCalledWith("spawned-conn")
    expect(h.store!.getConnection(TAB)).toBeUndefined()
  })

  it("detaches a mid-turn VIEWER without killing the owner's agent", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "owner-conn",
      event_seq: 0,
    })
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    emitAcpEvent(latestAttachHandlers(), {
      seq: 1,
      connection_id: "owner-conn",
      type: "status_changed",
      status: "prompting",
    })

    await act(async () => {
      await h.actions!.disconnectIfIdle(TAB)
    })

    // A viewer never owns the backend process, so busy or not it detaches —
    // and the idle sweep skips viewers, so leaving one would leak its stream.
    expect(h.acpDisconnect).not.toHaveBeenCalled()
    expect(h.store!.getConnection(TAB)).toBeUndefined()
  })
})

// The backend dedups connections by (agent, cwd, session), so a connect can
// hand back a connection this client already holds under another contextKey.
describe("AcpConnectionsProvider abandoned connect tears down only what it created", () => {
  const OTHER_TAB = "conv-1-claude_code-99"

  async function connectFirstOwner() {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    h.acpConnect.mockResolvedValue("live-conn")
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    h.acpDisconnect.mockClear()
  }

  it("spares a REUSED connection the client already owns elsewhere", async () => {
    await connectFirstOwner()
    // A second surface resolves to the SAME backend connection (dedup), and is
    // abandoned before the connect settles — killing it would end the first
    // tab's running turn.
    let resolveConnect: (v: string) => void = () => {}
    h.acpConnect.mockImplementation(
      () =>
        new Promise<string>((res) => {
          resolveConnect = res
        })
    )
    let connectPromise: Promise<void> | undefined
    await act(async () => {
      connectPromise = h.actions!.connect(
        OTHER_TAB,
        "claude_code",
        "/tmp/x",
        "sess-1"
      )
    })
    await act(async () => {
      await h.actions!.disconnect(OTHER_TAB)
    })
    await act(async () => {
      resolveConnect("live-conn")
      await connectPromise
    })

    expect(h.acpDisconnect).not.toHaveBeenCalled()
    expect(h.store!.getConnection(TAB)?.connectionId).toBe("live-conn")
  })

  it("still tears down a connection the abandoned connect spawned itself", async () => {
    await connectFirstOwner()
    let resolveConnect: (v: string) => void = () => {}
    h.acpConnect.mockImplementation(
      () =>
        new Promise<string>((res) => {
          resolveConnect = res
        })
    )
    let connectPromise: Promise<void> | undefined
    await act(async () => {
      connectPromise = h.actions!.connect(
        OTHER_TAB,
        "claude_code",
        "/tmp/other",
        "sess-2"
      )
    })
    await act(async () => {
      await h.actions!.disconnect(OTHER_TAB)
    })
    await act(async () => {
      resolveConnect("fresh-conn")
      await connectPromise
    })

    expect(h.acpDisconnect).toHaveBeenCalledWith("fresh-conn")
  })
})

describe("AcpConnectionsProvider permission request details", () => {
  it("hydrates a permission request from an existing live tool call input", async () => {
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    const handlers = latestAttachHandlers()
    const rawInput = JSON.stringify({ command: "pnpm test", cwd: "/tmp/x" })

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "tool_call",
      tool_call_id: "call_1",
      title: "Bash",
      kind: "execute",
      status: "pending",
      content: null,
      raw_input: rawInput,
      raw_output: null,
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "permission_request",
      request_id: "req-1",
      tool_call: {
        kind: "execute",
        status: "pending",
        toolCallId: "call_1",
      },
      options: [],
    })

    const permission = h.store!.getConnection(TAB)!.pendingPermission
    expect(parsePermissionToolCall(permission?.tool_call).title).toBe("Bash")
    expect(parsePermissionToolCall(permission?.tool_call).command).toBe(
      "pnpm test"
    )
    expect(parsePermissionToolCall(permission?.tool_call).cwd).toBe("/tmp/x")
  })

  it("backfills an already-open permission request when tool input arrives later", async () => {
    const originalRaf = globalThis.requestAnimationFrame
    const originalCancelRaf = globalThis.cancelAnimationFrame
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(0)
      return 1
    })
    vi.stubGlobal("cancelAnimationFrame", () => {})

    try {
      await mountProvider()

      await act(async () => {
        await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
      })

      const handlers = latestAttachHandlers()

      emitAcpEvent(handlers, {
        seq: 1,
        connection_id: "spawned-conn",
        type: "permission_request",
        request_id: "req-2",
        tool_call: {
          kind: "execute",
          status: "pending",
          toolCallId: "call_2",
        },
        options: [],
      })

      expect(
        parsePermissionToolCall(
          h.store!.getConnection(TAB)!.pendingPermission?.tool_call
        ).command
      ).toBeNull()

      emitAcpEvent(handlers, {
        seq: 2,
        connection_id: "spawned-conn",
        type: "tool_call_update",
        tool_call_id: "call_2",
        title: "Bash",
        status: "pending",
        content: null,
        raw_input: JSON.stringify({ command: "pnpm build" }),
        raw_output: null,
      })

      expect(
        parsePermissionToolCall(
          h.store!.getConnection(TAB)!.pendingPermission?.tool_call
        ).command
      ).toBe("pnpm build")
    } finally {
      vi.stubGlobal("requestAnimationFrame", originalRaf)
      vi.stubGlobal("cancelAnimationFrame", originalCancelRaf)
    }
  })

  it("hydrates snapshot permission details from active tool call input", async () => {
    await mountProvider()

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    const handlers = latestAttachHandlers()
    h.denormalizeSnapshot.mockReturnValue({
      connectionId: "spawned-conn",
      status: "connected",
      sessionId: "sess-1",
      modes: null,
      configOptions: null,
      availableCommands: [],
      usage: null,
      liveMessage: {
        id: "live-1",
        role: "assistant",
        startedAt: 0,
        content: [
          {
            type: "tool_call",
            info: {
              tool_call_id: "call_snapshot",
              title: "Bash",
              kind: "execute",
              status: "pending",
              content: null,
              raw_input: JSON.stringify({
                command: "pnpm test -- --runInBand",
                cwd: "/tmp/x",
              }),
              raw_output_chunks: [],
              raw_output_total_bytes: 0,
              locations: null,
              meta: null,
              images: [],
            },
          },
        ],
      },
      pendingPermission: {
        request_id: "req-snapshot",
        tool_call: {
          kind: "execute",
          status: "pending",
          toolCallId: "call_snapshot",
        },
        options: [],
      },
      pendingAskQuestion: null,
      pendingUserMessage: null,
      promptCapabilities: null,
      selectorsReady: true,
      supportsFork: false,
      configStale: false,
      configStaleKind: null,
      lastError: null,
      eventSeq: 5,
      activeDelegations: [],
    })
    hydrateSnapshot(handlers, {
      connection_id: "spawned-conn",
      conversation_id: null,
      folder_id: null,
      status: "connected",
      external_id: "sess-1",
      live_message: {
        id: "live-1",
        role: "assistant",
        started_at: new Date(0).toISOString(),
        content: [{ kind: "tool_call_ref", tool_call_id: "call_snapshot" }],
      },
      active_tool_calls: [
        {
          id: "call_snapshot",
          kind: "execute",
          label: "Bash",
          status: "pending",
          input: { command: "pnpm test -- --runInBand", cwd: "/tmp/x" },
          output: null,
          content: null,
          locations: null,
          meta: null,
        },
      ],
      pending_permission: {
        request_id: "req-snapshot",
        tool_call_id: "call_snapshot",
        tool_call: {
          kind: "execute",
          status: "pending",
          toolCallId: "call_snapshot",
        },
        options: [],
        created_at: new Date(0).toISOString(),
      },
      pending_question: null,
      pending_user_message: null,
      active_delegations: [],
      feedback: [],
      feedback_tool_available: false,
      modes: null,
      current_mode: null,
      config_options: null,
      prompt_capabilities: null,
      usage: null,
      fork_supported: false,
      available_commands: [],
      selectors_ready: true,
      config_stale: false,
      config_stale_kind: null,
      event_seq: 5,
    })

    const permission = h.store!.getConnection(TAB)!.pendingPermission
    const parsed = parsePermissionToolCall(permission?.tool_call)
    expect(parsed.title).toBe("Bash")
    expect(parsed.command).toBe("pnpm test -- --runInBand")
    expect(parsed.cwd).toBe("/tmp/x")
  })
})

describe("AcpConnectionsProvider liveMessage sink (mirror out of React)", () => {
  async function connectOwner(): Promise<AttachHandlers> {
    await mountProvider()
    await act(async () => {
      // No conversationId → skip discovery → owner spawn (acpConnect).
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })
    return latestAttachHandlers()
  }

  it("fires with isLive=true and a fresh non-null liveMessage when a turn starts", async () => {
    const handlers = await connectOwner()
    const calls: Array<{ content: unknown; isLive: boolean }> = []
    h.actions!.registerLiveMessageSink(TAB, (lm, isLive) =>
      calls.push({ content: lm.content, isLive })
    )

    // status → prompting resets liveMessage to a fresh empty assistant message.
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })

    expect(calls).toHaveLength(1)
    expect(calls[0]!.isLive).toBe(true)
    expect(calls[0]!.content).toEqual([])
  })

  it("relays a subsequent liveMessage change (tool call appended) to the sink", async () => {
    const handlers = await connectOwner()
    const calls: Array<{ len: number; isLive: boolean }> = []
    h.actions!.registerLiveMessageSink(TAB, (lm, isLive) =>
      calls.push({ len: lm.content.length, isLive })
    )

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "tool_call",
      tool_call_id: "call_1",
      title: "Bash",
      kind: "execute",
      status: "pending",
      content: null,
      raw_input: "{}",
      raw_output: null,
    })

    expect(calls.length).toBeGreaterThanOrEqual(2)
    const last = calls[calls.length - 1]!
    expect(last.isLive).toBe(true)
    expect(last.len).toBe(1) // the appended tool_call block
  })

  it("stops firing after the returned unregister runs", async () => {
    const handlers = await connectOwner()
    let count = 0
    const unregister = h.actions!.registerLiveMessageSink(TAB, () => {
      count += 1
    })

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    expect(count).toBe(1)

    unregister()
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    expect(count).toBe(1) // no further fire
  })

  it("does not fire when a transition leaves liveMessage unchanged", async () => {
    const handlers = await connectOwner()
    let count = 0
    h.actions!.registerLiveMessageSink(TAB, () => {
      count += 1
    })

    // connecting → connected never touches liveMessage (stays null).
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "connected",
    })
    expect(count).toBe(0)
  })

  it("replays the current liveMessage immediately when registering over a live connection", async () => {
    const handlers = await connectOwner()
    // Drive a live message with NO sink registered (e.g. before the panel's
    // registration effect, or a connection reused across a remount).
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "tool_call",
      tool_call_id: "call_1",
      title: "Bash",
      kind: "execute",
      status: "pending",
      content: null,
      raw_input: "{}",
      raw_output: null,
    })

    // Registering now must replay the existing liveMessage once, immediately —
    // otherwise a paused stream (no further delta) would leave the message list
    // blank until the next change.
    const calls: Array<{ len: number; isLive: boolean }> = []
    h.actions!.registerLiveMessageSink(TAB, (lm, isLive) =>
      calls.push({ len: lm.content.length, isLive })
    )
    expect(calls).toHaveLength(1)
    expect(calls[0]!.isLive).toBe(true) // still prompting
    expect(calls[0]!.len).toBe(1) // the tool_call block already present
  })

  it("mirrors to the sink BEFORE notifying connection key subscribers", async () => {
    const handlers = await connectOwner()
    const order: string[] = []
    h.actions!.registerLiveMessageSink(TAB, () => order.push("sink"))
    const unsub = h.store!.subscribeKey(TAB, () => order.push("notify"))

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    unsub()

    // The runtime sink runs before the connection's key subscribers are notified
    // for the liveMessage-changing dispatch. (A benign follow-up dispatch that
    // leaves liveMessage unchanged may append another "notify" without re-firing
    // the sink — assert the ordering + single sink, not the total notify count.)
    expect(order[0]).toBe("sink")
    expect(order.filter((x) => x === "sink")).toHaveLength(1)
    expect(order.indexOf("sink")).toBeLessThan(order.indexOf("notify"))
  })
})

describe("out-of-turn wire guard + background activity", () => {
  async function mountOwnerConnection() {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    return latestAttachHandlers()
  }

  it("distinguishes completed, interrupted, and failed turns in OS notifications", async () => {
    const handlers = await mountOwnerConnection()
    h.sendSystemNotification.mockClear()

    for (const [index, stopReason] of ["end_turn", "cancelled"].entries()) {
      emitAcpEvent(handlers, {
        seq: index + 1,
        connection_id: "spawned-conn",
        type: "turn_complete",
        session_id: "sess-1",
        stop_reason: stopReason,
        agent_type: "codex",
      })
    }
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "error",
      message: "Codex refused the prompt",
      agent_type: "codex",
      code: "turn_failed_refusal",
    })
    emitAcpEvent(handlers, {
      seq: 4,
      connection_id: "spawned-conn",
      type: "turn_complete",
      session_id: "sess-1",
      stop_reason: "refusal",
      agent_type: "codex",
    })

    expect(h.sendSystemNotification.mock.calls.map((call) => call[1])).toEqual([
      "notificationTurnComplete",
      "notificationTurnCancelled",
      "backendErrors.turnFailedRefusal",
    ])
  })

  it("includes the question result when Codex waits for an answer", async () => {
    const handlers = await mountOwnerConnection()
    h.sendSystemNotification.mockClear()

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "question_request",
      question_id: "question-1",
      questions: [
        {
          id: "scope",
          header: "Scope",
          question: "Should the notification include the final result?",
          multi_select: false,
          options: [],
        },
      ],
    })

    expect(h.sendSystemNotification).toHaveBeenCalledWith(
      "x - Claude Code questionDialog.title",
      "Should the notification include the final result?"
    )
  })

  it("drops streaming deltas while the connection is not prompting (Bug-A guard)", async () => {
    const handlers = await mountOwnerConnection()

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "connected",
    })
    // Out-of-turn delta (the backend idle loop forwards these between turns):
    // must NOT graft onto a liveMessage. The next status_changed flushes the
    // streaming queue BEFORE the status dispatch, so the drop is exercised
    // deterministically with the pre-flip status still "connected".
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "out-of-turn garbage",
    })
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    // Prompting resets liveMessage to an empty shell; the dropped delta must
    // not appear in it.
    const afterPrompting = h.store!.getConnection(TAB)
    expect(afterPrompting?.liveMessage?.content ?? []).toEqual([])

    // In-turn delta flows normally (flushed by the next non-streaming event).
    emitAcpEvent(handlers, {
      seq: 4,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "real reply",
    })
    emitAcpEvent(handlers, {
      seq: 5,
      connection_id: "spawned-conn",
      type: "usage_update",
      used: 1,
      size: 100,
    })
    const conn = h.store!.getConnection(TAB)
    expect(conn?.liveMessage?.content).toEqual([
      { type: "text", text: "real reply" },
    ])
  })

  it("routes parented deltas into separate blocks and drops orphans (claude-agent-acp ≥0.63)", async () => {
    const handlers = await mountOwnerConnection()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    // The launching Agent tool call precedes its subagent's chunks on the
    // seq-ordered wire — required by the reducer's parent-presence gate.
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "tool_call",
      tool_call_id: "toolu_parent",
      title: "Agent",
      kind: "other",
      status: "in_progress",
      content: null,
      raw_input: null,
      raw_output: null,
    })
    // main → sub → main within ONE flush window: the queue pre-coalescing
    // must not concatenate across attributions, and the reducer must produce
    // three separate text blocks.
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "main ",
    })
    emitAcpEvent(handlers, {
      seq: 4,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "sub report",
      parent_tool_use_id: "toolu_parent",
    })
    emitAcpEvent(handlers, {
      seq: 5,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "main tail",
    })
    // Orphan: no such tool call in liveMessage → dropped entirely.
    emitAcpEvent(handlers, {
      seq: 6,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "orphan noise",
      parent_tool_use_id: "toolu_unknown",
    })
    // Parented thinking lands as its own attributed block.
    emitAcpEvent(handlers, {
      seq: 7,
      connection_id: "spawned-conn",
      type: "thinking",
      text: "sub reasoning",
      parent_tool_use_id: "toolu_parent",
    })
    // Non-streaming event flushes the queue deterministically.
    emitAcpEvent(handlers, {
      seq: 8,
      connection_id: "spawned-conn",
      type: "usage_update",
      used: 1,
      size: 100,
    })

    const conn = h.store!.getConnection(TAB)
    const content = conn?.liveMessage?.content ?? []
    const rendered = content.map((b) =>
      b.type === "text" || b.type === "thinking"
        ? { type: b.type, text: b.text, parent: b.parentToolUseId ?? null }
        : { type: b.type }
    )
    expect(rendered).toEqual([
      { type: "tool_call" },
      { type: "text", text: "main ", parent: null },
      { type: "text", text: "sub report", parent: "toolu_parent" },
      { type: "text", text: "main tail", parent: null },
      { type: "thinking", text: "sub reasoning", parent: "toolu_parent" },
    ])
  })

  it("merges consecutive same-parent deltas into one growing block", async () => {
    const handlers = await mountOwnerConnection()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "tool_call",
      tool_call_id: "toolu_parent",
      title: "Agent",
      kind: "other",
      status: "in_progress",
      content: null,
      raw_input: null,
      raw_output: null,
    })
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "part one, ",
      parent_tool_use_id: "toolu_parent",
    })
    emitAcpEvent(handlers, {
      seq: 4,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "part two",
      parent_tool_use_id: "toolu_parent",
    })
    emitAcpEvent(handlers, {
      seq: 5,
      connection_id: "spawned-conn",
      type: "usage_update",
      used: 1,
      size: 100,
    })
    const content = h.store!.getConnection(TAB)?.liveMessage?.content ?? []
    const texts = content.filter((b) => b.type === "text")
    expect(texts).toHaveLength(1)
    expect(texts[0]).toMatchObject({
      text: "part one, part two",
      parentToolUseId: "toolu_parent",
    })
  })

  it("background_activity mirrors outstanding, applies overlay turns, and notifies settled tasks", async () => {
    const { useConversationRuntimeStore, resetConversationRuntimeStore } =
      await import("@/stores/conversation-runtime-store")
    const { sendSystemNotification } = await import("@/lib/notification")
    const notify = vi.mocked(sendSystemNotification)
    notify.mockClear()
    const { getFolderConversation } = await import("@/lib/api")
    vi.mocked(getFolderConversation).mockClear()
    resetConversationRuntimeStore()
    // Bind the agent session id to a runtime conversation so the overlay
    // bridge can resolve it. Model the draft-started shape (the common QA
    // flow): the runtime session key is a virtual NEGATIVE id and the real
    // DB row id (42) is bound separately — the settle refetch must fetch
    // with 42, not the virtual key (which the backend would reject,
    // silently leaving the launch card frozen on its ack).
    const VIRTUAL = -9
    useConversationRuntimeStore
      .getState()
      .actions.setExternalId(VIRTUAL, "sess-1")
    useConversationRuntimeStore
      .getState()
      .actions.setDbConversationId(VIRTUAL, 42)

    const handlers = await mountOwnerConnection()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "background_activity",
      session_id: "sess-1",
      turns: [
        {
          id: "bg-100-0",
          role: "assistant",
          blocks: [{ type: "text", text: "build finished cleanly" }],
          timestamp: "2026-07-07T03:47:08.000Z",
        },
      ],
      outstanding: 2,
      settled: [
        {
          task_id: "agent1",
          status: "completed",
          summary: 'Agent "Run pnpm build" finished',
          tool_use_id: "toolu_01",
          result: "Build succeeded (exit code 0).",
        },
      ],
      watermark: 4096,
    })

    // 1. outstanding mirrored onto the connection (sweep exemption + chip);
    //    the settlement arms the "syncing results" bridge state (the agent's
    //    reaction turn is being generated).
    expect(h.store!.getConnection(TAB)?.backgroundOutstanding).toBe(2)
    expect(h.store!.getConnection(TAB)?.backgroundSettleSyncingSince).toEqual(
      expect.any(Number)
    )

    // 2. overlay turn upserted into the runtime session — under the RUNTIME
    //    key (that's the session the panel renders).
    const session = useConversationRuntimeStore
      .getState()
      .byConversationId.get(VIRTUAL)
    expect(session?.backgroundTurns).toHaveLength(1)
    expect(session?.backgroundTurns[0]).toMatchObject({
      watermark: 4096,
      turn: { id: "bg-100-0" },
    })

    // 3. one OS notification per settled task, carrying its summary.
    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify.mock.calls[0][1]).toContain('Agent "Run pnpm build" finished')

    // 4. the settlement flips the launch card IN-MEMORY (no detail refetch):
    //    with no promoted card yet (it's mid-stream), it's queued under the
    //    runtime key by `tool_use_id` for COMPLETE_TURN to apply.
    expect(vi.mocked(getFolderConversation)).not.toHaveBeenCalled()
    expect(session?.pendingBackgroundSettlements).toEqual([
      {
        toolUseId: "toolu_01",
        taskId: "agent1",
        status: "completed",
        summary: 'Agent "Run pnpm build" finished',
        result: "Build succeeded (exit code 0).",
      },
    ])

    // Accounting-only follow-up (work settles to zero): mirror updates, no
    // duplicate overlay entries, no extra notification.
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "background_activity",
      session_id: "sess-1",
      outstanding: 0,
      watermark: 4200,
    })
    expect(h.store!.getConnection(TAB)?.backgroundOutstanding).toBe(0)
    expect(
      useConversationRuntimeStore.getState().byConversationId.get(VIRTUAL)
        ?.backgroundTurns
    ).toHaveLength(1)
    expect(notify).toHaveBeenCalledTimes(1)
    // Accounting-only events keep the syncing bridge armed — the reaction
    // turn hasn't surfaced yet.
    expect(h.store!.getConnection(TAB)?.backgroundSettleSyncingSince).toEqual(
      expect.any(Number)
    )

    // The reaction turn arriving (turns-only event) disarms the bridge.
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "background_activity",
      session_id: "sess-1",
      turns: [
        {
          id: "bg-100-1",
          role: "assistant",
          blocks: [{ type: "text", text: "here is what the build produced" }],
          timestamp: "2026-07-07T03:47:12.000Z",
        },
      ],
      outstanding: 0,
      watermark: 4400,
    })
    expect(h.store!.getConnection(TAB)?.backgroundSettleSyncingSince).toBeNull()

    resetConversationRuntimeStore()
  })

  it("does NOT arm the syncing-results hint for a wire-visible (#870-held) settle", async () => {
    const { resetConversationRuntimeStore } =
      await import("@/stores/conversation-runtime-store")
    resetConversationRuntimeStore()
    const handlers = await mountOwnerConnection()

    // #870: the launching turn is held OPEN and the sub-agent's reply streams
    // live as the tail of that held turn — the backend marks the settle
    // `wire_visible: true`. There is no "results not yet visible" gap, so the
    // hint must stay hidden (not strand on "Syncing background results…" until
    // the 30s cap). Gated on the backend flag, NOT the connection status, so it
    // holds even if this event is delivered after the turn returns to connected.
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "background_activity",
      session_id: "sess-1",
      outstanding: 0,
      settled: [
        {
          task_id: "agent1",
          status: "completed",
          tool_use_id: "toolu_01",
          result: "done",
          wire_visible: true,
        },
      ],
      watermark: 100,
    })

    expect(h.store!.getConnection(TAB)?.backgroundOutstanding).toBe(0)
    expect(h.store!.getConnection(TAB)?.backgroundSettleSyncingSince).toBeNull()

    resetConversationRuntimeStore()
  })
})

describe("AcpConnectionsProvider Grok cross-agent-type model switch", () => {
  function grokModelOptions(current: string): SessionConfigOptionInfo[] {
    return [
      {
        id: "model",
        name: "Model",
        category: "model",
        kind: {
          type: "select",
          current_value: current,
          options: [
            { value: "grok-4.5", name: "Grok 4.5" },
            { value: "grok-composer-2.5-fast", name: "Composer 2.5" },
          ],
          groups: [],
        },
      },
    ]
  }

  async function connectGrokOwner(): Promise<AttachHandlers> {
    h.acpGetAgentStatus.mockResolvedValue({
      agent_type: "grok",
      enabled: true,
      available: true,
      installed_version: "0.2.94",
      is_acp_adapter: false,
    })
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "grok", "/tmp/x", "sess-1")
    })
    return latestAttachHandlers()
  }

  it("applies a mid-turn config_option_update while the turn is still prompting", async () => {
    // codex-acp ≥1.1.8 flips `collaboration_mode` back to the default IN THE
    // MIDDLE of a turn once the user approves a plan review, then keeps
    // streaming the implementation under the same session/prompt. The selector
    // must follow — this is metadata, not agent output, so the out-of-turn
    // guards that drop tool calls / deltas must not touch it.
    const handlers = await connectGrokOwner()

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    expect(h.store!.getConnection(TAB)!.status).toBe("prompting")

    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-composer-2.5-fast"),
    })

    const conn = h.store!.getConnection(TAB)!
    expect(conn.status).toBe("prompting")
    expect(conn.configOptions?.[0]?.kind.current_value).toBe(
      "grok-composer-2.5-fast"
    )
  })

  it("reverts the optimistic pick, surfaces the localized error, and keeps the attempted preference", async () => {
    const handlers = await connectGrokOwner()

    // Composer selector arrives with grok-4.5 active.
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })
    expect(
      h.store!.getConnection(TAB)!.configOptions?.[0]?.kind.current_value
    ).toBe("grok-4.5")

    // User optimistically switches to the cross-agent-type Composer model.
    vi.mocked(saveConfigPreference).mockClear()
    await act(async () => {
      await h.actions!.setConfigOption(TAB, "model", "grok-composer-2.5-fast")
    })
    // Optimistic: the selector shows the pick and the preference is persisted.
    expect(
      h.store!.getConnection(TAB)!.configOptions?.[0]?.kind.current_value
    ).toBe("grok-composer-2.5-fast")
    expect(saveConfigPreference).toHaveBeenCalledTimes(1)
    expect(saveConfigPreference).toHaveBeenCalledWith(
      "grok",
      "model",
      "grok-composer-2.5-fast"
    )

    // Backend rejects the switch mid-conversation: it re-emits the authoritative
    // options (revert) followed by the coded, recoverable error.
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "error",
      message: "Cannot switch to that model in an existing conversation.",
      agent_type: "grok",
      code: "grok_model_switch_incompatible_agent",
    })

    const conn = h.store!.getConnection(TAB)!
    // The selector snapped back to the model actually in effect.
    expect(conn.configOptions?.[0]?.kind.current_value).toBe("grok-4.5")
    // The coded error is localized (the useTranslations mock echoes the key) —
    // NOT the raw fallback message.
    expect(conn.error).toBe("backendErrors.grokModelSwitchIncompatibleAgent")
    // The attempted model stays the saved preference (no revert of the persisted
    // choice), so a fresh session lands on Composer where the switch succeeds.
    expect(saveConfigPreference).toHaveBeenCalledTimes(1)
  })
})

describe("empty-turn error diagnostics", () => {
  async function connectOwner(): Promise<AttachHandlers> {
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })
    return latestAttachHandlers()
  }

  it("localizes each empty-turn code", async () => {
    const handlers = await connectOwner()

    const cases = [
      ["turn_failed_empty", "backendErrors.turnFailedEmpty"],
      ["turn_failed_empty_protocol", "backendErrors.turnFailedEmptyProtocol"],
      ["turn_failed_empty_metadata", "backendErrors.turnFailedEmptyMetadata"],
    ] as const

    // Sequence numbers must advance — the store's seq guard drops replays.
    cases.forEach(([code, key], i) => {
      emitAcpEvent(handlers, {
        seq: i + 1,
        connection_id: "spawned-conn",
        type: "error",
        message: "raw english fallback",
        agent_type: "claude_code",
        code,
      })
      expect(h.store!.getConnection(TAB)!.error).toBe(key)
    })
  })

  it("appends details to the alert but keeps them out of conn.error and the OS notification", async () => {
    const handlers = await connectOwner()
    h.pushAlert.mockClear()
    h.sendSystemNotification.mockClear()

    const details =
      "dropped 1 update(s) (0 decode, 1 dispatch)\nstderr (this turn, last 1 lines):\n  Error: 401 Unauthorized"
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "error",
      message: "raw english fallback",
      agent_type: "claude_code",
      code: "turn_failed_empty_protocol",
      details,
    })

    // The alert's detail slot carries the localized line AND the evidence.
    const alertCalls = h.pushAlert.mock.calls
    const [, , alertDetail] = alertCalls[alertCalls.length - 1]!
    expect(alertDetail).toContain("backendErrors.turnFailedEmptyProtocol")
    expect(alertDetail).toContain("Error: 401 Unauthorized")

    // `conn.error` feeds the composer tooltip — one line only.
    expect(h.store!.getConnection(TAB)!.error).toBe(
      "backendErrors.turnFailedEmptyProtocol"
    )

    // Notification centers persist their payload outside the app.
    const notifyCalls = h.sendSystemNotification.mock.calls
    const notificationArgs = notifyCalls[notifyCalls.length - 1]!
    expect(JSON.stringify(notificationArgs)).not.toContain("401 Unauthorized")
  })

  it("omits blank details rather than rendering an empty block", async () => {
    const handlers = await connectOwner()
    h.pushAlert.mockClear()

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "error",
      message: "raw english fallback",
      agent_type: "claude_code",
      code: "turn_failed_empty",
      details: "   \n  ",
    })

    const alertCalls = h.pushAlert.mock.calls
    const [, , alertDetail] = alertCalls[alertCalls.length - 1]!
    expect(alertDetail).toBe("backendErrors.turnFailedEmpty")
  })
})

describe("HYDRATE_FROM_SNAPSHOT last_error recovery", () => {
  // Full SnapshotPatch fixture; per-test overrides set connectionId / eventSeq /
  // lastError. `denormalizeSnapshot` is mocked, so onSnapshot dispatches exactly
  // this object as `action.patch`.
  function snapshotPatch(overrides: {
    eventSeq: number
    lastError: string | null
    lastErrorDetails?: string | null
    connectionId?: string
  }) {
    return {
      connectionId: "spawned-conn",
      status: "connected",
      sessionId: null,
      modes: null,
      configOptions: null,
      availableCommands: null,
      usage: null,
      liveMessage: null,
      pendingPermission: null,
      pendingAskQuestion: null,
      pendingUserMessage: null,
      promptCapabilities: null,
      selectorsReady: false,
      supportsFork: false,
      configStale: false,
      configStaleKind: null,
      backgroundOutstanding: 0,
      activeDelegations: [],
      lastErrorDetails: null,
      ...overrides,
    }
  }

  async function connectOwner(): Promise<AttachHandlers> {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    h.acpGetAgentStatus.mockResolvedValue({
      agent_type: "claude_code",
      enabled: true,
      available: true,
      installed_version: "1.0.0",
      is_acp_adapter: true,
    })
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    return latestAttachHandlers()
  }

  it("recovers last_error from a FRESH snapshot (client missed the live error)", async () => {
    const handlers = await connectOwner()
    // A freshly reconnected client (lastAppliedSeq=0) receives a snapshot ahead
    // of its cursor carrying an error whose live event it never saw. The fresh
    // path recovers it.
    h.denormalizeSnapshot.mockReturnValue(
      snapshotPatch({ eventSeq: 5, lastError: "boom from snapshot" })
    )
    hydrateSnapshot(handlers, {
      event_seq: 5,
    } as unknown as LiveSessionSnapshot)
    expect(h.store!.getConnection(TAB)!.error).toBe("boom from snapshot")
  })

  it("does NOT resurrect a cleared error from a STALE snapshot", async () => {
    const handlers = await connectOwner()
    // Live: an error lands, then a new prompt starts and clears it. This also
    // advances lastAppliedSeq to 2.
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "error",
      message: "boom",
      agent_type: "claude_code",
      code: "runtime_failure",
    })
    expect(h.store!.getConnection(TAB)!.error).toBe("boom")
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    expect(h.store!.getConnection(TAB)!.error).toBeNull()

    // A snapshot generated BEFORE the prompt (eventSeq=1 <= lastAppliedSeq=2)
    // still carries the old error. Folding it back in would resurrect an error
    // the current turn already cleared — the stale path must leave error alone.
    h.denormalizeSnapshot.mockReturnValue(
      snapshotPatch({ eventSeq: 1, lastError: "boom" })
    )
    hydrateSnapshot(handlers, {
      event_seq: 1,
    } as unknown as LiveSessionSnapshot)
    expect(h.store!.getConnection(TAB)!.error).toBeNull()
  })

  // Alerts are live-only, so a client that attached after the empty turn has
  // the snapshot as its ONLY channel for the diagnosis.
  it("raises an alert for snapshot-carried details without touching conn.error or notifications", async () => {
    const handlers = await connectOwner()
    h.pushAlert.mockClear()
    h.sendSystemNotification.mockClear()

    const details =
      "stderr (this turn, last 1 lines):\n  Error: 401 Unauthorized"
    h.denormalizeSnapshot.mockReturnValue(
      snapshotPatch({
        eventSeq: 5,
        lastError: "agent ended the turn without producing any response.",
        lastErrorDetails: details,
      })
    )
    hydrateSnapshot(handlers, {
      event_seq: 5,
    } as unknown as LiveSessionSnapshot)

    const alertCalls = h.pushAlert.mock.calls
    const [, , alertDetail] = alertCalls[alertCalls.length - 1]!
    expect(alertDetail).toContain("Error: 401 Unauthorized")
    // The tooltip string stays the single-line message.
    expect(h.store!.getConnection(TAB)!.error).toBe(
      "agent ended the turn without producing any response."
    )
    expect(h.sendSystemNotification).not.toHaveBeenCalled()
  })

  it("does not re-alert the same details on every re-attach", async () => {
    const handlers = await connectOwner()
    h.pushAlert.mockClear()

    const patch = snapshotPatch({
      eventSeq: 5,
      lastError: "boom",
      lastErrorDetails: "stderr (this turn, last 1 lines):\n  same evidence",
    })
    h.denormalizeSnapshot.mockReturnValue(patch)
    hydrateSnapshot(handlers, {
      event_seq: 5,
    } as unknown as LiveSessionSnapshot)
    const afterFirst = h.pushAlert.mock.calls.length
    expect(afterFirst).toBe(1)

    // A reconnect replays the same snapshot.
    hydrateSnapshot(handlers, {
      event_seq: 6,
    } as unknown as LiveSessionSnapshot)
    expect(h.pushAlert.mock.calls.length).toBe(afterFirst)
  })

  it("stays silent for snapshot errors that carry no details", async () => {
    const handlers = await connectOwner()
    h.pushAlert.mockClear()

    h.denormalizeSnapshot.mockReturnValue(
      snapshotPatch({ eventSeq: 5, lastError: "some older error" })
    )
    hydrateSnapshot(handlers, {
      event_seq: 5,
    } as unknown as LiveSessionSnapshot)

    // Attaching to a connection with an ordinary past error must not start
    // raising alerts it never used to.
    expect(h.pushAlert).not.toHaveBeenCalled()
  })
})

describe("global acp://event listener is mount-once", () => {
  // Regression guard for the duplicated-reply report: `handleMappedEvent`
  // closes over the i18n `t` / `tChat`, so if the global listener effect
  // depends on its identity, every provider re-render tears the Tauri
  // subscription down and re-registers it. Both `listen` and `unlisten` are
  // async IPC, so that churn leaves two listeners live at once and each
  // envelope is delivered twice. The handler must be reached through a
  // latest-ref instead, so the subscription is registered exactly once.
  let unlisten: ReturnType<typeof vi.fn>

  function mountDesktop() {
    return render(
      <AcpConnectionsProvider>
        <Probe />
      </AcpConnectionsProvider>
    )
  }

  beforeEach(() => {
    // Desktop firehose path — the web/attach transport skips this effect.
    h.eventStreamValue = null
    unlisten = vi.fn()
    vi.mocked(subscribe).mockClear()
    vi.mocked(subscribe).mockImplementation(async () => unlisten)
  })

  // Restore the suite-wide default. `subscribe` is a module-level mock that
  // the outer `beforeEach` does not reset, so leaving our implementation (or
  // a bare `mockReset`, which returns `undefined` and would make the
  // listener's `.then()` throw) installed would break any desktop-path test
  // added after this block.
  afterEach(() => {
    vi.mocked(subscribe).mockImplementation(async () => () => {})
  })

  it("subscribes exactly once across provider re-renders", async () => {
    // Precondition: this suite's next-intl mock hands out a FRESH `t` per
    // call, so every re-render rebuilds `handleMappedEvent`. If that mock ever
    // becomes memoized, this test would silently stop covering the bug.
    expect(useTranslations("Folder.chat")).not.toBe(
      useTranslations("Folder.chat")
    )

    const { rerender } = mountDesktop()
    await act(async () => {})

    expect(vi.mocked(subscribe)).toHaveBeenCalledTimes(1)
    expect(vi.mocked(subscribe).mock.calls[0]![0]).toBe("acp://event")

    for (let i = 0; i < 3; i++) {
      await act(async () => {
        rerender(
          <AcpConnectionsProvider>
            <Probe />
          </AcpConnectionsProvider>
        )
      })
    }

    expect(vi.mocked(subscribe)).toHaveBeenCalledTimes(1)
    // Never torn down while mounted — no window with two live listeners.
    expect(unlisten).not.toHaveBeenCalled()
  })

  it("keeps delivering events through the surviving listener after re-renders", async () => {
    // Guards the other half of the fix: the one subscription that survives
    // must still route, and each delta must land exactly once (the reported
    // symptom was doubled text). Note this canNOT distinguish an old from a
    // new `t` closure — the suite's mocked translator returns the key
    // verbatim, so both produce identical output. Ref freshness itself rests
    // on the sync effect running every render; what this catches is a dead,
    // detached, or wrongly-frozen handler.
    const { rerender } = mountDesktop()
    await act(async () => {})

    await act(async () => {
      // No conversationId → skip discovery → owner spawn (acpConnect).
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    await act(async () => {
      rerender(
        <AcpConnectionsProvider>
          <Probe />
        </AcpConnectionsProvider>
      )
    })

    const onEvent = vi.mocked(subscribe).mock.calls[0]![1] as (
      envelope: EventEnvelope
    ) => void

    act(() => {
      onEvent({
        seq: 1,
        connection_id: "spawned-conn",
        type: "status_changed",
        status: "prompting",
      } as EventEnvelope)
      onEvent({
        seq: 2,
        connection_id: "spawned-conn",
        type: "content_delta",
        text: "你好",
      } as EventEnvelope)
    })
    // Streaming deltas are queued and flushed on a 16ms timer.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 32))
    })

    const live = h.store!.getConnection(TAB)!.liveMessage
    const text = (live!.content as Array<{ type: string; text?: string }>)
      .filter((b) => b.type === "text")
      .map((b) => b.text ?? "")
      .join("")
    expect(text).toBe("你好")
  })

  it("unsubscribes on unmount", async () => {
    const { unmount } = mountDesktop()
    await act(async () => {})

    expect(vi.mocked(subscribe)).toHaveBeenCalledTimes(1)
    unmount()
    expect(unlisten).toHaveBeenCalledTimes(1)
  })
})

describe("delegation-child attach: mid-turn hydration", () => {
  // A work-task session viewer attaches to a turn that is ALREADY running.
  // On desktop the `acp://event` firehose carries only FUTURE events, so
  // without a snapshot the child sits at DELEGATION_CHILD_ATTACH's synthetic
  // "connected" with an empty live message — the viewer shows a stale
  // persisted transcript and never streams. Real delegation children attach
  // at spawn time and must NOT pay for a snapshot fetch.
  const CHILD = "task-conn-1"

  function attachChild(hydrate: boolean) {
    h.actions!.attachDelegationChild({
      connectionId: CHILD,
      parentConnectionId: CHILD,
      parentToolUseId: "work-task-9",
      agentType: "claude_code",
      hydrate,
    })
  }

  beforeEach(() => {
    // Desktop firehose path (the web attach protocol always opens with a
    // snapshot, so the gap this covers is desktop-only).
    h.eventStreamValue = null
    vi.mocked(subscribe).mockClear()
    h.acpGetSessionSnapshot.mockResolvedValue({
      connection_id: CHILD,
      event_seq: 7,
    })
    h.denormalizeSnapshot.mockReturnValue({
      connectionId: CHILD,
      status: "prompting",
      sessionId: "sess-child",
      modes: null,
      configOptions: null,
      availableCommands: null,
      usage: null,
      liveMessage: null,
      pendingPermission: null,
      pendingAskQuestion: null,
      pendingUserMessage: null,
      pendingPlanApproval: null,
      promptCapabilities: null,
      selectorsReady: false,
      supportsFork: false,
      configStale: false,
      configStaleKind: null,
      lastError: null,
      eventSeq: 7,
      activeDelegations: [],
    })
  })

  it("hydrates the in-flight turn, then routes later firehose events", async () => {
    await mountProvider()

    await act(async () => {
      attachChild(true)
    })
    await act(async () => {})

    expect(h.acpGetSessionSnapshot).toHaveBeenCalledWith(CHILD)
    // The load-bearing bit: "prompting" is what makes the read-only viewer
    // render the live stream instead of a settled transcript.
    expect(h.store!.getConnection(CHILD)?.status).toBe("prompting")
    expect(h.store!.getConnection(CHILD)?.lastAppliedSeq).toBe(7)

    // Reverse-map routing is installed AFTER hydration, so post-snapshot
    // events still land (and pre-snapshot ones are deduped by seq).
    const onEvent = vi.mocked(subscribe).mock.calls[0]![1] as (
      envelope: EventEnvelope
    ) => void
    act(() => {
      onEvent({
        seq: 8,
        connection_id: CHILD,
        type: "content_delta",
        text: "hi",
      } as EventEnvelope)
    })
    expect(h.store!.getConnection(CHILD)?.lastAppliedSeq).toBe(8)
  })

  it("re-seeds delegation bindings the hydrated snapshot carries", async () => {
    // `delegation_started` is transient — never in the snapshot's event set and
    // never replayed — so a viewer opening onto a turn that already delegated
    // establishes no binding unless the snapshot's `active_delegations` is
    // fanned out. Without this the work-task dialog's sub-agent cards lose
    // their agent icon/label, the child's live sub-stream and the "待批准"
    // badge. The other three snapshot consumers already did this; the desktop
    // hydrate branch did not.
    const active = [
      {
        parent_tool_use_id: "toolu_child",
        child_connection_id: "child-conn",
        child_conversation_id: 4242,
        agent_type: "codex" as const,
        task_preview: "review the diff",
        task_id: "task-1",
      },
    ]
    h.denormalizeSnapshot.mockReturnValue({
      connectionId: CHILD,
      status: "prompting",
      sessionId: "sess-child",
      modes: null,
      configOptions: null,
      availableCommands: null,
      usage: null,
      liveMessage: null,
      pendingPermission: null,
      pendingAskQuestion: null,
      pendingUserMessage: null,
      pendingPlanApproval: null,
      promptCapabilities: null,
      selectorsReady: false,
      supportsFork: false,
      configStale: false,
      configStaleKind: null,
      lastError: null,
      eventSeq: 7,
      activeDelegations: active,
    })
    await mountProvider()

    await act(async () => {
      attachChild(true)
    })
    await act(async () => {})

    expect(h.buildDelegationSeedEnvelopes).toHaveBeenCalledWith(
      CHILD,
      active,
      7
    )
  })

  it("does not seed when the child detached while the snapshot was in flight", async () => {
    let resolveSnapshot: (v: unknown) => void = () => {}
    h.acpGetSessionSnapshot.mockImplementation(
      () =>
        new Promise((res) => {
          resolveSnapshot = res
        })
    )
    await mountProvider()

    await act(async () => {
      attachChild(true)
    })
    await act(async () => {
      h.actions!.detachDelegationChild(CHILD)
    })
    await act(async () => {
      resolveSnapshot({ connection_id: CHILD, event_seq: 7 })
    })
    await act(async () => {})

    expect(h.buildDelegationSeedEnvelopes).not.toHaveBeenCalled()
  })

  it("skips the snapshot for a spawn-time child attach", async () => {
    await mountProvider()

    await act(async () => {
      attachChild(false)
    })
    await act(async () => {})

    expect(h.acpGetSessionSnapshot).not.toHaveBeenCalled()
    expect(h.store!.getConnection(CHILD)?.status).toBe("connected")
  })

  it("does not hydrate or route a child detached while the snapshot is in flight", async () => {
    let resolveSnapshot: (v: unknown) => void = () => {}
    h.acpGetSessionSnapshot.mockImplementation(
      () =>
        new Promise((res) => {
          resolveSnapshot = res
        })
    )
    await mountProvider()

    await act(async () => {
      attachChild(true)
    })
    await act(async () => {
      h.actions!.detachDelegationChild(CHILD)
    })
    await act(async () => {
      resolveSnapshot({ connection_id: CHILD, event_seq: 7 })
    })
    await act(async () => {})

    // The viewer is gone: no resurrected connection state, and the firehose
    // must not be routing to a contextKey nobody is watching.
    expect(h.store!.getConnection(CHILD)).toBeUndefined()
    const onEvent = vi.mocked(subscribe).mock.calls[0]![1] as (
      envelope: EventEnvelope
    ) => void
    act(() => {
      onEvent({
        seq: 8,
        connection_id: CHILD,
        type: "content_delta",
        text: "hi",
      } as EventEnvelope)
    })
    expect(h.store!.getConnection(CHILD)).toBeUndefined()
  })
})
