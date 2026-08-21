import { useEffect } from "react"
import { act, render } from "@testing-library/react"
import { useTranslations } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  AcpConnectionsProvider,
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
    acpTouchConnection: vi.fn(),
    acpCancel: vi.fn(),
    buildDelegationSeedEnvelopes: vi.fn(() => []),
    denormalizeSnapshot: vi.fn(),
    // Stable across renders so tests can assert on what the error handler
    // routes to the status-bar alert vs. to the OS notification.
    pushAlert: vi.fn(),
    sendSystemNotification: vi.fn(async () => undefined),
    toastWarning: vi.fn(),
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

vi.mock("sonner", () => ({
  toast: { warning: h.toastWarning },
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
  acpCancel: h.acpCancel,
  acpRespondPermission: vi.fn(),
  acpTouchConnection: h.acpTouchConnection,
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
    host_tools_agent_mode: false,
    is_acp_adapter: true,
  })
  h.acpConnect.mockResolvedValue("spawned-conn")
  h.acpDisconnect.mockResolvedValue(undefined)
  h.acpGetSessionSnapshot.mockResolvedValue(null)
  h.acpTouchConnection.mockReset()
  // Default: the backend still holds every connection under test. The liveness
  // probe treats `false` as "gone", so a default of `undefined` would settle
  // healthy connections in unrelated suites.
  h.acpTouchConnection.mockResolvedValue(true)
  h.acpCancel.mockReset()
  h.acpCancel.mockResolvedValue(undefined)
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

// AIR typed session failures: retry warnings must settle ONLY at a clean
// `end_turn` — a cancelled/failed exit did not recover, and a failed turn's
// terminal failure arrives as a `session_failure` event emitted just before
// its `turn_complete` (the record rides the prompt RESPONSE `_meta`; both
// adapters disguise that response as `end_turn`). Settling on any
// leave-prompting transition painted a still-dead connection as a recovered
// warning (2026-08-15 field report).
describe("AcpConnectionsProvider AIR session-failure lifecycle", () => {
  async function connectOwner(): Promise<AttachHandlers> {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    return latestAttachHandlers()
  }

  function failure(
    id: string,
    revision: number,
    severity: string,
    title: string
  ) {
    return {
      id,
      revision,
      category: "connection",
      severity,
      title,
      actions: ["new_session"],
    }
  }

  it("escalates the response-borne terminal error instead of settling it at the disguised end_turn", async () => {
    const handlers = await connectOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "session_failure",
      record: failure(
        "t1:error",
        5,
        "warning",
        "Reconnecting to Claude, attempt 5 of 5."
      ),
    })
    // The terminal record rides the prompt response; the backend emits it
    // BEFORE turn_complete as a same-id higher-revision error escalation.
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "session_failure",
      record: failure(
        "t1:error",
        6,
        "error",
        "The connection to Claude was lost."
      ),
    })
    emitAcpEvent(handlers, {
      seq: 4,
      connection_id: "spawned-conn",
      type: "turn_complete",
      session_id: "sess-1",
      stop_reason: "end_turn",
    })

    const failures = h.store!.getConnection(TAB)?.sessionFailures
    expect(failures).toHaveLength(1)
    expect(failures?.[0]).toMatchObject({
      id: "t1:error",
      revision: 6,
      severity: "error",
      resolved: false,
    })
  })

  it("keeps warnings active across a cancelled exit and settles them only on a clean end_turn", async () => {
    const handlers = await connectOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "session_failure",
      record: failure(
        "t1:error",
        1,
        "warning",
        "Reconnecting to Claude, attempt 1 of 5."
      ),
    })
    // Cancelled exit: not recovery — the amber strip must survive it.
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "turn_complete",
      session_id: "sess-1",
      stop_reason: "cancelled",
    })
    expect(h.store!.getConnection(TAB)?.sessionFailures?.[0]).toMatchObject({
      id: "t1:error",
      resolved: false,
    })

    // A later clean turn end is the recovery evidence that settles it.
    emitAcpEvent(handlers, {
      seq: 4,
      connection_id: "spawned-conn",
      type: "turn_complete",
      session_id: "sess-1",
      stop_reason: "end_turn",
    })
    expect(h.store!.getConnection(TAB)?.sessionFailures?.[0]).toMatchObject({
      id: "t1:error",
      resolved: true,
    })
  })

  // Issue #496: with `end_turn` as the only mid-flight settle point, a long
  // turn that reconnected N times stacked N permanent amber strips under the
  // composer. Turn PROGRESS settles the incident — codex's own
  // `completeRetryIncidentOnTurnProgress`.
  it("settles retry incidents as soon as the turn produces output again", async () => {
    const handlers = await connectOwner()
    const categorized = (id: string, category: string, severity: string) => ({
      id,
      revision: 1,
      category,
      severity,
      title: `${id} title`,
      actions: [],
    })
    const failuresNow = () => {
      const table = h.store!.getConnection(TAB)?.sessionFailures ?? []
      return Object.fromEntries(table.map((f) => [f.id, f.resolved]))
    }

    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "status_changed",
      status: "prompting",
    })
    for (const [i, rec] of [
      categorized("i1", "connection", "warning"),
      categorized("i2", "service", "warning"),
      // Informational, not an incident: codex config/skill-budget notices and
      // claude advisories both land on category "unknown". Progress must leave
      // them readable.
      categorized("notice", "unknown", "warning"),
      categorized("err", "connection", "error"),
    ].entries()) {
      emitAcpEvent(handlers, {
        seq: 2 + i,
        connection_id: "spawned-conn",
        type: "session_failure",
        record: rec,
      })
    }
    expect(failuresNow()).toEqual({
      i1: false,
      i2: false,
      notice: false,
      err: false,
    })

    emitAcpEvent(handlers, {
      seq: 10,
      connection_id: "spawned-conn",
      type: "content_delta",
      text: "back online",
    })
    expect(failuresNow()).toEqual({
      i1: true,
      i2: true,
      notice: false,
      err: false,
    })

    // A local tool call ADVANCING proves nothing about the upstream, so
    // `tool_call_update` deliberately does not settle.
    emitAcpEvent(handlers, {
      seq: 11,
      connection_id: "spawned-conn",
      type: "session_failure",
      record: categorized("i3", "limit", "warning"),
    })
    emitAcpEvent(handlers, {
      seq: 12,
      connection_id: "spawned-conn",
      type: "tool_call_update",
      tool_call_id: "call_1",
      title: "Bash",
      status: "in_progress",
      content: null,
      raw_input: null,
      raw_output: null,
    })
    expect(failuresNow().i3).toBe(false)

    // A NEW tool call is model output, so it does.
    emitAcpEvent(handlers, {
      seq: 13,
      connection_id: "spawned-conn",
      type: "tool_call",
      tool_call_id: "call_2",
      title: "Read",
      kind: "read",
      status: "pending",
      content: null,
      raw_input: null,
      raw_output: null,
    })
    expect(failuresNow().i3).toBe(true)

    // The notice still waits for the clean boundary; the error outlives it.
    emitAcpEvent(handlers, {
      seq: 14,
      connection_id: "spawned-conn",
      type: "turn_complete",
      session_id: "sess-1",
      stop_reason: "end_turn",
    })
    expect(failuresNow()).toMatchObject({ notice: true, err: false })
  })
})

