import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi, type Mock } from "vitest"

import type { AgentType, DbConversationSummary, MessageTurn } from "@/lib/types"
import type { ConversationTimelineTurn } from "@/stores/conversation-runtime-store"
import type { DelegationBinding } from "@/contexts/delegation-context"
import type { LiveContentBlock } from "@/contexts/acp-connections-context"
import enMessages from "@/i18n/messages/en.json"

// Regression guard: the branch selector + command launcher were REMOVED from
// this tab — they live in the bottom status bar now, on every platform. Stub
// them so that if either is ever re-added here, its test id surfaces and the
// "no actions bar" assertions below fail.
vi.mock("./branch-dropdown", () => ({
  BranchDropdown: () => <div data-testid="branch-dropdown" />,
}))
vi.mock("./command-dropdown", () => ({
  CommandDropdown: () => <div data-testid="command-dropdown" />,
}))
// The agent icon renders inline SVG with a <title> that duplicates the label.
vi.mock("@/components/agent-icon", () => ({ AgentIcon: () => null }))
// Stats are always supplied here, so the cold-fetch path must never fire; stub
// the API so an accidental call is inert rather than a real transport hit.
// `openNativeSubagentSession` is the row's click path — each native-row test
// scripts it explicitly (resolve = tab opens, reject = dialog fallback).
vi.mock("@/lib/api", () => ({
  getFolderConversation: vi.fn(),
  openNativeSubagentSession: vi.fn(),
}))
// Rows resolve their model from `useDelegatedSubSession` (live binding) and
// the connections store (child pending-permission). Stub both — the same
// contexts DelegatedSubThread's own test stubs.
vi.mock("@/hooks/use-delegated-sub-session", () => ({
  useDelegatedSubSession: vi.fn(),
}))
// SubAgentSessionDialog pulls in MessageListView + the runtime provider tree.
// Stub it to a sentinel exposing the open state + target conversation id —
// the aux panel renders rows OUTSIDE a SessionViewerHost, so the dialog is
// the row's click path here and clicking a row must prove it.
vi.mock("@/components/message/sub-agent-session-dialog", () => ({
  SubAgentSessionDialog: ({
    open,
    childConversationId,
  }: {
    open: boolean
    childConversationId: number
  }) =>
    open ? (
      <div
        data-testid="sub-agent-session-dialog"
        data-conversation-id={childConversationId}
      />
    ) : null,
}))
// Same for the native sub-agent dialog (child session id + agent type) — the
// row's FALLBACK path when the child upsert fails; the success path opens a
// tab instead and is asserted via the tab store's `openTab`.
vi.mock("@/components/message/subagent-session-dialog", () => ({
  SubagentSessionDialog: ({
    open,
    sessionId,
    agentType,
    subagentType,
  }: {
    open: boolean
    sessionId: string
    agentType: AgentType
    subagentType?: string | null
  }) =>
    open ? (
      <div
        data-testid="subagent-session-dialog"
        data-session-id={sessionId}
        data-agent-type={agentType}
        data-subagent-type={subagentType ?? ""}
      />
    ) : null,
}))

vi.mock("@/contexts/aux-panel-context", () => ({ useAuxPanelContext: vi.fn() }))
vi.mock("@/contexts/tab-context", () => ({ useTabStore: vi.fn() }))
const timelineRegistry = vi.hoisted(() => ({ current: [] as unknown[] }))
vi.mock("@/stores/conversation-runtime-store", () => ({
  useConversationRuntimeStore: vi.fn(),
  // The component passes this straight through from a slice selector; the
  // test hands it the scene's timeline via the hoisted registry.
  selectTimelineTurns: () => timelineRegistry.current,
}))
vi.mock("@/stores/app-workspace-store", () => ({
  useAppWorkspaceStore: vi.fn(),
}))
// The tab reads the parent connection id (to scope delegation bindings) and
// the child's live status (via useDelegationCardModel). Stub the store to a
// fixed connection and the hook to a fixed binding map.
vi.mock("@/contexts/acp-connections-context", async () => {
  const actual = await vi.importActual<
    typeof import("@/contexts/acp-connections-context")
  >("@/contexts/acp-connections-context")
  // Stable objects: `useDelegationCardModel` reads the child connection
  // through `useSyncExternalStore(getConnection)` — a fresh object per call
  // reads as an ever-changing snapshot and spins the render loop.
  const parentConnection = { connectionId: "conn-parent" }
  const store = {
    subscribeKey: () => () => {},
    getConnection: (key: string) =>
      key === "conn-parent" ? parentConnection : undefined,
    getActiveKey: () => null,
    subscribeActiveKey: () => () => {},
  }
  return {
    ...actual,
    useConnectionStore: () => store,
  }
})
vi.mock("@/contexts/delegation-context", () => ({
  useDelegation: vi.fn(),
}))

