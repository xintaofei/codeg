import { describe, it, expect, vi, beforeEach, afterEach } from "vitest"
import { renderHook, waitFor } from "@testing-library/react"
import { usePipelineRun } from "./use-pipeline-run"
import * as api from "@/lib/api"

vi.mock("@/lib/api")

function createMockRun(overrides = {}) {
  return {
    id: 1,
    pipeline_id: null,
    folder_id: 1,
    worktree_folder_id: null,
    parent_conversation_id: null,
    graph: { steps: [], loops: [] },
    status: "running" as const,
    current_step_id: null,
    current_iteration: 0,
    error: null,
    attempts: [],
    started_at: "2024-01-01T00:00:00Z",
    ended_at: null,
    ...overrides,
  }
}

describe("usePipelineRun", () => {
  beforeEach(() => {
    vi.mocked(api.pipelineRunStatus).mockImplementation(async (runId) =>
      createMockRun({ id: runId })
    )

    vi.mocked(api.subscribePipelineChanged).mockImplementation(async () => {
      return () => {}
    })
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  it("loads run on mount with runId", async () => {
    renderHook(() => usePipelineRun(42))

    await waitFor(() => {
      expect(api.pipelineRunStatus).toHaveBeenCalledWith(42)
    })
  })

  it("subscribes to pipeline://changed on mount", async () => {
    renderHook(() => usePipelineRun(42))

    await waitFor(() => {
      expect(api.subscribePipelineChanged).toHaveBeenCalled()
    })
  })

  it("returns null when runId is null", () => {
    const { result } = renderHook(() => usePipelineRun(null))
    expect(result.current).toBeNull()
  })

  it("unsubscribes on unmount", async () => {
    const unsubscribeMock = vi.fn()
    vi.mocked(api.subscribePipelineChanged).mockResolvedValue(unsubscribeMock)

    const { unmount } = renderHook(() => usePipelineRun(42))

    await waitFor(() => {
      expect(api.subscribePipelineChanged).toHaveBeenCalled()
    })

    unmount()

    expect(unsubscribeMock).toHaveBeenCalled()
  })
})