// The composer's connection-status popover. Unlike `reapplyConfig` (live owners
// only), this has to work from EVERY state the icon can show — including the
// states where the store holds no entry at all.
describe("AcpConnectionsProvider reconnect (status-icon button)", () => {
  async function connectOwner() {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
  }

  it("restarts a live owner with the same identity", async () => {
    await connectOwner()
    h.acpConnect.mockResolvedValue("respawned-conn")

    let result: boolean | undefined
    await act(async () => {
      result = await h.actions!.reconnect(TAB)
    })

    expect(result).toBe(true)
    expect(h.acpDisconnect).toHaveBeenCalledWith("spawned-conn")
    // Same agent / cwd / session — the point is a fresh PROCESS, not new params,
    // which is exactly what connect()'s "nothing changed" fast path would skip.
    expect(h.acpConnect).toHaveBeenLastCalledWith(
      "claude_code",
      "/tmp/x",
      "sess-1",
      undefined,
      {}
    )
    expect(h.store!.getConnection(TAB)?.connectionId).toBe("respawned-conn")
  })

  it("rebuilds even when the backend no longer knows the connection", async () => {
    await connectOwner()
    // The single most important case for this button: the agent process is
    // already gone (reaped by another window, crashed, backend restarted), so
    // the teardown 404s. That must not abort the respawn.
    h.acpDisconnect.mockRejectedValue(new Error("Connection not found"))
    h.acpConnect.mockResolvedValue("respawned-conn")

    let result: boolean | undefined
    await act(async () => {
      result = await h.actions!.reconnect(TAB)
    })

    expect(result).toBe(true)
    expect(h.acpConnect).toHaveBeenLastCalledWith(
      "claude_code",
      "/tmp/x",
      "sess-1",
      undefined,
      {}
    )
    expect(h.store!.getConnection(TAB)?.connectionId).toBe("respawned-conn")
  })

  it("reconnects a tab whose connection is gone entirely", async () => {
    await connectOwner()
    await act(async () => {
      await h.actions!.disconnect(TAB)
    })
    expect(h.store!.getConnection(TAB)).toBeUndefined()
    h.acpConnect.mockClear()
    h.acpDisconnect.mockClear()

    let result: boolean | undefined
    await act(async () => {
      result = await h.actions!.reconnect(TAB)
    })

    expect(result).toBe(true)
    // Nothing to tear down — the params come from what connect() recorded.
    expect(h.acpDisconnect).not.toHaveBeenCalled()
    expect(h.acpConnect).toHaveBeenCalledWith(
      "claude_code",
      "/tmp/x",
      "sess-1",
      undefined,
      {}
    )
  })

  it("reconnects after a connect that never produced a connection", async () => {
    // The `error` state the icon shows for an agent that failed its preflight:
    // no store entry was ever created, so only the recorded params survive.
    h.acpGetAgentStatus.mockResolvedValue({
      agent_type: "claude_code",
      enabled: true,
      available: false,
      installed_version: null,
      host_tools_agent_mode: false,
      is_acp_adapter: true,
    })
    await mountProvider()
    await act(async () => {
      await h
        .actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
        .catch(() => {})
    })
    expect(h.acpConnect).not.toHaveBeenCalled()
    expect(h.actions!.getReconnectInfo(TAB)).toEqual({
      agentType: "claude_code",
      workingDir: "/tmp/x",
      sessionId: "sess-1",
    })

    h.acpGetAgentStatus.mockResolvedValue({
      agent_type: "claude_code",
      enabled: true,
      available: true,
      installed_version: "1.0.0",
      host_tools_agent_mode: false,
      is_acp_adapter: true,
    })
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await act(async () => {
      await h.actions!.reconnect(TAB)
    })

    expect(h.acpConnect).toHaveBeenCalledWith(
      "claude_code",
      "/tmp/x",
      "sess-1",
      undefined,
      {}
    )
  })

  it("re-attaches a viewer without killing the owner's agent", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "owner-conn",
      event_seq: 0,
    })
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    expect(h.store!.getConnection(TAB)?.isViewer).toBe(true)

    await act(async () => {
      await h.actions!.reconnect(TAB)
    })

    expect(h.acpDisconnect).not.toHaveBeenCalled()
    expect(h.acpConnect).not.toHaveBeenCalled()
    expect(h.store!.getConnection(TAB)?.isViewer).toBe(true)
  })

  it("is a no-op for a key that was never connected", async () => {
    await mountProvider()

    expect(h.actions!.getReconnectInfo(TAB)).toBeNull()
    let result: boolean | undefined
    await act(async () => {
      result = await h.actions!.reconnect(TAB)
    })

    expect(result).toBe(false)
    expect(h.acpConnect).not.toHaveBeenCalled()
    expect(h.acpDisconnect).not.toHaveBeenCalled()
  })

  it("refuses a delegation child — the broker owns its lifetime", async () => {
    await mountProvider()
    act(() => {
      h.actions!.attachDelegationChild({
        connectionId: "child-conn",
        parentConnectionId: "parent-conn",
        parentToolUseId: "tool-1",
        agentType: "claude_code",
      })
    })
    expect(h.store!.getConnection("child-conn")?.isDelegationChild).toBe(true)

    let result: boolean | undefined
    await act(async () => {
      result = await h.actions!.reconnect("child-conn")
    })

    expect(result).toBe(false)
    expect(h.actions!.getReconnectInfo("child-conn")).toBeNull()
    expect(h.acpDisconnect).not.toHaveBeenCalled()
  })

  it("forgets remembered params on disconnectAll, so a recycled key can't resurrect the old session", async () => {
    await connectOwner()
    await act(async () => {
      await h.actions!.disconnectAll()
    })

    expect(h.actions!.getReconnectInfo(TAB)).toBeNull()
  })

  it("still rebuilds when a connect for the same key is already in flight", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()

    let releaseConnect: (connectionId: string) => void = () => {}
    h.acpConnect.mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          releaseConnect = resolve
        })
    )

    let firstConnect: Promise<void> | undefined
    await act(async () => {
      firstConnect = h.actions!.connect(
        TAB,
        "claude_code",
        "/tmp/x",
        "sess-1",
        42
      )
      await Promise.resolve()
    })

    // The state users actually click this button in: the connect is HUNG, so
    // there is no store entry to tear down and the params are identical.
    // connect() parks a same-parameter request as pending and its `finally`
    // then drops it as a duplicate — so this used to spin the button once and
    // change nothing at all.
    h.acpConnect.mockResolvedValue("respawned-conn")
    let reconnectResult: Promise<boolean> | undefined
    await act(async () => {
      reconnectResult = h.actions!.reconnect(TAB)
      await Promise.resolve()
    })

    await act(async () => {
      releaseConnect("spawned-conn")
      await firstConnect
      await reconnectResult
    })

    expect(await reconnectResult).toBe(true)
    // Waited for the hung attempt to settle, then rebuilt what it produced.
    expect(h.acpDisconnect).toHaveBeenCalledWith("spawned-conn")
    expect(h.acpConnect).toHaveBeenLastCalledWith(
      "claude_code",
      "/tmp/x",
      "sess-1",
      undefined,
      {}
    )
    expect(h.store!.getConnection(TAB)?.connectionId).toBe("respawned-conn")
  })

  it("gives the button back when the in-flight connect never answers", async () => {
    vi.useFakeTimers()
    try {
      h.acpFindConnectionForConversation.mockResolvedValue(null)
      await mountProvider()
      // Never resolves: a wedged IPC, which is a state users click Reconnect
      // from. Waiting on it unbounded would spin the button forever.
      h.acpConnect.mockImplementationOnce(() => new Promise<string>(() => {}))
      await act(async () => {
        void h
          .actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
          .catch(() => {})
        await Promise.resolve()
      })

      let settled: boolean | undefined
      const pending = h.actions!.reconnect(TAB).then((r) => {
        settled = r
        return r
      })
      await act(async () => {
        await vi.advanceTimersByTimeAsync(15_000)
      })
      await act(async () => {
        await pending
      })

      // Reports "nothing happened" rather than hanging — the user can retry.
      expect(settled).toBe(false)
    } finally {
      vi.useRealTimers()
    }
  })

  it("resumes a session known only from a snapshot hydrate (cold attach)", async () => {
    // The event that carries the sessionId fired BEFORE this client attached,
    // so it is never replayed — the snapshot is the only place identity
    // appears, and it lands on the store entry alone.
    h.denormalizeSnapshot.mockReturnValue({
      connectionId: "spawned-conn",
      status: "connected",
      sessionId: "snapshot-session",
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
      eventSeq: 7,
      activeDelegations: [],
    })
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", undefined, 42)
    })
    hydrateSnapshot(latestAttachHandlers(), {
      event_seq: 7,
    } as unknown as LiveSessionSnapshot)
    expect(h.store!.getConnection(TAB)?.sessionId).toBe("snapshot-session")

    await act(async () => {
      await h.actions!.disconnect(TAB)
    })
    expect(h.actions!.getReconnectInfo(TAB)?.sessionId).toBe("snapshot-session")

    h.acpConnect.mockClear()
    await act(async () => {
      await h.actions!.reconnect(TAB)
    })

    expect(h.acpConnect).toHaveBeenCalledWith(
      "claude_code",
      "/tmp/x",
      "snapshot-session",
      undefined,
      {}
    )
  })

  it("resumes the session the BACKEND minted once the entry is gone", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()
    // A new conversation connects with no sessionId at all — the backend mints
    // one later, and it only ever lands on the store entry.
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", undefined, 42)
    })
    emitAcpEvent(latestAttachHandlers(), {
      seq: 1,
      connection_id: "spawned-conn",
      type: "session_started",
      session_id: "minted-1",
    })
    expect(h.store!.getConnection(TAB)?.sessionId).toBe("minted-1")

    // Whatever removes the entry (backend GC via connection_gone, the idle
    // sweep, the unmount cleanup) leaves only the recorded params behind.
    await act(async () => {
      await h.actions!.disconnect(TAB)
    })
    expect(h.store!.getConnection(TAB)).toBeUndefined()
    expect(h.actions!.getReconnectInfo(TAB)?.sessionId).toBe("minted-1")

    h.acpConnect.mockClear()
    await act(async () => {
      await h.actions!.reconnect(TAB)
    })

    // Reconnecting on the request AS ISSUED would pass sessionId undefined —
    // a brand-new ACP session, silently abandoning the conversation's history.
    expect(h.acpConnect).toHaveBeenCalledWith(
      "claude_code",
      "/tmp/x",
      "minted-1",
      undefined,
      {}
    )
  })
})

