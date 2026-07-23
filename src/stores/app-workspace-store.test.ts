import { beforeEach, describe, expect, it, vi } from "vitest"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "./app-workspace-store"
import type { DbConversationSummary, FolderDetail } from "@/lib/types"

vi.mock("@/lib/api", () => ({
  getFolder: vi.fn(),
}))

const { getFolder } = await import("@/lib/api")
const mockGetFolder = vi.mocked(getFolder)

function makeSummary(
  overrides: Partial<DbConversationSummary> & { id: number }
): DbConversationSummary {
  return {
    folder_id: 1,
    title: null,
    title_locked: false,
    agent_type: "claude_code",
    status: "in_progress",
    kind: "regular",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 0,
    child_count: 0,
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
    pinned_at: null,
    parent_id: null,
    parent_tool_use_id: null,
    delegation_call_id: null,
    ...overrides,
  }
}

beforeEach(() => {
  resetAppWorkspaceStore()
})

describe("updateConversationLocal — stats reference stability", () => {
  function seedTwo() {
    const store = useAppWorkspaceStore.getState()
    store.applyConversationUpsert(makeSummary({ id: 1, message_count: 3 }))
    store.applyConversationUpsert(makeSummary({ id: 2, message_count: 4 }))
  }

  it("reuses the stats reference on a status patch (no stat can change)", () => {
    seedTwo()
    const before = useAppWorkspaceStore.getState()
    const statsBefore = before.stats
    const conversationsBefore = before.conversations

    useAppWorkspaceStore
      .getState()
      .updateConversationLocal(1, { status: "pending_review" })

    const after = useAppWorkspaceStore.getState()
    // The regression guard: a turn-boundary status flip must NOT mint a fresh
    // `stats` object (which would re-render every stats subscriber for a no-op).
    expect(after.stats).toBe(statsBefore)
    // But the row's data genuinely changed, so `conversations` gets a new ref
    // (sidebar consumers must see the status update).
    expect(after.conversations).not.toBe(conversationsBefore)
    expect(after.conversations.find((c) => c.id === 1)?.status).toBe(
      "pending_review"
    )
  })

  it("reuses the stats reference on a title patch", () => {
    seedTwo()
    const statsBefore = useAppWorkspaceStore.getState().stats

    useAppWorkspaceStore
      .getState()
      .updateConversationLocal(2, { title: "Renamed" })

    const after = useAppWorkspaceStore.getState()
    expect(after.stats).toBe(statsBefore)
    expect(after.conversations.find((c) => c.id === 2)?.title).toBe("Renamed")
  })

  it("leaves state untouched (stable refs) for an unknown id", () => {
    seedTwo()
    const before = useAppWorkspaceStore.getState()

    before.updateConversationLocal(999, { status: "cancelled" })

    const after = useAppWorkspaceStore.getState()
    expect(after.stats).toBe(before.stats)
    expect(after.conversations).toBe(before.conversations)
  })

  it("still tracks stats when message_count actually changes (via upsert)", () => {
    seedTwo()
    // total_messages = 3 + 4
    expect(useAppWorkspaceStore.getState().stats?.total_messages).toBe(7)

    // A real message_count change flows through applyConversationUpsert (whose
    // recompute we intentionally left intact), so stats update as before.
    useAppWorkspaceStore
      .getState()
      .applyConversationUpsert(makeSummary({ id: 1, message_count: 10 }))

    expect(useAppWorkspaceStore.getState().stats?.total_messages).toBe(14)
  })
})

function makeFolder(
  overrides: Partial<FolderDetail> & { id: number }
): FolderDetail {
  return {
    name: "repo",
    path: "/tmp/repo",
    git_branch: null,
    default_agent_type: null,
    last_opened_at: "2026-01-01T00:00:00.000Z",
    sort_order: 1,
    color: "#000000",
    parent_id: null,
    kind: "regular",
    alias: null,
    ...overrides,
  }
}

describe("refreshFolder — branch null-guard", () => {
  it("keeps the poll-resolved branch when the refreshed row's git_branch is null", async () => {
    // Git-head polling has populated the display branch; the folder row's
    // `git_branch` column is null (it always is today), so the refresh must
    // leave the polled name alone.
    useAppWorkspaceStore.getState().setBranch(1, "feature/x")
    mockGetFolder.mockResolvedValue(makeFolder({ id: 1, git_branch: null }))

    await useAppWorkspaceStore.getState().refreshFolder(1)

    // Regression guard for the "no branch" flash: a null DB branch must not
    // clobber the polled name (which would blank the bottom selector until the
    // next poll, up to 10s later).
    expect(useAppWorkspaceStore.getState().branches.get(1)).toBe("feature/x")
  })

  it("adopts the refreshed branch when the row actually carries one", async () => {
    useAppWorkspaceStore.getState().setBranch(1, "old")
    mockGetFolder.mockResolvedValue(makeFolder({ id: 1, git_branch: "main" }))

    await useAppWorkspaceStore.getState().refreshFolder(1)

    expect(useAppWorkspaceStore.getState().branches.get(1)).toBe("main")
  })
})
