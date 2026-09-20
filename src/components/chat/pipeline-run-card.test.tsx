import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import type {
  PipelineRun,
  PipelineRunStatus,
  PipelineVerdict,
} from "@/lib/types"
import { PipelineRunCard, type PipelineRunCardProps } from "./pipeline-run-card"

const messages = {
  Pipeline: {
    title: "Agent pipeline",
    modeSingle: "Single agent",
    modeDuet: "Duet",
    modeTeam: "Team",
    modeCustom: "Custom",
    modeHintDuet: "Coder writes, reviewer checks, up to {n} fix rounds",
    modeHintTeam: "Planner, coder, reviewer and tests with fix rounds",
    rolePlanner: "Planner",
    roleCoder: "Coder",
    roleReviewer: "Reviewer",
    roleTests: "Tests",
    roleCustom: "Custom",
    loopLimit: "Fix rounds: up to {n}",
    agent: "Agent",
    model: "Model",
    modelUnconfirmed: "Model not confirmed",
    mode: "Mode",
    prompt: "Prompt",
    promptHint: "Placeholders: $task, $plan, $summary, $review, $memory",
    timeout: "Timeout (min)",
    maxIterations: "Max fix rounds",
    readMemory: "Read memory before this step",
    readOnly: "Read-only step",
    readOnlyUnsupported:
      "This agent has no read-only mode; the reviewer may edit files",
    isolation: "Isolation",
    isolationWorktree: "New worktree per run",
    isolationShared: "Run in the folder",
    run: "Run pipeline",
    running: "Running",
    step: "Step {index} of {total}",
    iteration: "Round {n} of {max}",
    verdictPass: "Pass",
    verdictChangesRequested: "Changes requested",
    verdictInconclusive: "Inconclusive",
    verdictNone: "No verdict",
    verdictSourceMarker: "from text marker",
    verdictSourceGuard: "reviewer modified files",
    statusSucceeded: "Succeeded",
    statusFailed: "Failed",
    statusCancelled: "Cancelled",
    statusInterrupted: "Interrupted by restart",
    statusStoppedMaxIterations: "Stopped: fix round limit reached",
    statusInconclusive: "Stopped: inconclusive verdict",
    openConversation: "Open conversation",
    openCode: "Open code",
    stop: "Stop",
    stopping: "Stopping, waiting for the agent to finish its turn",
    sendToCoder: "Send to coder",
    fixMyself: "I will fix it myself",
    apply: "Apply changes",
    applySquash: "Squash merge",
    applyNoFf: "Merge commit",
    applied: "Changes applied",
    diffEmpty: "No changes yet",
    diffTruncated: "Diff truncated, open the worktree to see everything",
    commentPlaceholder: "Note for the coder about this line",
    addComment: "Add note",
    notesForCoder: "Notes for the coder",
    alreadyRunning: "A pipeline is already running in this folder",
    engineUnavailable: "Pipeline engine is not running",
    savePreset: "Save as preset",
    presetName: "Preset name",
    presets: "Presets",
    deletePreset: "Delete preset",
    presetInUse: "This preset is used by an automation",
    canvasNode: "Pipeline",
    canvasAddPipeline: "Pipeline",
    canvasNodeVerdict: "Review verdict",
    canvasNodeMemory: "Memory",
    canvasLoopEdge: "on changes requested",
    inspectorTitle: "Step settings",
    validationError: "Pipeline is invalid: {message}",
    validation: {
      empty: "Add at least one step",
      tooManySteps: "At most 8 steps",
      duplicateStepId: "Duplicate step id {id}",
      loopFromWrongRole: "Only reviewer or tests steps can loop back",
      loopTargetNotEarlier: "A loop must point to an earlier step",
      duplicateLoop: "One loop per step",
      badMaxIterations: "Fix rounds must be between 1 and 10",
      emptyPrompt: "Step {id} has an empty prompt",
      unknownAgent: "Unknown agent in step {id}",
    },
    automationAction: "Run pipeline",
    automationPipeline: "Pipeline",
  },
}

