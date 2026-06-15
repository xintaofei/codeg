import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { IterationList } from "./iteration-list"
import type {
  LoopArtifactRow,
  LoopIterationRow,
  LoopValidationRunRow,
} from "@/lib/types"

const stableT = (key: string) => key
vi.mock("next-intl", () => ({ useTranslations: () => stableT }))

const listLoopIterations = vi.fn()
const listLoopArtifacts = vi.fn()
const listLoopValidations = vi.fn()
vi.mock("@/lib/loops-api", () => ({
  listLoopIterations: (...a: unknown[]) => listLoopIterations(...a),
  listLoopArtifacts: (...a: unknown[]) => listLoopArtifacts(...a),
  listLoopValidations: (...a: unknown[]) => listLoopValidations(...a),
}))

vi.mock("@/components/loops/loop-realtime-context", () => ({
  useLoopRealtime: () => ({ register: () => () => {} }),
}))

// The single IterationDialog now lives in the overlays context; the list opens
// it by dispatch, so we assert the opener is called with the right args.
const openIteration = vi.fn()
vi.mock("@/components/loops/loop-overlays-context", () => ({
  useLoopOverlays: () => ({ openIteration }),
}))

function iter(over: Partial<LoopIterationRow>): LoopIterationRow {
  return {
    id: 1,
    issue_id: 1,
    issue_seq: 1,
    stage: "implement",
    target_artifact_id: 10,
    target_title: null,
    conversation_id: null,
    status: "succeeded",
    launched_by: "engine",
    attempt: 0,
    tokens_used: 1234,
    created_at: "2026-06-14T00:00:00Z",
    started_at: null,
    ended_at: null,
    ...over,
  }
}

function artifact(over: Partial<LoopArtifactRow>): LoopArtifactRow {
  return {
    id: 20,
    issue_id: 1,
    issue_seq: 1,
    kind: "review",
    title: "Review note",
    status: "done",
    origin: "agent",
    produced_by_iteration_id: 1,
    verdict: null,
    attempt: 0,
    sort: 0,
    updated_at: "2026-06-14T00:00:00Z",
    ...over,
  }
}

function run(over: Partial<LoopValidationRunRow>): LoopValidationRunRow {
  return {
    id: 30,
    task_artifact_id: 10,
    iteration_id: 1,
    commands: ["pnpm test"],
    exit_codes: [0],
    passed: true,
    created_at: "2026-06-14T00:00:00Z",
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  listLoopArtifacts.mockResolvedValue([])
  listLoopValidations.mockResolvedValue([])
  listLoopIterations.mockResolvedValue([])
})

describe("IterationList", () => {
  it("renders an iteration row and expands to its artifacts and runs", async () => {
    listLoopIterations.mockResolvedValue([iter({ id: 1, conversation_id: 55 })])
    listLoopArtifacts.mockResolvedValue([
      artifact({ produced_by_iteration_id: 1 }),
    ])
    listLoopValidations.mockResolvedValue([run({ iteration_id: 1 })])
    render(<IterationList spaceId={1} />)

    // Row shows stage + tokens; produced artifact hidden until expanded.
    expect(await screen.findByText("implement")).toBeInTheDocument()
    expect(screen.queryByText("Review note")).not.toBeInTheDocument()

    fireEvent.click(screen.getByLabelText("expand"))
    expect(screen.getByText("Review note")).toBeInTheDocument() // produced artifact
    expect(screen.getByText("passed")).toBeInTheDocument() // validation verdict
    expect(screen.getByText("pnpm test")).toBeInTheDocument() // run command
  })

  it("opens the conversation viewer for an iteration with a conversation", async () => {
    listLoopIterations.mockResolvedValue([iter({ id: 1, conversation_id: 55 })])
    render(<IterationList spaceId={1} />)

    fireEvent.click(await screen.findByLabelText("openConversation"))
    expect(openIteration).toHaveBeenCalledWith({ conversationId: 55 })
  })

  it("scopes the fetch to one issue when issueId is given", async () => {
    render(<IterationList spaceId={1} issueId={9} />)
    await waitFor(() => expect(listLoopIterations).toHaveBeenCalledWith(1, 9))
  })
})