import { SessionDetailsTab } from "./aux-panel-session-details-tab"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useTabStore } from "@/contexts/tab-context"
import { useConversationRuntimeStore } from "@/stores/conversation-runtime-store"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useDelegation } from "@/contexts/delegation-context"
import { useDelegatedSubSession } from "@/hooks/use-delegated-sub-session"
import { openNativeSubagentSession } from "@/lib/api"

const mockAux = useAuxPanelContext as unknown as Mock
const mockTabs = useTabStore as unknown as Mock
const mockRuntime = useConversationRuntimeStore as unknown as Mock
const mockWorkspace = useAppWorkspaceStore as unknown as Mock
const mockDelegation = useDelegation as unknown as Mock
const mockSubSession = useDelegatedSubSession as unknown as Mock
const mockOpenNative = openNativeSubagentSession as unknown as Mock

type TabSlice = {
  tabs: Array<{
    id: number
    conversationId: number | null
    runtimeConversationId?: number
  }>
  activeTabId: number | null
  rawTabs: Array<{ id: number; conversationId: number | null }>
  openTab: Mock
  registerNativeChildTab: Mock
}
type WorkspaceSlice = { conversations: DbConversationSummary[] }

interface Scene {
  hasActiveConversation: boolean
  timeline?: ConversationTimelineTurn[]
  liveMessage?: { id: string; content: LiveContentBlock[] } | null
  bindings?: DelegationBinding[]
}

function summary(
  over: Partial<DbConversationSummary> = {}
): DbConversationSummary {
  return {
    id: 7,
    folder_id: 1,
    title: "My session",
    title_locked: false,
    agent_type: "claude_code",
    status: "in_progress",
    kind: "regular",
    model: "claude-opus-4-8",
    git_branch: "main",
    external_id: "ext-abc",
    message_count: 12,
    child_count: 0,
    created_at: "2026-06-10T10:00:00.000Z",
    updated_at: "2026-06-12T12:00:00.000Z",
    pinned_at: null,
    ...over,
  }
}

function assistantTurn(
  id: string,
  blocks: MessageTurn["blocks"]
): ConversationTimelineTurn {
  return {
    key: id,
    turn: {
      id,
      role: "assistant",
      blocks,
      timestamp: "2026-06-12T12:00:00.000Z",
    },
    phase: "persisted",
  }
}

function bindingOf(overrides: Partial<DelegationBinding>): DelegationBinding {
  return {
    parentConnectionId: "conn-parent",
    parentToolUseId: "tool-delegate-1",
    childConnectionId: "child-conn",
    childConversationId: 99,
    agentType: "codex",
    status: "running",
    task: "Investigate flaky test",
    taskId: "task-1",
    ...overrides,
  }
}

