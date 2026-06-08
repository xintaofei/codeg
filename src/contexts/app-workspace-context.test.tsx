import { act, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import {
  AppWorkspaceProvider,
  useAppWorkspace,
} from "@/contexts/app-workspace-context"
import type { ConversationChange, DbConversationSummary } from "@/lib/types"

// Capture the `conversation://changed` handler + reconnect callback the
// provider registers, plus dispose/unsub spies, so tests can drive events and
// assert cleanup. `vi.hoisted` runs before the (hoisted) mock factories so they
// can close over this shared state without a TDZ error.
const h = vi.hoisted(() => ({
  handler: null as null | ((change: unknown) => void),
  reconnect: null as null | (() => void),
  disposeSpy: vi.fn(),
  reconnectUnsubSpy: vi.fn(),
  listAll: vi.fn(async () => [] as unknown[]),
}))

vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn(async (_event: string, handler: (c: unknown) => void) => {
    h.handler = handler
    return h.disposeSpy
  }),
  onTransportReconnect: vi.fn((cb: () => void) => {
    h.reconnect = cb
    return h.reconnectUnsubSpy
  }),
}))

vi.mock("@/lib/api", () => ({
  listAllConversations: h.listAll,
  listAllFolderDetails: vi.fn(async () => []),
  listOpenFolderDetails: vi.fn(async () => []),
  getGitBranch: vi.fn(async () => null),
  openFolder: vi.fn(),
  openFolderById: vi.fn(),
  removeFolderFromWorkspace: vi.fn(),
  reorderFolders: vi.fn(),
  getFolder: vi.fn(),
}))

// The provider imports `useAcpEvent` only for the separate
// `ConversationStatusEventBridge` (not rendered here); stub the module so we
// don't pull in the heavy ACP context.
vi.mock("@/contexts/acp-connections-context", () => ({
  useAcpEvent: vi.fn(),
}))

function makeSummary(
  overrides: Partial<DbConversationSummary> & { id: number }
): DbConversationSummary {
  return {
    folder_id: 1,
    title: null,
    agent_type: "claude_code",
    status: "in_progress",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 0,
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
    parent_id: null,
    parent_tool_use_id: null,
    delegation_call_id: null,
    ...overrides,
  }
}

function Probe() {
  const { conversations, stats } = useAppWorkspace()
  return (
    <div>
      <output data-testid="ids">
        {conversations.map((c) => c.id).join(",")}
      </output>
      <output data-testid="count">{conversations.length}</output>
      <output data-testid="statuses">
        {conversations.map((c) => `${c.id}:${c.status}`).join(",")}
      </output>
      <output data-testid="stat-total">
        {stats?.total_conversations ?? 0}
      </output>
      <output data-testid="stat-messages">{stats?.total_messages ?? 0}</output>
    </div>
  )
}

async function mountProvider() {
  const utils = render(
    <AppWorkspaceProvider>
      <Probe />
    </AppWorkspaceProvider>
  )
  // Flush mount effects: fetchFolders/refreshConversations + the async
  // subscribe() IIFE that captures the handler.
  await act(async () => {})
  return utils
}

function emit(change: ConversationChange) {
  act(() => {
    h.handler?.(change)
  })
}

beforeEach(() => {
  h.handler = null
  h.reconnect = null
  h.disposeSpy.mockClear()
  h.reconnectUnsubSpy.mockClear()
  h.listAll.mockClear()
  h.listAll.mockResolvedValue([])
})

