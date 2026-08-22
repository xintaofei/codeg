import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import type { PkRound } from "@/stores/pk-arena-store"

const mocks = vi.hoisted(() => ({
  eventHandler: null as ((event: Record<string, unknown>) => void) | null,
  updateConversationStatus: vi.fn(),
  updateJudge: vi.fn(),
  disconnect: vi.fn().mockResolvedValue(undefined),
}))

vi.mock("next-intl", () => ({ useLocale: () => "zh-CN" }))

vi.mock("@/lib/api", () => ({
  acpRespondPermission: vi.fn(),
  createPkConversation: vi.fn().mockResolvedValue(81),
  getFolderConversation: vi.fn().mockResolvedValue({
    turns: [
      {
        role: "assistant",
        blocks: [
          {
            type: "text",
            text: '{"scores":[],"summary":"done"}',
          },
        ],
      },
    ],
  }),
  getFileTree: vi.fn().mockResolvedValue([]),
  getGitBranch: vi.fn(),
  gitDiff: vi.fn(),
  gitDiffWithBranch: vi.fn(),
  gitStatus: vi.fn().mockResolvedValue([]),
  gitRemoveWorktree: vi.fn(),
  gitWorktreeAdd: vi.fn(),
  pkRoundCreate: vi.fn(),
  pkRoundDelete: vi.fn(),
  pkRoundGetReportSnapshot: vi.fn().mockResolvedValue(null),
  pkRoundSaveReportSnapshot: vi.fn().mockResolvedValue(undefined),
  pkRoundUpdateJudge: vi.fn().mockResolvedValue(undefined),
  pkRoundUpdateStatus: vi.fn().mockResolvedValue(undefined),
  readWorkspaceFileBase64: vi.fn(),
  updateConversationStatus: mocks.updateConversationStatus,
}))

vi.mock("@/contexts/acp-connections-context", () => ({
  useAcpActions: () => ({
    connect: vi.fn().mockResolvedValue("judge-connection"),
    sendPrompt: vi.fn().mockResolvedValue(undefined),
    cancel: vi.fn(),
    disconnect: mocks.disconnect,
    setMode: vi.fn(),
    setConfigOption: vi.fn(),
    touchActivity: vi.fn(),
    respondPermission: vi.fn(),
    attachDelegationChild: vi.fn(),
    detachDelegationChild: vi.fn(),
  }),
  useConnectionStore: () => ({ getConnection: vi.fn() }),
  useAcpEvent: (handler: (event: Record<string, unknown>) => void) => {
    mocks.eventHandler = handler
  },
}))

import { usePkRound } from "@/hooks/use-pk-round"
import { usePkArenaStore } from "@/stores/pk-arena-store"

describe("PK judge conversation lifecycle", () => {
  beforeEach(() => {
    mocks.eventHandler = null
    mocks.updateConversationStatus.mockReset().mockResolvedValue(undefined)
    mocks.disconnect.mockClear()
    usePkArenaStore.setState({
      rounds: [],
      activeRoundId: null,
      launcherOpen: false,
      pillDismissed: false,
      hydrating: false,
    })
  })

  it("marks the judge conversation completed after turn_complete", async () => {
    const round = {
      id: "2",
      task: "judge task",
      folderId: 1,
      workingDir: "/repo",
      status: "finished",
      judgeAgent: "codex",
      judgeStatus: "idle",
      judgeDimensions: null,
      contestants: [
        {
          slot: 0,
          agentType: "qoder",
          status: "done",
          diff: "+done",
        },
      ],
    } as PkRound
    usePkArenaStore.setState({ rounds: [round] })
    const { result } = renderHook(() => usePkRound())

    await act(async () => result.current.runJudge(round))
    expect(mocks.eventHandler).not.toBeNull()
    act(() => {
      mocks.eventHandler?.({
        type: "turn_complete",
        connection_id: "judge-connection",
      })
    })

    await waitFor(() => {
      expect(mocks.updateConversationStatus).toHaveBeenCalledWith(
        81,
        "completed"
      )
    })
    act(() => {
      mocks.eventHandler?.({
        type: "status_changed",
        status: "disconnected",
        connection_id: "judge-connection",
      })
    })
    expect(mocks.updateConversationStatus).not.toHaveBeenCalledWith(
      81,
      "cancelled"
    )
  })

  it("settles every live contestant conversation when a round is canceled", async () => {
    const round = {
      id: "4",
      task: "cancel task",
      folderId: 1,
      workingDir: "/repo",
      status: "ready",
      judgeAgent: null,
      judgeStatus: "idle",
      judgeDimensions: null,
      contestants: [
        {
          slot: 0,
          agentType: "qoder",
          status: "ready",
          conversationId: 89,
        },
        {
          slot: 1,
          agentType: "codex",
          status: "connecting",
          conversationId: 90,
        },
      ],
    } as PkRound
    usePkArenaStore.setState({ rounds: [round] })
    const { result } = renderHook(() => usePkRound())

    await act(async () => result.current.cancelRound(round))

    expect(mocks.updateConversationStatus).toHaveBeenCalledWith(89, "cancelled")
    expect(mocks.updateConversationStatus).toHaveBeenCalledWith(90, "cancelled")
  })
})
