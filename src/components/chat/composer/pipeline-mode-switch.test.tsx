import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { PipelineGraph, Pipeline } from "@/lib/types"

// Mock pipelinePresets to return duet and team presets
vi.mock("@/lib/api", () => ({
  pipelinePresets: vi.fn(async () => [
    {
      id: 1,
      // The backend returns the display label here, not the key.
      name: "Duet",
      preset_key: "duet",
      folder_id: null,
      graph: {
        steps: [
          {
            id: "coder",
            role: "coder",
            label: "Coder",
            agent_type: "claude",
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
            agent_type: "claude",
            config_values: {},
            prompt_template: "$review",
            timeout_secs: 1800,
            read_memory: false,
            read_only: true,
          },
        ],
        loops: [{ from_step: "reviewer", to_step: "coder", max_iterations: 3 }],
      },
      isolation: "worktree_per_run" as const,
      created_at: "",
      updated_at: "",
    } as Pipeline,
    {
      id: 2,
      name: "Team",
      preset_key: "team",
      folder_id: null,
      graph: {
        steps: [
          {
            id: "planner",
            role: "planner",
            label: "Planner",
            agent_type: "claude",
            config_values: {},
            prompt_template: "$task",
            timeout_secs: 1800,
            read_memory: true,
            read_only: false,
          },
          {
            id: "coder",
            role: "coder",
            label: "Coder",
            agent_type: "claude",
            config_values: {},
            prompt_template: "$plan",
            timeout_secs: 1800,
            read_memory: false,
            read_only: false,
          },
          {
            id: "reviewer",
            role: "reviewer",
            label: "Reviewer",
            agent_type: "claude",
            config_values: {},
            prompt_template: "$review",
            timeout_secs: 1800,
            read_memory: false,
            read_only: true,
          },
          {
            id: "tests",
            role: "tests",
            label: "Tests",
            agent_type: "claude",
            config_values: {},
            prompt_template: "$summary",
            timeout_secs: 1800,
            read_memory: false,
            read_only: true,
          },
        ],
        loops: [
          { from_step: "reviewer", to_step: "coder", max_iterations: 3 },
          { from_step: "tests", to_step: "coder", max_iterations: 3 },
        ],
      },
      isolation: "worktree_per_run" as const,
      created_at: "",
      updated_at: "",
    } as Pipeline,
  ]),
}))

// Mock translations for the Pipeline namespace
const translations: Record<string, string> = {
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
  modelUnconfirmed: "Model not confirmed",
  inspectorTitle: "Step settings",
}

vi.mock("next-intl", () => ({
  useTranslations: (namespace?: string) => {
    return (key: string, params?: Record<string, unknown>) => {
      let str =
        namespace === "Pipeline"
          ? (translations[key] ?? key)
          : (translations[key] ?? key)
      if (params) {
        for (const [k, v] of Object.entries(params)) {
          str = str.replace(`{${k}}`, String(v))
        }
      }
      return str
    }
  },
}))

import { PipelineModeSwitch } from "./pipeline-mode-switch"

