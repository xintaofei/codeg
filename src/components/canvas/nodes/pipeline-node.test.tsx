import type { ComponentProps } from "react"
import { describe, it, expect, vi, beforeEach } from "vitest"
import { render, screen, waitFor } from "@testing-library/react"
import { IntlProvider } from "next-intl"
import { ReactFlowProvider } from "@xyflow/react"
import messages from "@/i18n/messages/en.json"
import { PipelineNode } from "./pipeline-node"
import type { PipelineNodeData } from "../canvas-model"
import type { CanvasNode } from "@/lib/types"
import * as api from "@/lib/api"

vi.mock("@/lib/api")

const mockPipeline = {
  id: 1,
  name: "Test Pipeline",
  preset_key: null,
  folder_id: null,
  graph: {
    steps: [
      {
        id: "planner_0",
        role: "planner" as const,
        label: "Planner",
        agent_type: "claude_code",
        mode_id: null,
        config_values: {},
        prompt_template: "$task",
        timeout_secs: 1800,
        read_memory: false,
        read_only: false,
      },
      {
        id: "coder_0",
        role: "coder" as const,
        label: "Coder",
        agent_type: "claude_code",
        mode_id: null,
        config_values: {},
        prompt_template: "$plan",
        timeout_secs: 1800,
        read_memory: false,
        read_only: false,
      },
    ],
    loops: [],
  },
  isolation: "worktree_per_run" as const,
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
}

function makePipelineNode(over: Partial<CanvasNode> = {}): CanvasNode {
  return {
    id: 1,
    kind: "pipeline",
    folder_id: null,
    folder_group_id: null,
    conversation_id: null,
    pipeline_id: 1,
    agent_type: null,
    member_ids: [],
    title: "Test Pipeline",
    content: null,
    path: null,
    color: null,
    collapsed: false,
    grid_columns: 0,
    grid_rows: 0,
    x: 0,
    y: 0,
    width: 580,
    height: 380,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    deleted_at: null,
    ...over,
  } as CanvasNode
}

function renderPipelineNode(dbNode: CanvasNode) {
  const props = {
    id: `node-${dbNode.id}`,
    data: {
      dbNode,
      pipelineId: dbNode.pipeline_id ?? null,
      label: dbNode.title ?? "Pipeline",
    } satisfies PipelineNodeData,
    selected: false,
    type: "pipeline" as const,
    zIndex: 1,
    isConnectable: false,
    positionAbsoluteX: dbNode.x,
    positionAbsoluteY: dbNode.y,
    dragging: false,
    draggable: true,
    selectable: true,
    deletable: true,
  }

  return render(
    <IntlProvider locale="en" messages={messages}>
      <ReactFlowProvider>
        <PipelineNode
          {...(props as unknown as ComponentProps<typeof PipelineNode>)}
        />
      </ReactFlowProvider>
    </IntlProvider>
  )
}

describe("PipelineNode", () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it("renders pipeline node with title", async () => {
    vi.mocked(api.pipelineGet).mockResolvedValue(mockPipeline)

    const dbNode = makePipelineNode()
    renderPipelineNode(dbNode)

    expect(screen.getByText("Test Pipeline")).toBeInTheDocument()
  })

  it("loads pipeline graph when mounted", async () => {
    vi.mocked(api.pipelineGet).mockResolvedValue(mockPipeline)

    const dbNode = makePipelineNode()
    renderPipelineNode(dbNode)

    await waitFor(() => {
      expect(vi.mocked(api.pipelineGet)).toHaveBeenCalledWith(1)
    })
  })

  it("renders loading state initially", () => {
    vi.mocked(api.pipelineGet).mockImplementation(() => new Promise(() => {}))

    const dbNode = makePipelineNode()
    renderPipelineNode(dbNode)

    expect(screen.getByText(/loading/i)).toBeInTheDocument()
  })

  it("renders error message when pipeline load fails", async () => {
    vi.mocked(api.pipelineGet).mockRejectedValue(
      new Error("Failed to load pipeline")
    )

    const dbNode = makePipelineNode()
    renderPipelineNode(dbNode)

    await waitFor(
      () => {
        expect(screen.getByText(/failed to load pipeline/i)).toBeInTheDocument()
      },
      { timeout: 2000 }
    )
  })

  it("displays all steps from the pipeline graph", async () => {
    vi.mocked(api.pipelineGet).mockResolvedValue(mockPipeline)

    const dbNode = makePipelineNode()
    renderPipelineNode(dbNode)

    await waitFor(() => {
      expect(screen.getByText("Planner")).toBeInTheDocument()
      expect(screen.getByText("Coder")).toBeInTheDocument()
    })
  })

  it("shows nodrag and nowheel classes to prevent drag propagation", async () => {
    vi.mocked(api.pipelineGet).mockResolvedValue(mockPipeline)

    const dbNode = makePipelineNode()
    const { container } = renderPipelineNode(dbNode)

    const nodeContainer = container.querySelector(".nodrag.nowheel")
    expect(nodeContainer).toBeInTheDocument()
  })
})
