import React, { useState } from "react"
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import userEvent from "@testing-library/user-event"
import enMessages from "@/i18n/messages/en.json"

import { ConversationShell } from "@/components/chat/conversation-shell"
import { MessageInput } from "@/components/chat/message-input"
import { PipelineModeSwitch } from "@/components/chat/composer/pipeline-mode-switch"
import { PipelineDiffPanel } from "@/components/chat/pipeline-diff-panel"
import { PipelineRunCard } from "@/components/chat/pipeline-run-card"
import { usePipelineRun } from "@/hooks/use-pipeline-run"
import * as api from "@/lib/api"
import type {
  PipelineChange,
  PipelineDiff,
  PipelineModeKey,
  PipelineRun,
  PromptCapabilitiesInfo,
} from "@/lib/types"
import {
  mockDuetGraph,
  mockDuetPreset,
  mockInitialDuetRun,
  mockPipelineDiff,
  mockPipelineEvents,
  mockReviewChangesRequestedRun,
  mockRound2CoderRun,
  mockSucceededRun,
} from "@/test-utils/pipeline-fixtures"

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>()
  return {
    ...actual,
    PIPELINE_CHANGED_EVENT: "pipeline://changed",
    pipelinePresets: vi.fn(),
    pipelineList: vi.fn(),
    pipelineRun: vi.fn(),
    pipelineRunStatus: vi.fn(),
    pipelineCancel: vi.fn(),
    pipelineRequestChanges: vi.fn(),
    pipelineStopManual: vi.fn(),
    pipelineRunDiff: vi.fn(),
    pipelineRunApply: vi.fn(),
    subscribePipelineChanged: vi.fn(),
  }
})

vi.mock("@/hooks/use-enabled-skill-ids", () => ({
  useEnabledSkillIds: () => ({
    enabledIds: new Set(),
    ready: false,
    supported: true,
  }),
}))

vi.mock("@/hooks/use-agent-skills", () => ({ useAgentSkills: () => [] }))
vi.mock("@/hooks/use-built-in-experts", () => ({
  useBuiltInExperts: () => [],
}))
vi.mock("@/hooks/use-built-in-science", () => ({
  useBuiltInScience: () => [],
}))
vi.mock("@/hooks/use-custom-skills", () => ({ useCustomSkills: () => [] }))

vi.mock("@/lib/transport", () => ({
  getActiveRemoteConnectionId: () => null,
  isDesktop: () => false,
  getTransport: () => ({
    call: vi.fn(),
    subscribe: vi.fn(async () => () => {}),
  }),
  getShellTransport: () => ({
    call: vi.fn(),
    subscribe: vi.fn(async () => () => {}),
  }),
}))

const CAPS: PromptCapabilitiesInfo = {
  image: true,
  audio: false,
  embedded_context: true,
}