function makeMockRun(over: Partial<PipelineRun> = {}): PipelineRun {
  return {
    id: 1,
    pipeline_id: 10,
    folder_id: 100,
    worktree_folder_id: 101,
    parent_conversation_id: 50,
    graph: {
      steps: [
        {
          id: "coder",
          role: "coder",
          label: "Coder",
          agent_type: "claude_code",
          config_values: {},
          prompt_template: "$task",
          timeout_secs: 1800,
          read_memory: false,
          read_only: false,
        },
        {
          id: "reviewer",
          role: "reviewer",
          label: "Reviewer",
          agent_type: "claude_code",
          config_values: {},
          prompt_template: "Review",
          timeout_secs: 1800,
          read_memory: false,
          read_only: true,
        },
      ],
      loops: [
        {
          from_step: "reviewer",
          to_step: "coder",
          max_iterations: 3,
        },
      ],
    },
    status: "running",
    current_step_id: "coder",
    current_iteration: 1,
    error: null,
    attempts: [
      {
        id: 1,
        run_id: 1,
        step_id: "coder",
        iteration: 1,
        status: "running",
        conversation_id: 51,
        model_requested: "claude-3-7-sonnet",
        model_actual: "claude-3-7-sonnet-20250219",
        verdict: null,
        verdict_source: null,
        notes: null,
        summary: null,
        started_at: "2026-09-19T10:00:00Z",
        ended_at: null,
      },
    ],
    started_at: "2026-09-19T10:00:00Z",
    ended_at: null,
    ...over,
  }
}

function renderCard(props: PipelineRunCardProps) {
  return render(
    <NextIntlClientProvider locale="en" messages={messages}>
      <PipelineRunCard {...props} />
    </NextIntlClientProvider>
  )
}