function setupScene(opts: Scene): TabSlice {
  mockAux.mockReturnValue({ isOpen: true, activeTab: "session_details" })

  const tabState: TabSlice = {
    tabs: opts.hasActiveConversation ? [{ id: 1, conversationId: 7 }] : [],
    activeTabId: opts.hasActiveConversation ? 1 : null,
    rawTabs: opts.hasActiveConversation ? [{ id: 1, conversationId: 7 }] : [],
    openTab: vi.fn() as Mock,
    registerNativeChildTab: vi.fn() as Mock,
  }
  mockTabs.mockImplementation((sel: (s: TabSlice) => unknown) => sel(tabState))

  const timeline = opts.timeline ?? []
  const liveMessage = opts.liveMessage ?? null
  timelineRegistry.current = timeline
  // The component subscribes to the store with two selectors (details and
  // streaming); a bare session object satisfies both — every field it reads
  // (detail, sessionStats, localTurns, liveMessage) is present or defaulted.
  // The streaming selector gets its timeline from the mocked
  // `selectTimelineTurns` above, so the store only backs `liveMessage`.
  const session = {
    detail: null,
    sessionStats: null,
    localTurns: [],
    liveMessage,
  }
  mockRuntime.mockImplementation((sel: (s: unknown) => unknown) =>
    sel({
      byConversationId: new Map(
        opts.hasActiveConversation ? [[7, session]] : []
      ),
    })
  )

  mockWorkspace.mockImplementation((sel: (s: WorkspaceSlice) => unknown) =>
    sel({ conversations: opts.hasActiveConversation ? [summary()] : [] })
  )

  mockDelegation.mockReturnValue({
    listAllBindings: () => opts.bindings ?? [],
    findByParentToolUseId: () => undefined,
    findByChildConversationId: () => undefined,
    findByTaskId: () => undefined,
  })

  mockSubSession.mockReset()
  mockSubSession.mockImplementation((parentToolUseId: string) => {
    const b = (opts.bindings ?? []).find(
      (x) => x.parentToolUseId === parentToolUseId
    )
    return { binding: b, detail: null, loading: false, error: null }
  })
  return tabState
}

function renderTab() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <SessionDetailsTab />
    </NextIntlClientProvider>
  )
}

const TODO_INPUT = JSON.stringify({
  todos: [
    { content: "Plan step A", status: "completed" },
    { content: "Plan step B", status: "pending" },
  ],
})

const DELEGATE_ACK = JSON.stringify({
  kind: "ack",
  task_id: "task-1",
  agent_type: "codex",
})

