import { beforeEach, describe, expect, it, vi } from "vitest"
import {
  contestantBranchName,
  contestantContextKey,
  dbRoundToStoreRound,
  usePkArenaStore,
  type PkRound,
} from "./pk-arena-store"
import type { DbConversationSummary, PkRoundInfo } from "@/lib/types"
import {
  pkRoundCreate,
  pkRoundUpdateJudge,
  pkRoundUpdateStatus,
} from "@/lib/api"

// Mock the API calls so createRound/markRound/archiveRound don't hit the network.
vi.mock("@/lib/api", () => ({
  pkRoundCreate: vi.fn().mockResolvedValue({
    id: 1,
    folder_id: 7,
    task: "write a snake game",
    config: {
      agents: ["claude_code", "codex"],
      permission_mode: "default",
      bare_mode: false,
      effort: "default",
    },
    status: "ready",
    failure_reason: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    finished_at: null,
    judge_status: "idle",
  }),
  pkRoundUpdateStatus: vi.fn().mockResolvedValue(undefined),
  pkRoundDelete: vi.fn().mockResolvedValue(undefined),
  pkRoundUpdateJudge: vi.fn().mockResolvedValue(undefined),
}))

function freshStore() {
  usePkArenaStore.setState({
    rounds: [],
    activeRoundId: null,
    launcherOpen: false,
    pillDismissed: false,
    hydrating: false,
  })
}

async function makeRound(overrides?: Partial<PkRound>): Promise<PkRound> {
  const round = await usePkArenaStore.getState().createRound({
    task: "write a snake game",
    folderId: 7,
    workingDir: "/tmp/repo",
    agents: [
      { agentType: "claude_code" as const },
      { agentType: "codex" as const },
    ],
  })
  return overrides ? { ...round, ...overrides } : round
}