function renderWithIntl(ui: React.ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

/**
 * Integrated behavioral harness for testing the complete end-to-end pipeline flow:
 * Composer Mode Selection -> Run Trigger -> Live Event Stream -> Review Feedback -> Fix Round -> Apply
 */
function IntegratedPipelineHarness({
  folderId = 10,
  initialRunId = null as number | null,
  onRunCreated,
}: {
  folderId?: number
  initialRunId?: number | null
  onRunCreated?: (run: PipelineRun) => void
}) {
  const [mode, setMode] = useState<PipelineModeKey>("single")
  const [activeRunId, setActiveRunId] = useState<number | null>(initialRunId)
  const [diff, setDiff] = useState<PipelineDiff | null>(null)
  const run = usePipelineRun(activeRunId)

  const handleSendDuet = async () => {
    const newRun = await api.pipelineRun({
      folderId,
      graph: mockDuetGraph,
      promptBlocks: [{ type: "text", text: "Implement greeting feature" }],
      displayText: "Implement greeting feature",
    })
    setActiveRunId(newRun.id)
    onRunCreated?.(newRun)
  }

  const handleRequestChanges = async (notes: string) => {
    if (!activeRunId) return
    await api.pipelineRequestChanges(activeRunId, notes)
  }

  const handleCardApply = async (
    runId: number,
    strategy: "squash" | "no_ff" = "squash"
  ) => {
    await api.pipelineRunApply(runId, strategy)
  }

  const handleDiffApply = async (strategy: "squash" | "no_ff" = "squash") => {
    if (!activeRunId) return
    await api.pipelineRunApply(activeRunId, strategy)
  }

  const handleLoadDiff = async () => {
    if (!activeRunId) return
    const fetchedDiff = await api.pipelineRunDiff(activeRunId)
    setDiff(fetchedDiff)
  }

  return (
    <div data-testid="integrated-pipeline-harness" className="space-y-4 p-4">
      {/* 1. Composer Pipeline Mode Switch */}
      <PipelineModeSwitch
        folderId={folderId}
        mode={mode}
        onModeChange={setMode}
      />

      <div className="flex gap-2">
        <button
          type="button"
          data-testid="composer-send-button"
          onClick={() => void handleSendDuet()}
          className="btn-send"
        >
          Send Task
        </button>
        <button
          type="button"
          data-testid="load-diff-trigger"
          onClick={() => void handleLoadDiff()}
          className="btn-diff"
        >
          Load Diff
        </button>
      </div>

      {/* 2. Pipeline Execution Card */}
      {run && (
        <div data-testid="pipeline-card-wrapper">
          <PipelineRunCard
            run={run}
            onStop={api.pipelineCancel}
            onApply={handleCardApply}
          />
        </div>
      )}

      {/* 3. Pipeline Review & Diff Panel */}
      {diff && (
        <div data-testid="pipeline-diff-wrapper">
          <PipelineDiffPanel
            diff={diff}
            onRequestChanges={handleRequestChanges}
            onApply={handleDiffApply}
          />
        </div>
      )}
    </div>
  )
}

describe("Pipeline Multi-Agent Flow Integration Tests", () => {
  let eventListeners: Array<(change: PipelineChange) => void> = []
  let currentRunStatus: PipelineRun

  const emitPipelineChange = async (change: PipelineChange) => {
    await act(async () => {
      for (const listener of eventListeners) {
        listener(change)
      }
    })
  }

  beforeEach(() => {
    eventListeners = []
    currentRunStatus = mockInitialDuetRun

    vi.mocked(api.pipelinePresets).mockResolvedValue([mockDuetPreset])
    vi.mocked(api.pipelineList).mockResolvedValue([])
    vi.mocked(api.pipelineRun).mockResolvedValue(mockInitialDuetRun)
    vi.mocked(api.pipelineRunStatus).mockImplementation(
      async () => currentRunStatus
    )
    vi.mocked(api.pipelineRunDiff).mockResolvedValue(mockPipelineDiff)
    vi.mocked(api.pipelineRequestChanges).mockResolvedValue(undefined)
    vi.mocked(api.pipelineStopManual).mockResolvedValue(undefined)
    vi.mocked(api.pipelineRunApply).mockResolvedValue(undefined)
    vi.mocked(api.pipelineCancel).mockResolvedValue(undefined)

    vi.mocked(api.subscribePipelineChanged).mockImplementation(
      async (handler) => {
        eventListeners.push(handler)
        return () => {
          eventListeners = eventListeners.filter((l) => l !== handler)
        }
      }
    )
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it("executes complete Duet lifecycle: mode select -> submit -> Code step -> Review feedback (changes_requested) -> add note & Send to coder -> Round 2 -> Pass -> Succeeded -> Apply", async () => {
    renderWithIntl(<IntegratedPipelineHarness />)

    // Step 1: In composer, select Duet mode
    const duetOption = await screen.findByRole("radio", { name: "Duet" })
    expect(duetOption).toBeInTheDocument()

    await act(async () => {
      fireEvent.click(duetOption)
    })

    expect(duetOption).toHaveAttribute("data-state", "active")

    // Step 2: Send task -> triggers pipelineRun with Duet preset graph
    const sendBtn = screen.getByTestId("composer-send-button")
    await act(async () => {
      fireEvent.click(sendBtn)
    })

    expect(api.pipelineRun).toHaveBeenCalledWith(
      expect.objectContaining({
        folderId: 10,
        graph: mockDuetGraph,
        displayText: "Implement greeting feature",
      })
    )

    // Step 3: PipelineRunCard mounts and displays initial Coder step (Round 1 of 3)
    await waitFor(() => {
      expect(screen.getByTestId("pipeline-card-wrapper")).toBeInTheDocument()
    })

    const cardWrapper = screen.getByTestId("pipeline-card-wrapper")
    expect(within(cardWrapper).getByText("Running")).toBeInTheDocument()
    expect(within(cardWrapper).getByText("Step 1 of 2")).toBeInTheDocument()
    expect(within(cardWrapper).getByText("Round 1 of 3")).toBeInTheDocument()
    expect(within(cardWrapper).getByText("Coder")).toBeInTheDocument()

    // Step 4: Emit Review step event settling with verdict 'changes_requested' and reviewer notes
    currentRunStatus = mockReviewChangesRequestedRun
    await emitPipelineChange(
      mockPipelineEvents.stepSettled(1, 2, "changes_requested")
    )

    // Card updates with Changes requested verdict and notes
    await waitFor(() => {
      expect(
        within(cardWrapper).getByText("Changes requested")
      ).toBeInTheDocument()
    })
    expect(
      within(cardWrapper).getByText("Notes for the coder")
    ).toBeInTheDocument()
    expect(
      within(cardWrapper).getByText(
        "src/main.rs:2: please optimize string formatting"
      )
    ).toBeInTheDocument()

    // Step 5: Open diff panel, add note to line, and click "Send to coder"
    const loadDiffBtn = screen.getByTestId("load-diff-trigger")
    await act(async () => {
      fireEvent.click(loadDiffBtn)
    })

    await waitFor(() => {
      expect(screen.getByTestId("pipeline-diff-wrapper")).toBeInTheDocument()
    })

    const diffWrapper = screen.getByTestId("pipeline-diff-wrapper")
    expect(within(diffWrapper).getByText("src/main.rs")).toBeInTheDocument()

    // Add note to row-3 (line 2 of src/main.rs)
    const commentBtn = within(diffWrapper).getByTestId("line-comment-btn-row-3")
    await act(async () => {
      fireEvent.click(commentBtn)
    })

    const commentInput = within(diffWrapper).getByTestId("line-comment-input")
    await act(async () => {
      fireEvent.change(commentInput, {
        target: { value: "use formatted greeting" },
      })
    })

    const saveCommentBtn = within(diffWrapper).getByTestId("save-comment-btn")
    await act(async () => {
      fireEvent.click(saveCommentBtn)
    })

    expect(
      within(diffWrapper).getByTestId("notes-summary-bar")
    ).toHaveTextContent("src/main.rs:2")

    // Click "Send to coder" in PipelineDiffPanel
    const sendToCoderBtn = within(diffWrapper).getByTestId(
      "request-changes-button"
    )
    await act(async () => {
      fireEvent.click(sendToCoderBtn)
    })

    // Verify pipelineRequestChanges called with formatted note "path:line: note"
    expect(api.pipelineRequestChanges).toHaveBeenCalledWith(
      1,
      "src/main.rs:2: use formatted greeting"
    )

    // Step 6: Emit event starting Round 2 of 3 (Coder running)
    currentRunStatus = mockRound2CoderRun
    await emitPipelineChange(mockPipelineEvents.stepStarted(1, 3, "coder", 2))

    await waitFor(() => {
      expect(within(cardWrapper).getByText("Round 2 of 3")).toBeInTheDocument()
    })
    expect(within(cardWrapper).getByText("Step 1 of 2")).toBeInTheDocument()

    // Step 7: Emit Review pass event and run settled as succeeded
    currentRunStatus = mockSucceededRun
    await emitPipelineChange(mockPipelineEvents.stepSettled(1, 4, "pass"))
    await emitPipelineChange(mockPipelineEvents.runSettled(1, "succeeded"))

    await waitFor(() => {
      expect(within(cardWrapper).getByText("Succeeded")).toBeInTheDocument()
    })
    expect(within(cardWrapper).getByText("Pass")).toBeInTheDocument()

    // Step 8: Click Apply button on diff panel -> invokes pipelineRunApply with squash strategy
    const applyButton = within(diffWrapper).getByTestId("apply-button")
    await act(async () => {
      fireEvent.click(applyButton)
    })

    expect(api.pipelineRunApply).toHaveBeenCalledWith(1, "squash")
  })

  it("handles multi-line notes correctly formatted as sorted path:line: note blocks", async () => {
    renderWithIntl(<IntegratedPipelineHarness initialRunId={1} />)

    // Load diff panel
    const loadDiffBtn = screen.getByTestId("load-diff-trigger")
    await act(async () => {
      fireEvent.click(loadDiffBtn)
    })

    await waitFor(() => {
      expect(screen.getByTestId("pipeline-diff-wrapper")).toBeInTheDocument()
    })

    const diffWrapper = screen.getByTestId("pipeline-diff-wrapper")

    // Comment on row 3 (line 2 of src/main.rs)
    await act(async () => {
      fireEvent.click(within(diffWrapper).getByTestId("line-comment-btn-row-3"))
    })
    await act(async () => {
      fireEvent.change(within(diffWrapper).getByTestId("line-comment-input"), {
        target: { value: "add error log" },
      })
      fireEvent.click(within(diffWrapper).getByTestId("save-comment-btn"))
    })

    // Switch to src/lib.rs in file tree
    const libTreeItem = within(diffWrapper).getByTestId(
      "file-tree-item-src/lib.rs"
    )
    await act(async () => {
      fireEvent.click(libTreeItem)
    })

    // Comment on row 1 (line 1 of src/lib.rs)
    await act(async () => {
      fireEvent.click(within(diffWrapper).getByTestId("line-comment-btn-row-1"))
    })
    await act(async () => {
      fireEvent.change(within(diffWrapper).getByTestId("line-comment-input"), {
        target: { value: "export doc comments" },
      })
      fireEvent.click(within(diffWrapper).getByTestId("save-comment-btn"))
    })

    // Click "Send to coder"
    await act(async () => {
      fireEvent.click(within(diffWrapper).getByTestId("request-changes-button"))
    })

    // Expect alphabetical order by path: src/lib.rs:1 before src/main.rs:2
    expect(api.pipelineRequestChanges).toHaveBeenCalledWith(
      1,
      "src/lib.rs:1: export doc comments\nsrc/main.rs:2: add error log"
    )
  })

  it("invokes pipelineRunApply directly from PipelineRunCard when run status is succeeded", async () => {
    currentRunStatus = mockSucceededRun
    renderWithIntl(<IntegratedPipelineHarness initialRunId={1} />)

    await waitFor(() => {
      expect(screen.getByTestId("pipeline-card-wrapper")).toBeInTheDocument()
    })

    const cardWrapper = screen.getByTestId("pipeline-card-wrapper")
    expect(within(cardWrapper).getByText("Succeeded")).toBeInTheDocument()

    // Click Apply changes button rendered in PipelineRunCard actions footer
    const cardApplyButton = within(cardWrapper).getByRole("button", {
      name: /apply changes/i,
    })
    await act(async () => {
      fireEvent.click(cardApplyButton)
    })

    expect(api.pipelineRunApply).toHaveBeenCalledWith(1, "squash")
  })

  it("ConversationShell automatically docks PipelineDiffPanel into composer dock when reviewer emits changes_requested verdict", async () => {
    currentRunStatus = mockInitialDuetRun
    renderWithIntl(
      <ConversationShell
        status={null}
        promptCapabilities={CAPS}
        error={null}
        claudeApiRetry={null}
        pendingPermission={null}
        pendingQuestion={null}
        pendingAskQuestion={null}
        pendingPlanApproval={null}
        onFocus={vi.fn()}
        onSend={vi.fn()}
        onCancel={vi.fn()}
        onRespondPermission={vi.fn()}
        onAnswerQuestion={vi.fn()}
        onAnswerAskQuestion={vi.fn()}
        onAnswerPlanApproval={vi.fn()}
        pipelineRunId={1}
      >
        <div data-testid="chat-messages">Messages</div>
      </ConversationShell>
    )

    // Initial state: Card is mounted, no diff panel
    await waitFor(() => {
      expect(screen.getByText("Step 1 of 2")).toBeInTheDocument()
    })
    expect(
      screen.queryByTestId("pipeline-diff-wrapper")
    ).not.toBeInTheDocument()

    // Emit Reviewer settled with changes_requested
    currentRunStatus = mockReviewChangesRequestedRun
    await emitPipelineChange(
      mockPipelineEvents.stepSettled(1, 2, "changes_requested")
    )

    // Diff panel automatically loads and docks into view
    await waitFor(() => {
      expect(screen.getByTestId("pipeline-diff-wrapper")).toBeInTheDocument()
    })
    expect(api.pipelineRunDiff).toHaveBeenCalledWith(1)
    expect(screen.getByText("src/main.rs")).toBeInTheDocument()
  })

  it("MessageInput composer forwards full preset graph in pipelineRun payload when preset mode is selected", async () => {
    const onSend = vi.fn()
    const { container } = renderWithIntl(
      <MessageInput
        onSend={onSend}
        promptCapabilities={CAPS}
        folderPickerOverride={{
          folderId: 10,
          editable: true,
          onSelectFolder: vi.fn(),
          onSelectChatMode: vi.fn(),
        }}
      />
    )

    // Select Duet mode in PipelineModeSwitch
    const duetOption = await screen.findByRole("radio", { name: "Duet" })
    await act(async () => {
      fireEvent.click(duetOption)
    })
    expect(duetOption).toHaveAttribute("data-state", "active")

    await waitFor(
      () => expect(container.querySelector('[role="textbox"]')).not.toBeNull(),
      { timeout: 5000 }
    )

    const textbox = container.querySelector('[role="textbox"]') as HTMLElement
    expect(textbox).not.toBeNull()

    await userEvent.click(textbox)
    await userEvent.keyboard("Build new feature")

    const sendButton = screen.getByRole("button", { name: /send/i })
    await userEvent.click(sendButton)

    await waitFor(() => {
      expect(api.pipelineRun).toHaveBeenCalledWith(
        expect.objectContaining({
          folderId: 10,
          graph: mockDuetGraph,
          displayText: "Build new feature",
          isolation: "worktree_per_run",
        })
      )
    })
    expect(onSend).not.toHaveBeenCalled()
  })

  it("automatically mounts PipelineRunCard in ConversationShell when sending a task in Duet mode without initial pipelineRunId", async () => {
    const { container } = renderWithIntl(
      <ConversationShell
        status="connected"
        promptCapabilities={CAPS}
        error={null}
        claudeApiRetry={null}
        pendingPermission={null}
        pendingQuestion={null}
        pendingAskQuestion={null}
        pendingPlanApproval={null}
        onFocus={vi.fn()}
        onSend={vi.fn()}
        onCancel={vi.fn()}
        onRespondPermission={vi.fn()}
        onAnswerQuestion={vi.fn()}
        onAnswerAskQuestion={vi.fn()}
        onAnswerPlanApproval={vi.fn()}
        attachmentTabId="tab-test-duet"
        folderPickerOverride={{
          folderId: 10,
          editable: true,
          onSelectFolder: vi.fn(),
          onSelectChatMode: vi.fn(),
        }}
      >
        <div data-testid="chat-messages">Messages</div>
      </ConversationShell>
    )

    // Before send: no PipelineRunCard
    expect(screen.queryByText("Step 1 of 2")).not.toBeInTheDocument()
    expect(screen.queryByText("Coder")).not.toBeInTheDocument()

    // Select Duet mode in PipelineModeSwitch
    const duetOption = await screen.findByRole("radio", { name: "Duet" })
    await act(async () => {
      fireEvent.click(duetOption)
    })
    expect(duetOption).toHaveAttribute("data-state", "active")

    await waitFor(
      () => expect(container.querySelector('[role="textbox"]')).not.toBeNull(),
      { timeout: 5000 }
    )

    const textbox = container.querySelector('[role="textbox"]') as HTMLElement
    await userEvent.click(textbox)
    await userEvent.keyboard("Implement greeting feature")

    const sendButton = screen.getByRole("button", { name: /send/i })
    await userEvent.click(sendButton)

    // Verify pipelineRun API called
    await waitFor(() => {
      expect(api.pipelineRun).toHaveBeenCalledWith(
        expect.objectContaining({
          folderId: 10,
          graph: mockDuetGraph,
          displayText: "Implement greeting feature",
        })
      )
    })

    // Verify PipelineRunCard mounts and renders in ConversationShell
    await waitFor(() => {
      expect(screen.getByText("Step 1 of 2")).toBeInTheDocument()
    })
    expect(screen.getByText("Running")).toBeInTheDocument()
    expect(screen.getByText("Round 1 of 3")).toBeInTheDocument()
    expect(screen.getAllByText("Coder")[0]).toBeInTheDocument()
  })
})