describe("SessionDetailsTab", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    timelineRegistry.current = []
  })

  it("renders the active session's details with no folder-actions bar", () => {
    // Branch + command moved to the bottom status bar on every platform, so the
    // tab shows the details alone — no branch/command controls here.
    setupScene({ hasActiveConversation: true })
    const { getByText, queryByTestId } = renderTab()
    expect(getByText("My session")).toBeTruthy()
    expect(getByText("Claude Code")).toBeTruthy()
    expect(queryByTestId("branch-dropdown")).toBeNull()
    expect(queryByTestId("command-dropdown")).toBeNull()
  })

  it("shows the empty state when there is no active session", () => {
    setupScene({ hasActiveConversation: false })
    const { getByText, queryByTestId } = renderTab()
    expect(getByText("No active session")).toBeTruthy()
    expect(queryByTestId("branch-dropdown")).toBeNull()
    expect(queryByTestId("command-dropdown")).toBeNull()
  })

  it("hides both sections when the session has no plan and no delegations", () => {
    setupScene({ hasActiveConversation: true })
    renderTab()
    expect(screen.queryByText("Tasks")).toBeNull()
    expect(screen.queryByText("Sub-agents")).toBeNull()
  })

  it("renders the tasks section from a persisted TodoWrite plan", () => {
    setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: TODO_INPUT,
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "todo-1",
            output_preview: "Todos have been modified",
            is_error: false,
          },
        ]),
      ],
    })
    renderTab()
    expect(screen.getByText("Tasks")).toBeTruthy()
    expect(screen.getByText("Plan step A")).toBeTruthy()
    expect(screen.getByText("Plan step B")).toBeTruthy()
    expect(screen.getByText("1/2")).toBeTruthy()
  })

  it("prefers the live plan block over the historical fallback", () => {
    setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: JSON.stringify({
              todos: [{ content: "Old plan", status: "pending" }],
            }),
            status: null,
            meta: null,
          },
        ]),
      ],
      liveMessage: {
        id: "live-1",
        content: [
          {
            type: "plan",
            entries: [
              {
                content: "Fresh plan",
                status: "in_progress",
                priority: "high",
              },
            ],
          },
        ],
      },
    })
    renderTab()
    expect(screen.getByText("Fresh plan")).toBeTruthy()
    expect(screen.queryByText("Old plan")).toBeNull()
  })

  it("lists a delegation from the turn history with binding-backed status", () => {
    setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "tool-delegate-1",
            tool_name: "mcp__codeg__delegate_to_agent",
            input_preview: JSON.stringify({
              agent_type: "codex",
              task: "Investigate flaky test",
            }),
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "tool-delegate-1",
            output_preview: DELEGATE_ACK,
            is_error: false,
          },
        ]),
      ],
      bindings: [bindingOf({})],
    })
    renderTab()
    expect(screen.getByText("Sub-agents")).toBeTruthy()
    const rows = screen.getAllByTestId("sub-agent-row")
    expect(rows).toHaveLength(1)
    expect(screen.getByText("Investigate flaky test")).toBeTruthy()
    // task id shows its short form.
    expect(screen.getByText("#task-1")).toBeTruthy()
  })

  it("shows a mid-stream delegation before any turn carries it", () => {
    setupScene({
      hasActiveConversation: true,
      liveMessage: {
        id: "live-1",
        content: [
          {
            type: "tool_call",
            info: {
              tool_call_id: "tool-delegate-1",
              title: "delegate_to_agent",
              kind: "other",
              status: "in_progress",
              content: null,
              raw_input: JSON.stringify({
                agent_type: "codex",
                task: "Investigate flaky test",
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
      bindings: [bindingOf({})],
    })
    renderTab()
    const rows = screen.getAllByTestId("sub-agent-row")
    expect(rows).toHaveLength(1)
    expect(screen.getByText("Investigate flaky test")).toBeTruthy()
  })

  it("de-dupes a resume against the original delegate by task id", () => {
    setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "tool-delegate-1",
            tool_name: "mcp__codeg__delegate_to_agent",
            input_preview: JSON.stringify({
              agent_type: "codex",
              task: "Investigate flaky test",
            }),
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "tool-delegate-1",
            output_preview: DELEGATE_ACK,
            is_error: false,
          },
        ]),
        assistantTurn("t2", [
          {
            type: "tool_use",
            tool_use_id: "tool-resume-1",
            tool_name: "mcp__codeg__resume_delegation",
            input_preview: JSON.stringify({
              task_id: "task-1",
              reason: "continue",
            }),
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "tool-resume-1",
            output_preview: JSON.stringify({
              status: "running",
              child_conversation_id: 99,
            }),
            is_error: false,
          },
        ]),
      ],
    })
    renderTab()
    // One row per delegation, not two — the resume folds into the original.
    expect(screen.getAllByTestId("sub-agent-row")).toHaveLength(1)
  })

  it("scopes bindings to this conversation's connection", () => {
    setupScene({
      hasActiveConversation: true,
      // A binding from ANOTHER conversation's parent connection must not
      // leak into this session's list.
      bindings: [bindingOf({ parentConnectionId: "conn-other" })],
    })
    renderTab()
    expect(screen.queryByTestId("sub-agent-row")).toBeNull()
    expect(screen.queryByText("Sub-agents")).toBeNull()
  })

  it("opens the child session dialog when a row is clicked", () => {
    setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "tool-delegate-1",
            tool_name: "mcp__codeg__delegate_to_agent",
            input_preview: JSON.stringify({
              agent_type: "codex",
              task: "Investigate flaky test",
            }),
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "tool-delegate-1",
            output_preview: DELEGATE_ACK,
            is_error: false,
          },
        ]),
      ],
      bindings: [bindingOf({})],
    })
    renderTab()
    fireEvent.click(screen.getByTestId("sub-agent-row"))
    expect(
      screen.getByTestId("sub-agent-session-dialog").dataset.conversationId
    ).toBe("99")
  })

  // ── Native sub-agents: the agent's OWN spawned children ───────────────────
  // The same contract the message-area dispatch uses: the launch tool name
  // arrives through the real normalization (multi-agent family op-suffix
  // rule), the title through the shared launch-field parser.

  it("renders a codex team-of-agents spawn and opens its child session", async () => {
    const tabState = setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "spawn-1",
            tool_name: "multi_agent_v1__spawn_agent",
            // The rollout path stamps the launch marker + the child's UUID;
            // `message` is the child's whole assignment (no description).
            input_preview: JSON.stringify({
              message: "先用法语说你好",
              agent_id: "019f-uuid-child",
              __codegCodexSubagentLaunch: true,
            }),
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "spawn-1",
            output_preview: JSON.stringify({ ok: true }),
            is_error: false,
          },
        ]),
      ],
    })
    mockOpenNative.mockResolvedValue({
      conversationId: 42,
      // The backend derives the child's agent type from the parent row —
      // this scene's parent is claude_code.
      agentType: "claude_code",
      folderId: 1,
    })
    renderTab()
    const rows = screen.getAllByTestId("native-subagent-row")
    expect(rows).toHaveLength(1)
    // No subagent_type field ⇒ the title is the task text itself.
    expect(rows[0].textContent).toContain("先用法语说你好")
    fireEvent.click(rows[0])
    // The click upserts the child handle and opens it as a PINNED tab right
    // beside the parent (rawTabs slot 0 ⇒ insert at 1), linked for cascade.
    await waitFor(() => expect(tabState.openTab).toHaveBeenCalled())
    expect(mockOpenNative).toHaveBeenCalledWith(
      7,
      "019f-uuid-child",
      "先用法语说你好"
    )
    expect(tabState.registerNativeChildTab).toHaveBeenCalledWith(42, 7)
    expect(tabState.openTab).toHaveBeenCalledWith(
      1,
      42,
      "claude_code",
      true,
      "先用法语说你好",
      { index: 1 }
    )
  })

  it("falls back to the read-only dialog when the child upsert fails", async () => {
    setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "spawn-1",
            tool_name: "multi_agent_v1__spawn_agent",
            input_preview: JSON.stringify({
              message: "先用法语说你好",
              agent_id: "019f-uuid-child",
              __codegCodexSubagentLaunch: true,
            }),
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "spawn-1",
            output_preview: JSON.stringify({ ok: true }),
            is_error: false,
          },
        ]),
      ],
    })
    mockOpenNative.mockRejectedValue(new Error("offline"))
    renderTab()
    const rows = screen.getAllByTestId("native-subagent-row")
    fireEvent.click(rows[0])
    await waitFor(() =>
      expect(screen.queryByTestId("subagent-session-dialog")).toBeTruthy()
    )
    const dialog = screen.getByTestId("subagent-session-dialog")
    expect(dialog.dataset.sessionId).toBe("019f-uuid-child")
    // The handle's type follows the conversation's own agent (the scene's
    // summary is claude_code) — the parent's kind parses the child file.
    expect(dialog.dataset.agentType).toBe("claude_code")
  })

  it("expands a Hermes batched delegate_task into one row per task", () => {
    setupScene({
      hasActiveConversation: true,
      timeline: [
        assistantTurn("t1", [
          {
            type: "tool_use",
            tool_use_id: "batch-1",
            tool_name: "delegate_task",
            input_preview: JSON.stringify({
              tasks: [{ goal: "用日语问候" }, { goal: "算 97 是否质数" }],
            }),
            status: null,
            meta: null,
          },
          {
            type: "tool_result",
            tool_use_id: "batch-1",
            output_preview: JSON.stringify({ results: [] }),
            is_error: false,
          },
        ]),
      ],
    })
    renderTab()
    const rows = screen.getAllByTestId("native-subagent-row")
    expect(rows).toHaveLength(2)
    expect(rows[0].textContent).toContain("用日语问候")
    expect(rows[1].textContent).toContain("算 97 是否质数")
    // A batched Hermes child has no child-session handle on disk: the row is
    // a plain div, never a click target.
    expect(rows[0].tagName).not.toBe("BUTTON")
  })
})
