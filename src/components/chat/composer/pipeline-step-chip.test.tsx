import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import { Code2 } from "lucide-react"

import enMessages from "@/i18n/messages/en.json"
import type { PipelineStep } from "@/lib/types"

import { PipelineStepChip } from "./pipeline-step-chip"

vi.mock("@/hooks/use-acp-agents", () => ({
  useAcpAgents: () => ({
    agents: [
      { agent_type: "claude_code", name: "Claude Code", enabled: true },
      { agent_type: "antigravity", name: "Google Antigravity", enabled: true },
    ],
    fresh: true,
    refresh: vi.fn(),
  }),
}))

vi.mock("@/lib/api", () => ({
  describeAgentOptions: vi.fn(async (agent: string) => ({
    modes: null,
    available_commands: [],
    config_options: [
      {
        id: "model",
        name: "Model",
        kind: {
          type: "select" as const,
          current_value: "",
          options:
            agent === "claude_code"
              ? [
                  { value: "opus", name: "Opus 5" },
                  { value: "haiku", name: "Haiku 4.5" },
                ]
              : [{ value: "gemini-3.8-flash-high", name: "Gemini 3.8 Flash" }],
          groups: [],
        },
      },
    ],
  })),
}))

function makeStep(over: Partial<PipelineStep> = {}): PipelineStep {
  return {
    id: "coder",
    role: "coder",
    label: "Coder",
    agent_type: "claude_code",
    mode_id: null,
    config_values: {},
    prompt_template: "$task",
    timeout_secs: 1800,
    read_memory: false,
    read_only: false,
    ...over,
  }
}

function renderChip(
  props: Partial<React.ComponentProps<typeof PipelineStepChip>> = {}
) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <PipelineStepChip
        step={makeStep()}
        RoleIcon={Code2}
        roleLabel="Coder"
        agentName="claude_code"
        modelName={null}
        {...props}
      />
    </NextIntlClientProvider>
  )
}

describe("PipelineStepChip", () => {
  it("stays a plain label when it cannot be edited", () => {
    // A caller with nowhere to persist an edit must not offer one: a popover
    // whose changes vanish is worse than no popover.
    renderChip()
    expect(screen.queryByRole("button")).toBeNull()
  })

  it("offers the agent's own models and reports the pick", async () => {
    const onChange = vi.fn()
    renderChip({ onChange })

    fireEvent.click(screen.getByRole("button"))
    const model = await waitFor(() => {
      const el = document.getElementById(
        "chip-model-coder"
      ) as HTMLSelectElement
      expect(el).not.toBeNull()
      expect(Array.from(el.options).map((o) => o.value)).toContain("haiku")
      return el
    })

    fireEvent.change(model, { target: { value: "haiku" } })
    expect(onChange).toHaveBeenCalledWith({ model: "haiku" })
  })

  it("clears the model when the agent changes", async () => {
    // The old agent's model id almost never exists on the new one, so the two
    // have to move together — saving them apart would persist an invalid pair.
    const onChange = vi.fn()
    renderChip({
      onChange,
      step: makeStep({ config_values: { model: "opus" } }),
    })

    fireEvent.click(screen.getByRole("button"))
    const agent = await waitFor(() => {
      const el = document.getElementById(
        "chip-agent-coder"
      ) as HTMLSelectElement
      expect(el).not.toBeNull()
      return el
    })

    fireEvent.change(agent, { target: { value: "antigravity" } })
    expect(onChange).toHaveBeenCalledWith({
      agentType: "antigravity",
      model: "",
    })
  })

  it("keeps a saved model the agent no longer lists", async () => {
    // Renamed or retired upstream: dropping it silently would swap the step
    // onto whatever happens to be first in the list.
    renderChip({
      onChange: vi.fn(),
      step: makeStep({ config_values: { model: "claude-3-retired" } }),
    })

    fireEvent.click(screen.getByRole("button"))
    await waitFor(() => {
      const el = document.getElementById(
        "chip-model-coder"
      ) as HTMLSelectElement
      expect(Array.from(el.options).map((o) => o.value)).toContain(
        "claude-3-retired"
      )
      expect(el.value).toBe("claude-3-retired")
    })
  })

  it("offers deleting the step only when the caller allows it", async () => {
    const onDelete = vi.fn()
    renderChip({ onChange: vi.fn(), onDelete })
    fireEvent.click(screen.getByRole("button"))
    const del = await screen.findByRole("button", { name: /delete step/i })
    fireEvent.click(del)
    expect(onDelete).toHaveBeenCalled()
  })
})