// The local entry is always released — a stranded one sends the next connect()
// down its "already connected" fast path onto a possibly-dead session — but a
// teardown that did NOT happen must not be reported as one.
describe("AcpConnectionsProvider disconnect teardown confirmation", () => {
  async function connectOwner() {
    h.acpFindConnectionForConversation.mockResolvedValue(null)
    await mountProvider()
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
  }

  it("counts an already-gone connection as a real teardown", async () => {
    await connectOwner()
    h.acpDisconnect.mockRejectedValue(new Error("connection not found: abc"))

    let confirmed: boolean | undefined
    await act(async () => {
      confirmed = await h.actions!.disconnect(TAB)
    })

    // Nothing is left running, so there is nothing to warn about.
    expect(confirmed).toBe(true)
    expect(h.store!.getConnection(TAB)).toBeUndefined()
  })

  it("reports an unconfirmed teardown, and reapplyConfig stops claiming success", async () => {
    await connectOwner()
    // reapplyConfig resumes off the LIVE entry's session, which the backend
    // only supplies here.
    emitAcpEvent(latestAttachHandlers(), {
      seq: 1,
      connection_id: "spawned-conn",
      type: "session_started",
      session_id: "sess-1",
    })
    // A transport blip, not a missing connection: the agent process may still
    // be alive and still holding the OLD config.
    h.acpDisconnect.mockRejectedValue(new Error("request timed out"))
    h.acpConnect.mockResolvedValue("respawned-conn")

    let applied: boolean | undefined
    await act(async () => {
      applied = await h.actions!.reapplyConfig(TAB)
    })

    // Still reconnected — the user is not left stranded...
    expect(h.acpConnect).toHaveBeenLastCalledWith(
      "claude_code",
      "/tmp/x",
      "sess-1",
      undefined,
      {}
    )
    // ...but the caller must not show an "applied" confirmation for a restart
    // that may have landed right back on the process it meant to replace.
    expect(applied).toBe(false)
  })

  it("confirms an ordinary teardown", async () => {
    await connectOwner()

    let confirmed: boolean | undefined
    await act(async () => {
      confirmed = await h.actions!.disconnect(TAB)
    })

    expect(confirmed).toBe(true)
    expect(h.acpDisconnect).toHaveBeenCalledWith("spawned-conn")
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

    // 1. outstanding mirrored onto the connection (teardown gates only —
    //    nothing renders the count).
    expect(h.store!.getConnection(TAB)?.backgroundOutstanding).toBe(2)

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
      host_tools_agent_mode: false,
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

  it("reports a rejected pick, and only when the backend says so", async () => {
    // The backend owns the request↔answer correlation (`ConfigOptionRejected`):
    // `acpSetConfigOption` resolves once the command is merely QUEUED, and the
    // resulting option list is broadcast as an ordinary update — so this side
    // must never try to infer a verdict from an option snapshot.
    const handlers = await connectGrokOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })

    h.toastWarning.mockClear()
    await act(async () => {
      await h.actions!.setConfigOption(TAB, "model", "grok-composer-2.5-fast")
    })
    // The pick alone reports nothing — the agent may still honour it.
    expect(h.toastWarning).not.toHaveBeenCalled()

    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "config_option_rejected",
      config_id: "model",
      option_name: "Model",
      requested: "Composer 2.5",
      actual: "Grok 4.5",
    })
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })

    expect(h.toastWarning).toHaveBeenCalledTimes(1)
    // The useTranslations mock echoes the key, so the message itself is the key.
    expect(h.toastWarning).toHaveBeenCalledWith("configOptionAdjusted")
  })

  it("stays silent for option snapshots nobody asked for", async () => {
    // codex-acp flips `collaboration_mode` mid-turn, and pi answers one
    // `set_config_option` with TWO snapshots (response + notification). None of
    // those is a verdict; only an explicit rejection event is.
    const handlers = await connectGrokOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })

    h.toastWarning.mockClear()
    await act(async () => {
      await h.actions!.setConfigOption(TAB, "model", "grok-composer-2.5-fast")
    })
    // Honoured, then a spontaneous switch back, then a duplicate echo.
    for (const [seq, value] of [
      [2, "grok-composer-2.5-fast"],
      [3, "grok-4.5"],
      [4, "grok-4.5"],
    ] as const) {
      emitAcpEvent(handlers, {
        seq,
        connection_id: "spawned-conn",
        type: "session_config_options",
        config_options: grokModelOptions(value),
      })
    }

    expect(h.toastWarning).not.toHaveBeenCalled()
  })

  it("does not strand a verdict when the set fails outright", async () => {
    // A failed `set_config_option` emits only a recoverable Error — no option
    // snapshot at all. Nothing may linger to be charged against a later,
    // unrelated update.
    const handlers = await connectGrokOwner()
    emitAcpEvent(handlers, {
      seq: 1,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })

    h.toastWarning.mockClear()
    await act(async () => {
      await h.actions!.setConfigOption(TAB, "model", "grok-composer-2.5-fast")
    })
    emitAcpEvent(handlers, {
      seq: 2,
      connection_id: "spawned-conn",
      type: "error",
      message: "Failed to set config option: boom",
      agent_type: "grok",
      code: null,
    })
    emitAcpEvent(handlers, {
      seq: 3,
      connection_id: "spawned-conn",
      type: "session_config_options",
      config_options: grokModelOptions("grok-4.5"),
    })

    expect(h.toastWarning).not.toHaveBeenCalled()
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

  it("routes details to the alert's evidence slot, keeping them out of detail, conn.error and the OS notification", async () => {
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

    // The evidence rides its own slot so `StatusBarAlerts` can put it behind an
    // expander; the always-visible detail slot stays the localized line.
    const alertCalls = h.pushAlert.mock.calls
    const [, , alertDetail, , alertEvidence] =
      alertCalls[alertCalls.length - 1]!
    expect(alertDetail).toBe("backendErrors.turnFailedEmptyProtocol")
    expect(alertEvidence).toBe(details)

    // `conn.error` feeds the composer tooltip — the localized line plus a
    // pointer at the only surface that can expand the evidence, never the
    // evidence itself.
    expect(h.store!.getConnection(TAB)!.error).toBe(
      "backendErrors.turnFailedEmptyProtocol backendErrors.detailsInAlerts"
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
    const [, , alertDetail, , alertEvidence] =
      alertCalls[alertCalls.length - 1]!
    expect(alertDetail).toBe("backendErrors.turnFailedEmpty")
    expect(alertEvidence).toBeUndefined()
    // Nothing to expand, so the tooltip must not send the user looking for an
    // expander.
    expect(h.store!.getConnection(TAB)!.error).toBe(
      "backendErrors.turnFailedEmpty"
    )
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
      host_tools_agent_mode: false,
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
    const [, , alertDetail, , alertEvidence] =
      alertCalls[alertCalls.length - 1]!
    expect(alertDetail).toBe(
      "agent ended the turn without producing any response."
    )
    expect(alertEvidence).toBe(details)
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

// ── Stuck-on-"responding" regression suite ────────────────────────────────────
//
// Reported as: run a board task, open its conversation from the sidebar after
// it finished, and the panel streams forever — "responding" in the composer,
// Stop does nothing. The task card's "view session" showed the right state.
//
// Root cause: the desktop firehose reverse map was `connectionId -> contextKey`,
// 1:1, while SEVERAL surfaces can watch one connection (a conversation tab plus
// the work-task transcript viewer, which attaches the task's own connection
// through `attachDelegationChild`, or two split tiles). The last attach stole
// routing; the first detach deleted it. The blinded surface then kept a
// `prompting` ConnectionState that nothing could ever settle — the idle sweep
// skips `prompting`, `connect()` treats any non-terminal status as "already
// connected", and `cancel()` swallowed the backend's "connection not found".
describe("routing survives every surface watching one connection", () => {
  const CHILD_VIEW = "task-conn-1" // transcript viewer: contextKey === connId
  const CONN = "task-conn-1"
  let unlisten: ReturnType<typeof vi.fn>

  function mountDesktop() {
    return render(
      <AcpConnectionsProvider>
        <Probe />
      </AcpConnectionsProvider>
    )
  }

  function firehose() {
    return vi.mocked(subscribe).mock.calls[0]![1] as (
      envelope: EventEnvelope
    ) => void
  }

  beforeEach(() => {
    h.eventStreamValue = null // desktop firehose path
    unlisten = vi.fn()
    vi.mocked(subscribe).mockClear()
    vi.mocked(subscribe).mockImplementation(async () => unlisten)
    // The discovered connection is genuinely alive, so its snapshot resolves.
    // (A `null` snapshot means "already gone" and is covered separately.)
    h.acpGetSessionSnapshot.mockResolvedValue({
      connection_id: CONN,
    } as unknown as LiveSessionSnapshot)
    h.denormalizeSnapshot.mockReturnValue({
      connectionId: CONN,
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
  })

  afterEach(() => {
    vi.mocked(subscribe).mockImplementation(async () => () => {})
  })

  it("keeps the tab streaming after the transcript viewer detaches", async () => {
    // The tab views the task's live connection...
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: CONN,
      event_seq: 0,
    })
    mountDesktop()
    await act(async () => {})

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    // ...as a VIEWER, not an owner: a delegation-child/viewer entry for the same
    // id must never be mistaken for local ownership, or closing the tab would
    // acpDisconnect the connection and kill the running task's agent.
    expect(h.acpConnect).not.toHaveBeenCalled()
    expect(h.store!.getConnection(TAB)!.isViewer).toBe(true)

    // ...and the task card's transcript dialog opens on the SAME connection.
    act(() => {
      h.actions!.attachDelegationChild({
        connectionId: CONN,
        parentConnectionId: CONN,
        parentToolUseId: "",
        agentType: "claude_code",
      })
    })

    const onEvent = firehose()
    act(() => {
      onEvent({
        seq: 1,
        connection_id: CONN,
        type: "status_changed",
        status: "prompting",
      } as EventEnvelope)
    })
    // Both surfaces see the turn — the second attach didn't steal the first's
    // route.
    expect(h.store!.getConnection(TAB)!.status).toBe("prompting")
    expect(h.store!.getConnection(CHILD_VIEW)!.status).toBe("prompting")

    // Closing the dialog must not blind the tab.
    act(() => {
      h.actions!.detachDelegationChild(CONN)
    })
    expect(h.store!.getConnection(CHILD_VIEW)).toBeUndefined()

    // The turn ends. THIS is the event that used to go missing.
    act(() => {
      onEvent({
        seq: 2,
        connection_id: CONN,
        type: "turn_complete",
        stop_reason: "end_turn",
      } as EventEnvelope)
    })
    expect(h.store!.getConnection(TAB)!.status).toBe("connected")
  })

  it("delivers one envelope to every routed surface exactly once", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: CONN,
      event_seq: 0,
    })
    mountDesktop()
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    act(() => {
      h.actions!.attachDelegationChild({
        connectionId: CONN,
        parentConnectionId: CONN,
        parentToolUseId: "",
        agentType: "claude_code",
      })
    })

    const onEvent = firehose()
    act(() => {
      onEvent({
        seq: 1,
        connection_id: CONN,
        type: "status_changed",
        status: "prompting",
      } as EventEnvelope)
      onEvent({
        seq: 2,
        connection_id: CONN,
        type: "content_delta",
        text: "hi",
      } as EventEnvelope)
      // Replay of an already-applied seq: deduped per surface, not re-applied.
      onEvent({
        seq: 2,
        connection_id: CONN,
        type: "content_delta",
        text: "hi",
      } as EventEnvelope)
    })
    await act(async () => {
      await new Promise((r) => setTimeout(r, 32))
    })

    const textOf = (key: string) =>
      (
        h.store!.getConnection(key)!.liveMessage!.content as Array<{
          type: string
          text?: string
        }>
      )
        .filter((b) => b.type === "text")
        .map((b) => b.text ?? "")
        .join("")
    expect(textOf(TAB)).toBe("hi")
    expect(textOf(CHILD_VIEW)).toBe("hi")
  })

  it("raises envelope-scoped effects once, not once per surface", async () => {
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: CONN,
      event_seq: 0,
    })
    mountDesktop()
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })
    act(() => {
      h.actions!.attachDelegationChild({
        connectionId: CONN,
        parentConnectionId: CONN,
        parentToolUseId: "",
        agentType: "claude_code",
      })
    })
    h.sendSystemNotification.mockClear()

    act(() => {
      firehose()({
        seq: 1,
        connection_id: CONN,
        type: "turn_complete",
        stop_reason: "end_turn",
      } as EventEnvelope)
    })

    // Two surfaces are streaming the same turn; the user must still get ONE
    // "finished responding" notification. The store effect, by contrast, is
    // per surface — both leave `prompting`.
    expect(h.sendSystemNotification).toHaveBeenCalledTimes(1)
    expect(h.store!.getConnection(TAB)!.status).toBe("connected")
    expect(h.store!.getConnection(CHILD_VIEW)!.status).toBe("connected")
  })
})

