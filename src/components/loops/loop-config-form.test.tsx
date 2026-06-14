import { useState } from "react"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  LoopConfigForm,
  type LoopConfigFormState,
  configToFormState,
  formStateToConfig,
} from "./loop-config-form"
import type { AgentOptionsSnapshot, IssueConfig } from "@/lib/types"

// Stable `t` (same instance every render) so any t-dependent effect can't loop.
const t = (key: string, params?: Record<string, unknown>) =>
  params ? `${key} ${JSON.stringify(params)}` : key
vi.mock("next-intl", () => ({ useTranslations: () => t }))

const describeAgentOptions = vi.fn()
vi.mock("@/lib/api", () => ({
  describeAgentOptions: (...a: unknown[]) => describeAgentOptions(...a),
}))

// Render every tab's content (the real Tabs only mounts the active one). The
// same mock flattens the nested agent sub-tabs too, so all sub-tab bodies (the
// default agent, each single stage, the review editor) render at once.
vi.mock("@/components/ui/tabs", () => ({
  Tabs: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  TabsList: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  TabsTrigger: ({ children }: { children: React.ReactNode }) => (
    <button type="button">{children}</button>
  ),
  TabsContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}))
vi.mock("@/components/ui/select", () => ({
  Select: ({
    value,
    onValueChange,
    disabled,
    children,
  }: {
    value: string
    onValueChange: (v: string) => void
    disabled?: boolean
    children: React.ReactNode
  }) => (
    <select
      value={value}
      disabled={disabled}
      onChange={(e) => onValueChange(e.target.value)}
    >
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: React.ReactNode }) => (
    <>{children}</>
  ),
  SelectGroup: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  SelectLabel: () => null,
  SelectItem: ({
    value,
    children,
  }: {
    value: string
    children: React.ReactNode
  }) => <option value={value}>{children}</option>,
}))
vi.mock("@/components/ui/switch", () => ({
  Switch: ({
    id,
    checked,
    onCheckedChange,
  }: {
    id?: string
    checked: boolean
    onCheckedChange: (v: boolean) => void
  }) => (
    <input
      type="checkbox"
      role="switch"
      id={id}
      checked={checked}
      onChange={(e) => onCheckedChange(e.target.checked)}
    />
  ),
}))

// Most agents expose no extra config, so probes stay quiet by default and the
// only "High"/"Reasoning" option in the DOM is the one we explicitly opt into.
const emptySnapshot: AgentOptionsSnapshot = { modes: null, config_options: [] }
const snapshot: AgentOptionsSnapshot = {
  modes: null,
  config_options: [
    {
      id: "reasoning",
      name: "Reasoning",
      description: null,
      category: null,
      kind: {
        type: "select",
        current_value: "low",
        options: [
          { value: "low", name: "Low", description: null },
          { value: "high", name: "High", description: null },
        ],
        groups: [],
      },
    },
  ],
}

function Harness({ initial }: { initial: LoopConfigFormState }) {
  const [v, setV] = useState(initial)
  return (
    <>
      <LoopConfigForm value={v} onChange={setV} />
      <div data-testid="reviewers">{JSON.stringify(v.reviewers)}</div>
      <div data-testid="defaultAgent">{v.defaultSpec.agent}</div>
      <div data-testid="maxAttempts">{v.maxAttempts}</div>
    </>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  describeAgentOptions.mockResolvedValue(emptySnapshot)
})

describe("loop-config-form helpers", () => {
  it("round-trips a config through form state", () => {
    const cfg: IssueConfig = {
      v: 1,
      agents: {
        default: { agent: "codex", config_values: {} },
        design: { agent: "claude_code", config_values: {} },
      },
      validation_commands: ["pnpm test"],
      reviewer_count: 2,
      review_pass_rule: "majority",
      max_attempts: 6,
      auto_merge: true,
      force_route: "full",
      iteration_timeout_secs: 120,
      token_budget_per_turn: 1000,
      stall_alert_secs: 600,
      reviewers: [
        {
          agent: "claude_code",
          mode_id: "plan",
          config_values: { reasoning: "high" },
        },
        { agent: "codex", config_values: {} },
      ],
    }
    expect(formStateToConfig(configToFormState(cfg))).toEqual(cfg)
  })

  it("writes a concrete per-stage spec and omits inherited stages", () => {
    const form = configToFormState({
      v: 1,
      agents: { default: { agent: "claude_code", config_values: {} } },
      validation_commands: [],
      reviewer_count: 1,
      review_pass_rule: "unanimous",
      max_attempts: 6,
      auto_merge: false,
      force_route: null,
      iteration_timeout_secs: null,
      token_budget_per_turn: null,
      stall_alert_secs: null,
      reviewers: [],
    })
    form.stageSpecs.implement = {
      agent: "codex",
      mode_id: "auto",
      config_values: { reasoning: "high" },
    }
    const cfg = formStateToConfig(form)
    expect(cfg.agents.implement).toEqual({
      agent: "codex",
      mode_id: "auto",
      config_values: { reasoning: "high" },
    })
    // A stage left as "use default" is not written into agents.
    expect(cfg.agents.plan).toBeUndefined()
    expect(cfg.agents.default).toEqual({
      agent: "claude_code",
      config_values: {},
    })
  })
})

