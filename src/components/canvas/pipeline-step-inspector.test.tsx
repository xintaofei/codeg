import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import type { LoopBack, PipelineStep } from "@/lib/types"
import { PipelineStepInspector } from "./pipeline-step-inspector"

vi.mock("@/lib/api", () => ({
  describeAgentOptions: vi.fn(async () => ({
    modes: null,
    available_commands: [],
    config_options: [
      {
        id: "model",
        name: "Model",
        kind: {
          type: "select" as const,
          current_value: "sonnet",
          options: [
            { value: "opus", name: "Opus 5" },
            { value: "sonnet", name: "Sonnet 5" },
          ],
          // Some agents publish their models only inside groups.
          groups: [
            {
              group: "fast",
              name: "Fast",
              options: [{ value: "haiku", name: "Haiku 4.5" }],
            },
          ],
        },
      },
    ],
  })),
}))

vi.mock("@/hooks/use-acp-agents", () => ({
  useAcpAgents: () => ({
    agents: [
      { agent_type: "claude_code", name: "Claude Code", enabled: true },
      { agent_type: "codex", name: "Codex", enabled: true },
      { agent_type: "gemini", name: "Gemini", enabled: true },
    ],
    fresh: true,
    refresh: vi.fn(),
  }),
}))

function makeStep(over: Partial<PipelineStep> = {}): PipelineStep {
  return {
    id: "coder_1",
    role: "coder",
    label: "Coder",
    agent_type: "claude_code",
    mode_id: null,
    config_values: { model: "claude-3-7-sonnet" },
    prompt_template: "$task",
    timeout_secs: 1800,
    read_memory: false,
    read_only: false,
    ...over,
  }
}

function renderInspector(props: {
  step?: PipelineStep
  loop?: LoopBack | null
  availableLoopTargets?: PipelineStep[]
  open?: boolean
  onClose?: () => void
  onSave?: (step: PipelineStep, loop?: LoopBack | null) => void
  onDeleteStep?: (id: string) => void
}) {
  const defaultProps = {
    step: makeStep(),
    loop: null,
    availableLoopTargets: [],
    open: true,
    onClose: vi.fn(),
    onSave: vi.fn(),
    ...props,
  }

  return {
    ...render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <PipelineStepInspector {...defaultProps} />
      </NextIntlClientProvider>
    ),
    props: defaultProps,
  }
}

describe("PipelineStepInspector", () => {
  it("renders with step data populated", () => {
    const step = makeStep({
      id: "coder_step",
      label: "Custom Coder",
      config_values: { model: "gpt-4o" },
      prompt_template: "Write unit tests for $task",
      timeout_secs: 1200,
    })

    renderInspector({ step })

    expect(screen.getByText("Step settings")).toBeDefined()
    expect(screen.getByDisplayValue("coder_step")).toBeDefined()
    expect(screen.getByDisplayValue("Custom Coder")).toBeDefined()
    expect(screen.getByDisplayValue("gpt-4o")).toBeDefined()
    expect(screen.getByDisplayValue("Write unit tests for $task")).toBeDefined()
    expect(screen.getByDisplayValue("20")).toBeDefined() // 1200 / 60 = 20 min
  })

  it("updates fields and calls onSave with modified step data", async () => {
    const onSave = vi.fn()
    const onClose = vi.fn()
    const step = makeStep()

    renderInspector({ step, onSave, onClose })

    const labelInput = screen.getByDisplayValue("Coder")
    fireEvent.change(labelInput, { target: { value: "Lead Coder" } })

    const promptInput = screen.getByDisplayValue("$task")
    fireEvent.change(promptInput, {
      target: { value: "Implement $task and summarize in $summary" },
    })

    const timeoutInput = screen.getByDisplayValue("30")
    fireEvent.change(timeoutInput, { target: { value: "45" } })

    const saveButton = screen.getByRole("button", { name: "Save" })
    fireEvent.click(saveButton)

    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "coder_1",
        label: "Lead Coder",
        prompt_template: "Implement $task and summarize in $summary",
        timeout_secs: 2700, // 45 * 60
      }),
      null
    )
    expect(onClose).toHaveBeenCalled()
  })

  it("validates empty prompt and prevents saving", () => {
    const onSave = vi.fn()
    const step = makeStep({ prompt_template: "" })

    renderInspector({ step, onSave })

    const promptInput = screen.getByPlaceholderText("$task")
    fireEvent.change(promptInput, { target: { value: "   " } })

    const saveButton = screen.getByRole("button", { name: "Save" })
    fireEvent.click(saveButton)

    expect(onSave).not.toHaveBeenCalled()
    expect(screen.getByText(/empty prompt/i)).toBeDefined()
  })

  it("validates bad step id and prevents saving", () => {
    const onSave = vi.fn()
    const step = makeStep()

    renderInspector({ step, onSave })

    const idInput = screen.getByDisplayValue("coder_1")
    fireEvent.change(idInput, { target: { value: "INVALID ID WITH SPACES" } })

    const saveButton = screen.getByRole("button", { name: "Save" })
    fireEvent.click(saveButton)

    expect(onSave).not.toHaveBeenCalled()
    expect(screen.getByText(/is invalid/i)).toBeDefined()
  })

  it("shows read-only unsupported warning for agents without read-only mode", () => {
    const step = makeStep({
      role: "reviewer",
      agent_type: "open_code",
      read_only: true,
    })

    renderInspector({ step })

    expect(
      screen.getByText(
        "This agent has no read-only mode; the reviewer may edit files"
      )
    ).toBeDefined()
  })

  it("supports configuring loopback for reviewer steps", () => {
    const onSave = vi.fn()
    const coderStep = makeStep({ id: "coder_step", role: "coder" })
    const reviewerStep = makeStep({
      id: "reviewer_step",
      role: "reviewer",
      label: "Reviewer",
    })

    renderInspector({
      step: reviewerStep,
      availableLoopTargets: [coderStep],
      onSave,
    })

    expect(screen.getByText("on changes requested")).toBeDefined()

    // Enable loopback
    const loopSwitch = screen.getByRole("switch", { name: "" }) // loop switch
    fireEvent.click(loopSwitch)

    const saveButton = screen.getByRole("button", { name: "Save" })
    fireEvent.click(saveButton)

    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({ id: "reviewer_step" }),
      expect.objectContaining({
        from_step: "reviewer_step",
        to_step: "coder_step",
        max_iterations: 3,
      })
    )
  })

  it("offers the agent's own models instead of a blank text box", async () => {
    // The model used to be free text: picking one meant knowing the agent's
    // internal id by heart, and a typo only surfaced when the step ran.
    renderInspector({ step: makeStep() })

    const model = await waitFor(() => {
      const el = document.getElementById("step-model")
      expect(el?.tagName).toBe("SELECT")
      return el as HTMLSelectElement
    })

    const values = Array.from(model.options).map((o) => o.value)
    // Grouped options count too — some agents publish models only in groups.
    expect(values).toEqual(["", "opus", "sonnet", "haiku"])

    fireEvent.change(model, { target: { value: "haiku" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
  })

  it("calls onDeleteStep when delete is confirmed", () => {
    const onDeleteStep = vi.fn()
    const step = makeStep({ id: "to_delete" })

    renderInspector({ step, onDeleteStep })

    const deleteBtn = screen.getByRole("button", { name: /delete/i })
    fireEvent.click(deleteBtn)

    expect(onDeleteStep).toHaveBeenCalledWith("to_delete")
  })
})