describe("a `prompting` state whose connection is gone can always recover", () => {
  let unlisten: ReturnType<typeof vi.fn>

  function mountDesktop() {
    return render(
      <AcpConnectionsProvider>
        <Probe />
      </AcpConnectionsProvider>
    )
  }

  beforeEach(() => {
    h.eventStreamValue = null
    unlisten = vi.fn()
    vi.mocked(subscribe).mockClear()
    vi.mocked(subscribe).mockImplementation(async () => unlisten)
  })

  afterEach(() => {
    vi.mocked(subscribe).mockImplementation(async () => () => {})
  })

  /** Drive a tab into `prompting` on a connection it owns. */
  async function seedPromptingTab() {
    mountDesktop()
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
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
    })
    expect(h.store!.getConnection(TAB)!.status).toBe("prompting")
  }

  it("re-opening the conversation revives it instead of returning early", async () => {
    await seedPromptingTab()
    // The backend disowned the connection while the frontend missed the
    // terminal event (a board task disconnects within ~3ms of TurnComplete).
    h.acpTouchConnection.mockResolvedValue(false)
    h.acpConnect.mockResolvedValue("respawned-conn")

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    // Before the fix this connect() hit the "same params + non-terminal status"
    // fast return and the tab stayed on "responding" forever.
    expect(h.acpTouchConnection).toHaveBeenCalledWith("spawned-conn")
    expect(h.acpConnect).toHaveBeenCalledTimes(2)
    expect(h.store!.getConnection(TAB)!.connectionId).toBe("respawned-conn")
  })

  it("leaves a live turn alone", async () => {
    await seedPromptingTab()
    h.acpTouchConnection.mockResolvedValue(true)

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    // Still streaming: no re-spawn, no state churn.
    expect(h.acpConnect).toHaveBeenCalledTimes(1)
    expect(h.store!.getConnection(TAB)!.status).toBe("prompting")
  })

  it("treats a probe failure as alive (never settles on a flaky IPC)", async () => {
    await seedPromptingTab()
    h.acpTouchConnection.mockRejectedValue(new Error("ipc down"))

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    expect(h.acpConnect).toHaveBeenCalledTimes(1)
    expect(h.store!.getConnection(TAB)!.status).toBe("prompting")
  })

  it("Stop settles the state when the backend disowns the connection", async () => {
    await seedPromptingTab()
    h.acpCancel.mockRejectedValue(new Error("Connection not found: x"))

    await act(async () => {
      await h.actions!.cancel(TAB)
    })

    // Previously the rejection was logged and the button appeared dead.
    expect(h.store!.getConnection(TAB)!.status).toBe("disconnected")
  })

  it("keeps the entry (with its identity) so a reconnect resumes the session", async () => {
    await seedPromptingTab()
    h.acpCancel.mockRejectedValue(new Error("Connection not found: x"))
    await act(async () => {
      await h.actions!.cancel(TAB)
    })

    const info = h.actions!.getReconnectInfo(TAB)
    expect(info).not.toBeNull()
    expect(info!.agentType).toBe("claude_code")
    expect(info!.sessionId).toBe("sess-1")
  })

  it("spawns instead of stranding a viewer when the connection dies mid-attach", async () => {
    // Discovery saw a live connection, but by the time the snapshot request
    // landed the owner had torn it down: `acp_get_session_snapshot` answers
    // `null`. Attaching anyway froze the tab on `connecting` forever.
    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "dying-conn",
      event_seq: 3,
    })
    h.acpGetSessionSnapshot.mockResolvedValue(null)
    h.acpConnect.mockResolvedValue("fresh-conn")
    mountDesktop()
    await act(async () => {})

    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })

    expect(h.acpConnect).toHaveBeenCalledTimes(1)
    const conn = h.store!.getConnection(TAB)!
    expect(conn.connectionId).toBe("fresh-conn")
    expect(conn.isViewer).toBe(false)
  })
})