describe("LoopConfigForm", () => {
  const base = (): LoopConfigFormState =>
    configToFormState({
      v: 1,
      agents: { default: { agent: "claude_code", config_values: {} } },
      validation_commands: [],
      reviewer_count: 1,
      review_pass_rule: "unanimous",
      max_attempts: 6,
      auto_merge: false,
      force_route: null,
      iteration_timeout_secs: null,
      token_budget_per_turn: null,
      stall_alert_secs: null,
      reviewers: [],
    })

  it("edits a numeric field (controlled)", () => {
    render(<Harness initial={base()} />)
    fireEvent.change(screen.getByLabelText("maxAttempts"), {
      target: { value: "9" },
    })
    expect(screen.getByTestId("maxAttempts").textContent).toBe("9")
  })

  it("changes the default agent", () => {
    render(<Harness initial={base()} />)
    // The default-agent picker is the only select currently set to claude_code
    // (single stages are "use default", pass-rule/route carry other sentinels).
    // Switch to gemini (not codex) so the module-scope probe cache the reviewer
    // test relies on for codex stays untouched.
    const selects = screen.getAllByRole("combobox") as HTMLSelectElement[]
    const def = selects.find((s) => s.value === "claude_code")!
    fireEvent.change(def, { target: { value: "gemini" } })
    expect(screen.getByTestId("defaultAgent").textContent).toBe("gemini")
  })

  it("adds and removes reviewer rows", async () => {
    render(<Harness initial={base()} />)
    expect(screen.getByTestId("reviewers").textContent).toBe("[]")

    fireEvent.click(screen.getByText("add"))
    expect(JSON.parse(screen.getByTestId("reviewers").textContent!)).toEqual([
      { agent: "claude_code", mode_id: null, config_values: {} },
    ])

    fireEvent.click(screen.getByLabelText("remove"))
    expect(screen.getByTestId("reviewers").textContent).toBe("[]")
  })

  it("switches a reviewer to use the default agent", () => {
    const initial = configToFormState({
      v: 1,
      agents: { default: { agent: "claude_code", config_values: {} } },
      validation_commands: [],
      reviewer_count: 1,
      review_pass_rule: "unanimous",
      max_attempts: 6,
      auto_merge: false,
      force_route: null,
      iteration_timeout_secs: null,
      token_budget_per_turn: null,
      stall_alert_secs: null,
      // Use gemini (not codex) so probing this reviewer on mount doesn't pollute
      // the module-scope snapshot cache the codex reviewer test below relies on.
      reviewers: [{ agent: "gemini", mode_id: null, config_values: {} }],
    })
    render(<Harness initial={initial} />)
    // The reviewer's agent picker is the only select currently on "gemini"
    // (default is claude_code, stages are "use default"). Switch it to INHERIT.
    const selects = screen.getAllByRole("combobox") as HTMLSelectElement[]
    const reviewerSel = selects.find((s) => s.value === "gemini")!
    fireEvent.change(reviewerSel, { target: { value: "__inherit__" } })
    expect(JSON.parse(screen.getByTestId("reviewers").textContent!)).toEqual([
      { inherit: true },
    ])
  })

  it("writes a reviewer's probed config value", async () => {
    // Only codex exposes the reasoning option, so "High" is unambiguous even
    // with every flattened sub-tab rendering at once.
    describeAgentOptions.mockImplementation((a: string) =>
      Promise.resolve(a === "codex" ? snapshot : emptySnapshot)
    )
    const initial = configToFormState({
      v: 1,
      agents: { default: { agent: "claude_code", config_values: {} } },
      validation_commands: [],
      reviewer_count: 1,
      review_pass_rule: "unanimous",
      max_attempts: 6,
      auto_merge: false,
      force_route: null,
      iteration_timeout_secs: null,
      token_budget_per_turn: null,
      stall_alert_secs: null,
      reviewers: [{ agent: "codex", mode_id: null, config_values: {} }],
    })
    render(<Harness initial={initial} />)

    // The probe resolves and renders the "Reasoning" select.
    const high = await screen.findByRole("option", { name: "High" })
    const select = high.closest("select")!
    fireEvent.change(select, { target: { value: "high" } })

    await waitFor(() =>
      expect(
        JSON.parse(screen.getByTestId("reviewers").textContent!)[0]
          .config_values
      ).toEqual({ reasoning: "high" })
    )
    expect(describeAgentOptions).toHaveBeenCalledWith("codex")
  })
})