describe("PipelineRunCard", () => {
  const statuses: Array<{ status: PipelineRunStatus; expectedLabel: string }> =
    [
      { status: "running", expectedLabel: "Running" },
      { status: "succeeded", expectedLabel: "Succeeded" },
      { status: "failed", expectedLabel: "Failed" },
      { status: "cancelled", expectedLabel: "Cancelled" },
      { status: "interrupted", expectedLabel: "Interrupted by restart" },
      {
        status: "stopped_max_iterations",
        expectedLabel: "Stopped: fix round limit reached",
      },
      {
        status: "inconclusive",
        expectedLabel: "Stopped: inconclusive verdict",
      },
    ]

  statuses.forEach(({ status, expectedLabel }) => {
    it(`renders status badge for "${status}"`, () => {
      const run = makeMockRun({ status })
      renderCard({ run })
      expect(screen.getByText(expectedLabel)).toBeDefined()
    })
  })

  it("renders step N of M and round N of M", () => {
    const run = makeMockRun({
      current_step_id: "reviewer",
      current_iteration: 2,
    })
    renderCard({ run })
    expect(screen.getByText("Step 2 of 2")).toBeDefined()
    expect(screen.getByText("Round 2 of 3")).toBeDefined()
  })

  it("renders actual model when present", () => {
    const run = makeMockRun({
      attempts: [
        {
          id: 1,
          run_id: 1,
          step_id: "coder",
          iteration: 1,
          status: "done",
          conversation_id: 51,
          model_requested: "gpt-4",
          model_actual: "gpt-4o-2024-08-06",
          verdict: null,
          verdict_source: null,
          notes: null,
          summary: null,
          started_at: "2026-09-19T10:00:00Z",
          ended_at: "2026-09-19T10:05:00Z",
        },
      ],
    })
    renderCard({ run })
    expect(screen.getByText("gpt-4o-2024-08-06")).toBeDefined()
  })

  it("renders modelUnconfirmed when model_actual is missing", () => {
    const run = makeMockRun({
      attempts: [
        {
          id: 1,
          run_id: 1,
          step_id: "coder",
          iteration: 1,
          status: "running",
          conversation_id: 51,
          model_requested: null,
          model_actual: null,
          verdict: null,
          verdict_source: null,
          notes: null,
          summary: null,
          started_at: "2026-09-19T10:00:00Z",
          ended_at: null,
        },
      ],
    })
    renderCard({ run })
    expect(screen.getByText("Model not confirmed")).toBeDefined()
  })

  const verdicts: Array<{ verdict: PipelineVerdict; expected: string }> = [
    { verdict: "pass", expected: "Pass" },
    { verdict: "changes_requested", expected: "Changes requested" },
    { verdict: "inconclusive", expected: "Inconclusive" },
  ]

  verdicts.forEach(({ verdict, expected }) => {
    it(`renders verdict badge for "${verdict}"`, () => {
      const run = makeMockRun({
        attempts: [
          {
            id: 1,
            run_id: 1,
            step_id: "reviewer",
            iteration: 1,
            status: "done",
            conversation_id: 51,
            model_requested: null,
            model_actual: "claude-3-7-sonnet",
            verdict,
            verdict_source: "marker",
            notes: "Some reviewer notes",
            summary: null,
            started_at: "2026-09-19T10:00:00Z",
            ended_at: "2026-09-19T10:05:00Z",
          },
        ],
      })
      renderCard({ run })
      expect(screen.getByText(expected)).toBeDefined()
      expect(screen.getByText("(from text marker)")).toBeDefined()
      expect(screen.getByText("Notes for the coder")).toBeDefined()
      expect(screen.getByText("Some reviewer notes")).toBeDefined()
    })
  })

  it("renders guard verdict source label", () => {
    const run = makeMockRun({
      attempts: [
        {
          id: 1,
          run_id: 1,
          step_id: "reviewer",
          iteration: 1,
          status: "done",
          conversation_id: 51,
          model_requested: null,
          model_actual: "claude-3-7-sonnet",
          verdict: "inconclusive",
          verdict_source: "guard",
          notes: "reviewer modified files",
          summary: null,
          started_at: "2026-09-19T10:00:00Z",
          ended_at: "2026-09-19T10:05:00Z",
        },
      ],
    })
    renderCard({ run })
    expect(screen.getByText("(reviewer modified files)")).toBeDefined()
  })

  it("renders error message when present", () => {
    const run = makeMockRun({
      status: "failed",
      error: "Pipeline process crashed due to timeout",
    })
    renderCard({ run })
    expect(
      screen.getByText("Pipeline process crashed due to timeout")
    ).toBeDefined()
  })

  it("calls onStop when Stop button is clicked during running status", async () => {
    const onStop = vi.fn().mockResolvedValue(undefined)
    const run = makeMockRun({ status: "running" })
    renderCard({ run, onStop })

    const stopBtn = screen.getByRole("button", { name: /stop/i })
    await act(async () => {
      fireEvent.click(stopBtn)
    })

    expect(onStop).toHaveBeenCalledWith(run.id)
  })

  it("calls onOpenConversation when Open conversation button is clicked", () => {
    const onOpenConversation = vi.fn()
    const run = makeMockRun({
      attempts: [
        {
          id: 1,
          run_id: 1,
          step_id: "coder",
          iteration: 1,
          status: "running",
          conversation_id: 77,
          model_requested: null,
          model_actual: null,
          verdict: null,
          verdict_source: null,
          notes: null,
          summary: null,
          started_at: "2026-09-19T10:00:00Z",
          ended_at: null,
        },
      ],
    })
    renderCard({ run, onOpenConversation })

    const convBtn = screen.getByRole("button", { name: /open conversation/i })
    fireEvent.click(convBtn)

    expect(onOpenConversation).toHaveBeenCalledWith(77)
  })

  it("calls onOpenCode when Open code button is clicked", () => {
    const onOpenCode = vi.fn()
    const run = makeMockRun({ id: 123 })
    renderCard({ run, onOpenCode })

    const codeBtn = screen.getByRole("button", { name: /open code/i })
    fireEvent.click(codeBtn)

    expect(onOpenCode).toHaveBeenCalledWith(123)
  })

  it("calls onApply when Apply changes button is clicked on succeeded run", async () => {
    const onApply = vi.fn().mockResolvedValue(undefined)
    const run = makeMockRun({ status: "succeeded", id: 456 })
    renderCard({ run, onApply })

    const applyBtn = screen.getByRole("button", { name: /apply changes/i })
    await act(async () => {
      fireEvent.click(applyBtn)
    })

    expect(onApply).toHaveBeenCalledWith(456, "squash")
  })
})
