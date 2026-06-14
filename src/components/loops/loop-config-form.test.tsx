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

// Render every tab's content (the real Tabs only mounts the active one).
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
      <div data-testid="maxAttempts">{v.maxAttempts}</div>
    </>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  describeAgentOptions.mockResolvedValue(snapshot)
})

describe("loop-config-form helpers", () => {
  it("round-trips a config through form state", () => {
    const cfg: IssueConfig = {
      v: 1,
      agents: { default: "codex", design: "claude_code" },
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
})

describe("LoopConfigForm", () => {
  const base = (): LoopConfigFormState =>
    configToFormState({
      v: 1,
      agents: { default: "claude_code" },
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

  it("writes a reviewer's probed config value", async () => {
    const initial = configToFormState({
      v: 1,
      agents: { default: "claude_code" },
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