// Teardown races around the awaits inside `connect()`. Every one of them can
// otherwise resurrect a surface the caller just closed, or kill a connection
// another surface is watching.
describe("connect() teardown races", () => {
  // A second contextKey for the same session — what a tab close + sidebar
  // reopen produces, and what orphan rescue exists to reconcile.
  const RESCUE_TAB = "conv-1-claude_code-42-reopened"
  let unlisten: ReturnType<typeof vi.fn>

  function mountDesktop() {
    return render(
      <AcpConnectionsProvider>
        <Probe />
      </AcpConnectionsProvider>
    )
  }

  beforeEach(() => {
    h.eventStreamValue = null
    unlisten = vi.fn()
    vi.mocked(subscribe).mockClear()
    vi.mocked(subscribe).mockImplementation(async () => unlisten)
  })

  afterEach(() => {
    vi.mocked(subscribe).mockImplementation(async () => () => {})
  })

  it("abandons a connect whose tab is disconnected during the liveness probe", async () => {
    // Seed a `prompting` entry so the next connect() takes the probe path.
    mountDesktop()
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
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
    })
    h.acpConnect.mockClear()

    // The probe hangs; the user closes the tab meanwhile. `disconnect()` takes
    // its ENTRY-EXISTS branch here, which used to skip the abandon mark — so
    // connect() sailed on and spawned an agent for a tab that was gone.
    let resolveProbe: (v: boolean) => void = () => {}
    h.acpTouchConnection.mockImplementation(
      () =>
        new Promise<boolean>((res) => {
          resolveProbe = res
        })
    )
    let connectPromise: Promise<void> | undefined
    await act(async () => {
      connectPromise = h.actions!.connect(
        TAB,
        "claude_code",
        "/tmp/x",
        "sess-1"
      )
    })
    await act(async () => {
      await h.actions!.disconnect(TAB)
    })
    await act(async () => {
      resolveProbe(false)
      await connectPromise
    })

    expect(h.acpConnect).not.toHaveBeenCalled()
    expect(h.store!.getConnection(TAB)).toBeUndefined()
  })

  it("spares a connection only a transcript viewer references", async () => {
    // The work-task transcript dialog is watching the task's connection.
    mountDesktop()
    await act(async () => {})
    act(() => {
      h.actions!.attachDelegationChild({
        connectionId: "task-conn",
        parentConnectionId: "task-conn",
        parentToolUseId: "",
        agentType: "claude_code",
      })
    })

    // A tab connect resolves to that SAME connection (backend dedup) but is
    // abandoned before it settles. Tearing it down would kill the running
    // task's agent out from under the dialog — a viewer/child is not OWNERSHIP,
    // but it is very much a local reference.
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
        TAB,
        "claude_code",
        "/tmp/x",
        "sess-1"
      )
    })
    await act(async () => {
      await h.actions!.disconnect(TAB)
    })
    await act(async () => {
      resolveConnect("task-conn")
      await connectPromise
    })

    expect(h.acpDisconnect).not.toHaveBeenCalled()
    expect(h.store!.getConnection("task-conn")).toBeTruthy()
  })

  it("installs no route when the owner snapshot outlives its tab", async () => {
    let resolveSnapshot: (v: unknown) => void = () => {}
    h.acpGetSessionSnapshot.mockImplementation(
      () =>
        new Promise((res) => {
          resolveSnapshot = res
        })
    )
    mountDesktop()
    await act(async () => {})

    let connectPromise: Promise<void> | undefined
    await act(async () => {
      connectPromise = h.actions!.connect(
        TAB,
        "claude_code",
        "/tmp/x",
        "sess-1"
      )
    })
    // Entry exists (CONNECTION_CREATED ran); tear it down mid-snapshot.
    await act(async () => {
      await h.actions!.disconnect(TAB)
    })
    await act(async () => {
      resolveSnapshot(null)
      await connectPromise
    })

    expect(h.store!.getConnection(TAB)).toBeUndefined()

    // A route installed now would belong to no entry: nothing releases it, and
    // every envelope it swallows is lost — the reducer no-ops on a missing
    // entry and the unmapped buffer never sees it. Prove the event is still
    // buffered by attaching a fresh surface and watching it drain.
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
    })

    h.acpFindConnectionForConversation.mockResolvedValue({
      connection_id: "spawned-conn",
      event_seq: 0,
    })
    h.acpGetSessionSnapshot.mockResolvedValue({
      connection_id: "spawned-conn",
    } as unknown as LiveSessionSnapshot)
    h.denormalizeSnapshot.mockReturnValue({
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
      lastError: null,
      eventSeq: 0,
      activeDelegations: [],
    })
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1", 42)
    })

    // Snapshot says `connected` at seq 0; the drained seq-1 event is what makes
    // it `prompting`. Without the guard that event went to the dead route.
    expect(h.store!.getConnection(TAB)!.status).toBe("prompting")
  })

  it("leaves a connection rekeyed away during the probe under its new key", async () => {
    // TAB owns a streaming connection...
    mountDesktop()
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })
    const onEvent = vi.mocked(subscribe).mock.calls[0]![1] as (
      envelope: EventEnvelope
    ) => void
    act(() => {
      onEvent({
        seq: 1,
        connection_id: "spawned-conn",
        type: "session_started",
        session_id: "sess-1",
      } as EventEnvelope)
      onEvent({
        seq: 2,
        connection_id: "spawned-conn",
        type: "status_changed",
        status: "prompting",
      } as EventEnvelope)
    })
    h.acpConnect.mockClear()

    // ...a re-open of TAB starts and hangs on the liveness probe...
    let resolveProbe: (v: boolean) => void = () => {}
    h.acpTouchConnection.mockImplementation(
      () =>
        new Promise<boolean>((res) => {
          resolveProbe = res
        })
    )
    let stalePromise: Promise<void> | undefined
    await act(async () => {
      stalePromise = h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    // ...and meanwhile the canonical key claims the connection by orphan
    // rescue (tab close + sidebar reopen mints a different contextKey).
    h.acpTouchConnection.mockResolvedValue(true)
    await act(async () => {
      await h.actions!.connect(RESCUE_TAB, "claude_code", "/tmp/x", "sess-1")
    })
    expect(h.store!.getConnection(RESCUE_TAB)?.connectionId).toBe(
      "spawned-conn"
    )
    expect(h.store!.getConnection(TAB)).toBeUndefined()

    // The stale connect resumes. Its own orphan rescue would find the
    // connection under RESCUE_TAB and drag it back to the key it just left.
    await act(async () => {
      resolveProbe(true)
      await stalePromise
    })

    expect(h.store!.getConnection(RESCUE_TAB)?.connectionId).toBe(
      "spawned-conn"
    )
    expect(h.store!.getConnection(TAB)).toBeUndefined()
    expect(h.acpConnect).not.toHaveBeenCalled()
  })

  it("still connects when the backend GC'd the connection mid-probe", async () => {
    // Web/attach transport: `onDetached("connection_gone")` drops the entry
    // outright. That is NOT a rekey — nothing else holds the connection — so
    // the connect must go on and build one. Bailing here would leave the tab
    // with no connection and no retry: the auto-connect effect only re-fires on
    // [isActive, workingDir, agentType], never on a status change.
    h.eventStreamValue = h.stream
    render(
      <AcpConnectionsProvider>
        <Probe />
      </AcpConnectionsProvider>
    )
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })
    const handlers = latestAttachHandlers()
    act(() => {
      handlers.onEvent({
        seq: 1,
        connection_id: "spawned-conn",
        type: "status_changed",
        status: "prompting",
      } as EventEnvelope)
    })
    expect(h.store!.getConnection(TAB)!.status).toBe("prompting")

    let resolveProbe: (v: boolean) => void = () => {}
    h.acpTouchConnection.mockImplementation(
      () =>
        new Promise<boolean>((res) => {
          resolveProbe = res
        })
    )
    h.acpConnect.mockResolvedValue("rebuilt-conn")
    let connectPromise: Promise<void> | undefined
    await act(async () => {
      connectPromise = h.actions!.connect(
        TAB,
        "claude_code",
        "/tmp/x",
        "sess-1"
      )
    })
    act(() => {
      handlers.onDetached("connection_gone")
    })
    expect(h.store!.getConnection(TAB)).toBeUndefined()

    await act(async () => {
      resolveProbe(false)
      await connectPromise
    })

    expect(h.store!.getConnection(TAB)?.connectionId).toBe("rebuilt-conn")
  })

  it("still connects when a sibling surface happens to share the dead id", async () => {
    // Backend dedup legitimately hands the same connection to two surfaces. If
    // OUR key's subscription reports `connection_gone` first, the sibling entry
    // is merely stale — it is NOT evidence that our entry was rekeyed, so we
    // must still rebuild. Inferring the reason from "who references this id"
    // got this wrong and left both tabs entryless with no automatic retry.
    h.eventStreamValue = h.stream
    render(
      <AcpConnectionsProvider>
        <Probe />
      </AcpConnectionsProvider>
    )
    await act(async () => {})
    // Sibling surface on the same backend connection.
    await act(async () => {
      await h.actions!.connect(RESCUE_TAB, "claude_code", "/tmp/x", "sess-1")
    })
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })
    const handlers = latestAttachHandlers()
    act(() => {
      handlers.onEvent({
        seq: 1,
        connection_id: "spawned-conn",
        type: "status_changed",
        status: "prompting",
      } as EventEnvelope)
    })

    let resolveProbe: (v: boolean) => void = () => {}
    h.acpTouchConnection.mockImplementation(
      () =>
        new Promise<boolean>((res) => {
          resolveProbe = res
        })
    )
    h.acpConnect.mockResolvedValue("rebuilt-conn")
    let connectPromise: Promise<void> | undefined
    await act(async () => {
      connectPromise = h.actions!.connect(
        TAB,
        "claude_code",
        "/tmp/x",
        "sess-1"
      )
    })
    act(() => {
      handlers.onDetached("connection_gone")
    })
    // RESCUE_TAB still references "spawned-conn" — it hasn't detached yet.
    expect(h.store!.getConnection(RESCUE_TAB)?.connectionId).toBe(
      "spawned-conn"
    )
    expect(h.store!.getConnection(TAB)).toBeUndefined()

    await act(async () => {
      resolveProbe(false)
      await connectPromise
    })

    expect(h.store!.getConnection(TAB)?.connectionId).toBe("rebuilt-conn")
  })

  it("stands down after a rekey even if the destination is torn down too", async () => {
    // A is rekeyed to B, then B is closed before A's probe resumes. No entry
    // holds the connection any more, but A still must NOT resurrect itself:
    // the user rekeyed it away and then closed it.
    mountDesktop()
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })
    const onEvent = vi.mocked(subscribe).mock.calls[0]![1] as (
      envelope: EventEnvelope
    ) => void
    act(() => {
      onEvent({
        seq: 1,
        connection_id: "spawned-conn",
        type: "session_started",
        session_id: "sess-1",
      } as EventEnvelope)
      onEvent({
        seq: 2,
        connection_id: "spawned-conn",
        type: "status_changed",
        status: "prompting",
      } as EventEnvelope)
    })
    h.acpConnect.mockClear()

    let resolveProbe: (v: boolean) => void = () => {}
    h.acpTouchConnection.mockImplementation(
      () =>
        new Promise<boolean>((res) => {
          resolveProbe = res
        })
    )
    let stalePromise: Promise<void> | undefined
    await act(async () => {
      stalePromise = h.actions!.connect(TAB, "claude_code", "/tmp/x", "sess-1")
    })

    h.acpTouchConnection.mockResolvedValue(true)
    await act(async () => {
      await h.actions!.connect(RESCUE_TAB, "claude_code", "/tmp/x", "sess-1")
    })
    await act(async () => {
      await h.actions!.disconnect(RESCUE_TAB)
    })
    expect(h.store!.getConnection(RESCUE_TAB)).toBeUndefined()

    await act(async () => {
      resolveProbe(true)
      await stalePromise
    })

    expect(h.acpConnect).not.toHaveBeenCalled()
    expect(h.store!.getConnection(TAB)).toBeUndefined()
  })

  it("still establishes the replacement reapplyConfig asked for", async () => {
    mountDesktop()
    await act(async () => {})
    await act(async () => {
      await h.actions!.connect(TAB, "claude_code", "/tmp/x")
    })
    expect(h.store!.getConnection(TAB)?.connectionId).toBe("spawned-conn")

    // A same-parameter connect re-fires (the auto-connect effect does this on
    // any dep change) and parks on its preflight.
    let resolvePreflight: (v: unknown) => void = () => {}
    h.acpGetAgentStatus.mockImplementation(
      () =>
        new Promise((res) => {
          resolvePreflight = res
        })
    )
    let inflight: Promise<void> | undefined
    await act(async () => {
      inflight = h.actions!.connect(TAB, "claude_code", "/tmp/x")
    })

    // The user hits "restart to apply" on top of it. `reapplyConfig` tears the
    // entry down — which abandons the parked generation — then asks for the
    // SAME connection back. That request can only be queued, because the
    // abandoned generation still holds the key.
    h.acpConnect.mockResolvedValue("replacement-conn")
    let reapply: Promise<boolean> | undefined
    await act(async () => {
      reapply = h.actions!.reapplyConfig(TAB)
      await reapply
    })
    expect(h.store!.getConnection(TAB)).toBeUndefined()

    // Let the abandoned generation finish. Its finalizer used to drop the
    // queued request as a "duplicate" of the connection it never established,
    // leaving the tab with nothing at all.
    h.acpGetAgentStatus.mockImplementation(async () => ({
      agent_type: "claude_code",
      enabled: true,
      available: true,
      installed_version: "1.0.0",
      host_tools_agent_mode: false,
      is_acp_adapter: true,
    }))
    await act(async () => {
      resolvePreflight({
        agent_type: "claude_code",
        enabled: true,
        available: true,
        installed_version: "1.0.0",
        host_tools_agent_mode: false,
        is_acp_adapter: true,
      })
      await inflight
    })
    // The re-dispatch runs on a microtask, then awaits its own preflight.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0))
    })

    expect(h.store!.getConnection(TAB)?.connectionId).toBe("replacement-conn")
  })
})