describe("PipelineModeSwitch", () => {
  beforeEach(() => {
    localStorage.clear()
    vi.restoreAllMocks()
  })

  it("renders all four mode segments with accessible aria-labels", () => {
    render(<PipelineModeSwitch />)

    expect(
      screen.getByRole("radio", { name: "Single agent" })
    ).toBeInTheDocument()
    expect(screen.getByRole("radio", { name: "Duet" })).toBeInTheDocument()
    expect(screen.getByRole("radio", { name: "Team" })).toBeInTheDocument()
    expect(screen.getByRole("radio", { name: "Custom" })).toBeInTheDocument()
  })

  it("defaults to single mode and does not render role chips", () => {
    render(<PipelineModeSwitch />)

    const singleRadio = screen.getByRole("radio", { name: "Single agent" })
    expect(singleRadio).toHaveAttribute("aria-checked", "true")
    expect(singleRadio).toHaveAttribute("data-state", "active")
    expect(screen.queryByTestId("pipeline-mode-chips")).not.toBeInTheDocument()
  })

  it("loads stored mode from localStorage scoped to folderId", async () => {
    localStorage.setItem("codeg.pipeline.mode.42", "duet")
    render(<PipelineModeSwitch folderId={42} />)

    const duetRadio = screen.getByRole("radio", { name: "Duet" })
    expect(duetRadio).toHaveAttribute("aria-checked", "true")
    await waitFor(() => {
      expect(screen.getByTestId("pipeline-mode-chips")).toBeInTheDocument()
    })
  })

  it("switches to duet mode and displays role chips with model unconfirmed indicator", async () => {
    const onModeChange = vi.fn()
    render(<PipelineModeSwitch folderId={10} onModeChange={onModeChange} />)

    const duetRadio = screen.getByRole("radio", { name: "Duet" })
    fireEvent.click(duetRadio)

    expect(onModeChange).toHaveBeenCalledWith("duet")
    expect(localStorage.getItem("codeg.pipeline.mode.10")).toBe("duet")
    expect(duetRadio).toHaveAttribute("aria-checked", "true")

    // Role chips for Coder and Reviewer
    await waitFor(() => {
      expect(screen.getByTestId("pipeline-step-chip-coder")).toHaveTextContent(
        "Coder"
      )
    })
    expect(screen.getByTestId("pipeline-step-chip-reviewer")).toHaveTextContent(
      "Reviewer"
    )

    // Both chips should show "Model not confirmed" since models are not configured
    const unconfirmedBadges = screen.getAllByText("Model not confirmed")
    expect(unconfirmedBadges.length).toBeGreaterThanOrEqual(2)

    // Loop limit indicator
    expect(screen.getByTestId("pipeline-loop-limit-chip")).toHaveTextContent(
      "Fix rounds: up to 3"
    )
  })

  it("switches to team mode and displays planner, coder, reviewer, and tests chips", async () => {
    const onModeChange = vi.fn()
    render(<PipelineModeSwitch onModeChange={onModeChange} />)

    const teamRadio = screen.getByRole("radio", { name: "Team" })
    fireEvent.click(teamRadio)

    expect(onModeChange).toHaveBeenCalledWith("team")
    await waitFor(() => {
      expect(
        screen.getByTestId("pipeline-step-chip-planner")
      ).toHaveTextContent("Planner")
    })
    expect(screen.getByTestId("pipeline-step-chip-coder")).toHaveTextContent(
      "Coder"
    )
    expect(screen.getByTestId("pipeline-step-chip-reviewer")).toHaveTextContent(
      "Reviewer"
    )
    expect(screen.getByTestId("pipeline-step-chip-tests")).toHaveTextContent(
      "Tests"
    )
  })

  it("renders specific models when provided in graph steps with custom mode", () => {
    const customGraph: PipelineGraph = {
      steps: [
        {
          id: "step-1",
          role: "coder",
          label: "Coder",
          agent_type: "claude_code",
          config_values: { model: "claude-3-7-sonnet" },
          prompt_template: "$task",
          timeout_secs: 1800,
          read_memory: false,
          read_only: false,
        },
        {
          id: "step-2",
          role: "reviewer",
          label: "Reviewer",
          agent_type: "codex",
          config_values: { model: "o3-mini" },
          prompt_template: "$review",
          timeout_secs: 1800,
          read_memory: false,
          read_only: true,
        },
      ],
      loops: [{ from_step: "step-2", to_step: "step-1", max_iterations: 5 }],
    }

    render(<PipelineModeSwitch mode="custom" graph={customGraph} />)

    expect(screen.getByText("claude-3-7-sonnet")).toBeInTheDocument()
    expect(screen.getByText("o3-mini")).toBeInTheDocument()
    expect(screen.getByTestId("pipeline-loop-limit-chip")).toHaveTextContent(
      "Fix rounds: up to 5"
    )
  })

  it("renders agent defaults when provided in props", async () => {
    const agentDefaults = {
      coder: { agentType: "claude_code", model: "claude-3-5-sonnet" },
      reviewer: { agentType: "codex", model: "gpt-4o" },
    }

    render(<PipelineModeSwitch mode="duet" agentDefaults={agentDefaults} />)

    await waitFor(() => {
      expect(screen.getByText("claude-3-5-sonnet")).toBeInTheDocument()
    })
    expect(screen.getByText("gpt-4o")).toBeInTheDocument()
  })

  it("renders custom configure button when onConfigureCustom is provided and custom mode is active", () => {
    const onConfigureCustom = vi.fn()
    render(
      <PipelineModeSwitch mode="custom" onConfigureCustom={onConfigureCustom} />
    )

    const configBtn = screen.getByRole("button", { name: "Step settings" })
    expect(configBtn).toBeInTheDocument()
    fireEvent.click(configBtn)
    expect(onConfigureCustom).toHaveBeenCalledTimes(1)
  })

  it("respects the disabled prop and prevents changing selection", () => {
    const onModeChange = vi.fn()
    render(<PipelineModeSwitch disabled onModeChange={onModeChange} />)

    const duetRadio = screen.getByRole("radio", { name: "Duet" })
    expect(duetRadio).toBeDisabled()
    fireEvent.click(duetRadio)

    expect(onModeChange).not.toHaveBeenCalled()
  })

  it("hides chips when showChips is false", async () => {
    render(<PipelineModeSwitch mode="team" showChips={false} />)
    // Wait for presets to load, then verify chips are not shown even though mode is team
    await waitFor(() => {
      expect(
        screen.queryByTestId("pipeline-mode-chips")
      ).not.toBeInTheDocument()
    })
  })
})