describe("AppWorkspaceProvider conversation://changed sync", () => {
  it("registers a subscription and reconnect backstop on mount", async () => {
    await mountProvider()
    expect(h.handler).toBeTypeOf("function")
    expect(h.reconnect).toBeTypeOf("function")
  })

  it("inserts a new root conversation, prepending most-recent-first", async () => {
    await mountProvider()
    emit({ kind: "upsert", summary: makeSummary({ id: 1 }) })
    emit({ kind: "upsert", summary: makeSummary({ id: 2 }) })
    expect(screen.getByTestId("ids")).toHaveTextContent("2,1")
    expect(screen.getByTestId("count")).toHaveTextContent("2")
    expect(screen.getByTestId("stat-total")).toHaveTextContent("2")
  })

  it("replaces an existing conversation in place (no reorder) and updates fields", async () => {
    await mountProvider()
    emit({ kind: "upsert", summary: makeSummary({ id: 1 }) })
    emit({ kind: "upsert", summary: makeSummary({ id: 2 }) })
    // Re-upsert id 1 with a new status; it must keep its index (1), not jump.
    emit({
      kind: "upsert",
      summary: makeSummary({ id: 1, status: "pending_review" }),
    })
    expect(screen.getByTestId("ids")).toHaveTextContent("2,1")
    expect(screen.getByTestId("statuses")).toHaveTextContent(
      "2:in_progress,1:pending_review"
    )
  })

  it("ignores delegation children (parent_id set) — not sidebar rows", async () => {
    await mountProvider()
    emit({ kind: "upsert", summary: makeSummary({ id: 1 }) })
    emit({ kind: "upsert", summary: makeSummary({ id: 5, parent_id: 1 }) })
    expect(screen.getByTestId("ids")).toHaveTextContent("1")
    expect(screen.getByTestId("count")).toHaveTextContent("1")
  })

  it("removes on deleted and is idempotent for an unknown id", async () => {
    await mountProvider()
    emit({ kind: "upsert", summary: makeSummary({ id: 1 }) })
    emit({ kind: "upsert", summary: makeSummary({ id: 2 }) })
    emit({ kind: "deleted", id: 1 })
    expect(screen.getByTestId("ids")).toHaveTextContent("2")
    emit({ kind: "deleted", id: 999 })
    expect(screen.getByTestId("ids")).toHaveTextContent("2")
    expect(screen.getByTestId("count")).toHaveTextContent("1")
  })

  it("does not resurrect a row when a stale upsert lands after a delete", async () => {
    await mountProvider()
    emit({ kind: "upsert", summary: makeSummary({ id: 1 }) })
    emit({ kind: "deleted", id: 1 })
    expect(screen.getByTestId("count")).toHaveTextContent("0")
    // A stale/out-of-order upsert for the just-deleted id must be ignored —
    // ids are never reused, so the tombstone is authoritative.
    emit({
      kind: "upsert",
      summary: makeSummary({ id: 1, status: "pending_review" }),
    })
    expect(screen.getByTestId("count")).toHaveTextContent("0")
    expect(screen.getByTestId("ids").textContent).toBe("")
  })

  it("patches status for a known conversation and no-ops for an unknown one", async () => {
    await mountProvider()
    emit({ kind: "upsert", summary: makeSummary({ id: 1 }) })
    emit({ kind: "status", id: 1, status: "pending_review" })
    expect(screen.getByTestId("statuses")).toHaveTextContent("1:pending_review")
    emit({ kind: "status", id: 999, status: "cancelled" })
    expect(screen.getByTestId("count")).toHaveTextContent("1")
    expect(screen.getByTestId("statuses")).toHaveTextContent("1:pending_review")
  })

  it("derives stats.total_messages from upserted message counts", async () => {
    await mountProvider()
    emit({ kind: "upsert", summary: makeSummary({ id: 1, message_count: 3 }) })
    emit({ kind: "upsert", summary: makeSummary({ id: 2, message_count: 4 }) })
    expect(screen.getByTestId("stat-total")).toHaveTextContent("2")
    expect(screen.getByTestId("stat-messages")).toHaveTextContent("7")
  })

  it("re-fetches the full list on transport reconnect (disconnect backstop)", async () => {
    await mountProvider()
    expect(h.listAll).toHaveBeenCalledTimes(1) // initial mount fetch
    await act(async () => {
      h.reconnect?.()
    })
    expect(h.listAll).toHaveBeenCalledTimes(2)
  })

  it("disposes the subscription and reconnect handler on unmount", async () => {
    const { unmount } = await mountProvider()
    unmount()
    expect(h.disposeSpy).toHaveBeenCalledTimes(1)
    expect(h.reconnectUnsubSpy).toHaveBeenCalledTimes(1)
  })
})