describe("pk arena store", () => {
  beforeEach(() => {
    freshStore()
    vi.clearAllMocks()
  })

  it("creates a round with one preparing contestant per agent", async () => {
    const round = await makeRound()
    expect(round.status).toBe("ready")
    expect(round.contestants.map((c) => c.agentType)).toEqual([
      "claude_code",
      "codex",
    ])
    expect(round.contestants.map((c) => c.slot)).toEqual([0, 1])
    expect(
      round.contestants.every(
        (c) =>
          c.status === "preparing" &&
          c.contextKey === null &&
          c.connectionId === null &&
          c.conversationId === null
      )
    ).toBe(true)
    expect(usePkArenaStore.getState().activeRoundId).toBe(round.id)
    expect(usePkArenaStore.getState().rounds).toHaveLength(1)
  })

  it("patches a single contestant without touching its peers", async () => {
    const round = await makeRound()
    usePkArenaStore.getState().updateContestant(round.id, 1, {
      status: "running",
      startedAt: 1234,
      connectionId: "conn-1",
    })
    const codex = usePkArenaStore
      .getState()
      .rounds[0].contestants.find((c) => c.slot === 1)
    const claude = usePkArenaStore
      .getState()
      .rounds[0].contestants.find((c) => c.slot === 0)
    expect(codex).toMatchObject({ status: "running", connectionId: "conn-1" })
    expect(claude?.status).toBe("preparing")
  })

  it("marks round status and removes rounds", async () => {
    const round = await makeRound()
    usePkArenaStore.getState().markRound(round.id, "finished")
    expect(usePkArenaStore.getState().rounds[0].status).toBe("finished")

    await usePkArenaStore.getState().archiveRound(round.id)
    expect(usePkArenaStore.getState().rounds).toHaveLength(0)
    expect(usePkArenaStore.getState().activeRoundId).toBeNull()
  })

  it("surfaces persistence failures until the same field saves successfully", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {})
    const round = await makeRound()
    vi.mocked(pkRoundUpdateStatus).mockRejectedValueOnce(
      new Error("database unavailable")
    )

    usePkArenaStore.getState().markRound(round.id, "running")

    await vi.waitFor(() =>
      expect(
        usePkArenaStore.getState().rounds[0].persistenceErrors.status
      ).toBe("database unavailable")
    )

    usePkArenaStore.getState().retryPersistence(round.id)

    await vi.waitFor(() =>
      expect(
        usePkArenaStore.getState().rounds[0].persistenceErrors.status
      ).toBeNull()
    )
    consoleError.mockRestore()
  })

  it("serializes writes so an older request cannot overwrite a newer value", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {})
    const round = await makeRound()
    let rejectOlder!: (error: Error) => void
    vi.mocked(pkRoundUpdateStatus).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectOlder = reject
        })
    )

    usePkArenaStore.getState().markRound(round.id, "running")
    usePkArenaStore.getState().markRound(round.id, "finished")
    await vi.waitFor(() => expect(pkRoundUpdateStatus).toHaveBeenCalledTimes(1))
    rejectOlder(new Error("stale failure"))
    await vi.waitFor(() => expect(pkRoundUpdateStatus).toHaveBeenCalledTimes(2))
    await vi.waitFor(() =>
      expect(
        usePkArenaStore.getState().rounds[0].persistenceErrors.status
      ).toBeNull()
    )

    expect(vi.mocked(pkRoundUpdateStatus).mock.calls).toEqual([
      [Number(round.id), "running"],
      [Number(round.id), "finished"],
    ])
    consoleError.mockRestore()
  })

  it("surfaces judge persistence failures independently", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {})
    const round = await makeRound()
    vi.mocked(pkRoundUpdateJudge).mockRejectedValueOnce(
      new Error("judge write failed")
    )

    usePkArenaStore.getState().updateJudge(round.id, {
      judgeStatus: "running",
    })

    await vi.waitFor(() =>
      expect(usePkArenaStore.getState().rounds[0].persistenceErrors.judge).toBe(
        "judge write failed"
      )
    )
    expect(
      usePkArenaStore.getState().rounds[0].persistenceErrors.status
    ).toBeNull()
    consoleError.mockRestore()
  })

  it("does not implicitly select history during hydration", async () => {
    const round = await makeRound()
    usePkArenaStore.setState({
      rounds: [],
      activeRoundId: null,
      hydrating: true,
    })

    usePkArenaStore.getState().hydrateFromDb([round])

    expect(usePkArenaStore.getState().activeRoundId).toBeNull()
    expect(usePkArenaStore.getState().hydrating).toBe(false)
  })

  it("preserves a sidebar-selected round while its history is still hydrating", async () => {
    const clickedRound = await makeRound({ id: "3" })
    const newerRound = { ...clickedRound, id: "4", task: "newer round" }
    usePkArenaStore.setState({
      rounds: [],
      activeRoundId: "3",
      hydrating: true,
    })

    usePkArenaStore.getState().hydrateFromDb([newerRound, clickedRound])

    expect(usePkArenaStore.getState().activeRoundId).toBe("3")
  })

  it("derives branch and context keys from the round id and slot", () => {
    expect(contestantBranchName("r1", 0)).toBe("codeg-pk/r1/0")
    expect(contestantContextKey("r1", 1)).toBe("pk:r1:1")
  })

  it("revives a running round as interrupted from DB hydration", () => {
    const dbRound: PkRoundInfo = {
      id: 42,
      folder_id: 7,
      task: "write a snake game",
      config: {
        agents: ["claude_code", "codex"],
        permission_mode: "default",
        bare_mode: false,
        effort: "default",
      },
      status: "running",
      judge_status: "idle",
      failure_reason: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      finished_at: null,
    }
    const revived = dbRoundToStoreRound(dbRound, "/tmp/repo")
    expect(revived.id).toBe("42")
    expect(revived.status).toBe("interrupted")
    expect(revived.contestants.map((c) => c.status)).toEqual([
      "canceled",
      "canceled",
    ])
    expect(revived.contestants.every((c) => c.contextKey === null)).toBe(true)
    expect(revived.contestants.map((c) => c.slot)).toEqual([0, 1])
  })

  it("revives a finished round unchanged from DB hydration", () => {
    const dbRound: PkRoundInfo = {
      id: 43,
      folder_id: 7,
      task: "done task",
      config: {
        agents: ["codex"],
        permission_mode: "default",
        bare_mode: false,
        effort: "default",
      },
      status: "finished",
      judge_status: "idle",
      failure_reason: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T01:00:00Z",
      finished_at: "2026-01-01T01:00:00Z",
    }
    const revived = dbRoundToStoreRound(dbRound, "/tmp/repo")
    expect(revived.status).toBe("finished")
    expect(revived.contestants[0].status).toBe("done")
  })

  it("reattaches persisted PK conversations to contestant slots", () => {
    const dbRound: PkRoundInfo = {
      id: 42,
      folder_id: 7,
      task: "history",
      config: {
        agents: ["claude_code", "qoder"],
        permission_mode: "default",
        bare_mode: false,
        effort: "default",
      },
      status: "finished",
      judge_status: "done",
      failure_reason: null,
      created_at: "2026-08-20T08:00:00Z",
      updated_at: "2026-08-20T08:10:00Z",
      finished_at: "2026-08-20T08:10:00Z",
    }
    const conversations = [
      {
        id: 81,
        pk_round_id: 42,
        title: "PK Judge · history",
        created_at: "2026-08-20T08:03:00Z",
      },
      {
        id: 80,
        pk_round_id: 42,
        title: "PK · history",
        model: "qoder-model",
        status: "cancelled",
        created_at: "2026-08-20T08:02:00Z",
      },
      {
        id: 79,
        pk_round_id: 42,
        title: "PK · history",
        model: "claude-model",
        status: "pending_review",
        created_at: "2026-08-20T08:01:00Z",
      },
    ] as DbConversationSummary[]

    const hydrated = dbRoundToStoreRound(dbRound, "/repo", conversations)

    expect(hydrated.contestants.map((c) => c.conversationId)).toEqual([79, 80])
    expect(hydrated.contestants.map((c) => c.selectedModel)).toEqual([
      "claude-model",
      "qoder-model",
    ])
    expect(hydrated.contestants.map((c) => c.status)).toEqual([
      "done",
      "canceled",
    ])
  })

  it("supports same agent in multiple slots (control-variable PK)", async () => {
    const round = await usePkArenaStore.getState().createRound({
      task: "write a snake game",
      folderId: 7,
      workingDir: "/tmp/repo",
      agents: [
        {
          agentType: "claude_code" as const,
          label: "Sonnet",
          configValues: { model: "sonnet" },
        },
        {
          agentType: "claude_code" as const,
          label: "Opus",
          configValues: { model: "opus" },
        },
      ],
    })
    expect(round.contestants).toHaveLength(2)
    expect(round.contestants.every((c) => c.agentType === "claude_code")).toBe(
      true
    )
    expect(round.contestants.map((c) => c.slot)).toEqual([0, 1])
    expect(round.contestants.map((c) => c.label)).toEqual(["Sonnet", "Opus"])
    expect(round.contestants.map((c) => c.configValues.model)).toEqual([
      "sonnet",
      "opus",
    ])
    expect(vi.mocked(pkRoundCreate)).toHaveBeenCalledWith(
      7,
      "write a snake game",
      expect.objectContaining({
        agents: [
          {
            agent: "claude_code",
            label: "Sonnet",
            config_values: { model: "sonnet" },
          },
          {
            agent: "claude_code",
            label: "Opus",
            config_values: { model: "opus" },
          },
        ],
      })
    )
  })

  it("supports labeled contestant entries in DB hydration", () => {
    const dbRound: PkRoundInfo = {
      id: 44,
      folder_id: 7,
      task: "control variable test",
      config: {
        agents: [
          {
            agent: "claude_code",
            label: "Sonnet",
            config_values: { model: "sonnet" },
          },
          {
            agent: "claude_code",
            label: "Opus",
            config_values: { model: "opus" },
          },
        ],
        permission_mode: "default",
        bare_mode: false,
        effort: "default",
      },
      status: "finished",
      judge_status: "idle",
      failure_reason: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T01:00:00Z",
      finished_at: "2026-01-01T01:00:00Z",
    }
    const revived = dbRoundToStoreRound(dbRound, "/tmp/repo")
    expect(revived.contestants).toHaveLength(2)
    expect(
      revived.contestants.every((c) => c.agentType === "claude_code")
    ).toBe(true)
    expect(revived.contestants.map((c) => c.slot)).toEqual([0, 1])
    expect(revived.contestants.map((c) => c.label)).toEqual(["Sonnet", "Opus"])
    expect(revived.contestants.map((c) => c.configValues.model)).toEqual([
      "sonnet",
      "opus",
    ])
    expect(revived.contestants.map((c) => c.selectedModel)).toEqual([
      "sonnet",
      "opus",
    ])
  })
})
