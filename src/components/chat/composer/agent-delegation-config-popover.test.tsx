import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { useState } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type { AgentDelegationDefaults, AgentOptionsSnapshot } from "@/lib/types"

import { AgentDelegationConfigPopover } from "./agent-delegation-config-popover"

// Module-level holder the tests rewrite per case (the vi.mock factory hoists
// above const declarations, so it must only dereference this lazily — inside
// the mocked hook body, not at factory time).
const mockedSnapshot: {
  value: AgentOptionsSnapshot | null
  loading: boolean
  error: string | null
} = { value: null, loading: false, error: null }

vi.mock("@/components/automations/use-agent-options", () => ({
  useAgentOptions: vi.fn(() => ({
    snapshot: mockedSnapshot.value,
    loading: mockedSnapshot.loading,
    error: mockedSnapshot.error,
    reload: () => {},
    ensure: async () => mockedSnapshot.value,
  })),
}))

const snapshot: AgentOptionsSnapshot = {
  modes: null,
  config_options: [
    {
      id: "model",
      name: "Model",
      kind: {
        type: "select",
        current_value: "claude-sonnet-4-5",
        options: [
          { value: "claude-sonnet-4-5", name: "Sonnet 4.5" },
          { value: "claude-opus-4-1", name: "Opus 4.1" },
        ],
        groups: [],
      },
    },
    {
      id: "permission_mode",
      name: "Permission mode",
      kind: {
        type: "select",
        current_value: "default",
        options: [
          { value: "default", name: "Default" },
          { value: "plan", name: "Plan" },
        ],
        groups: [],
      },
    },
  ],
  available_commands: [],
}

const globalDefault: AgentDelegationDefaults = {
  config_values: { model: "claude-opus-4-1" },
}

function renderPopover(overrides?: {
  value?: AgentDelegationDefaults | null
  onChange?: (next: AgentDelegationDefaults | null) => void
  globalDefault?: AgentDelegationDefaults | null
  globalDefaultLoading?: boolean
  globalDefaultError?: string | null
  onRetryGlobalDefault?: () => void
  agentType?: "claude_code" | "cline"
}) {
  const onChange =
    overrides?.onChange ??
    vi.fn<(next: AgentDelegationDefaults | null) => void>()
  const utils = render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AgentDelegationConfigPopover
        agentType={overrides?.agentType ?? "claude_code"}
        workingDir="/repo"
        anchorEl={null}
        globalDefault={
          "globalDefault" in (overrides ?? {})
            ? overrides!.globalDefault!
            : globalDefault
        }
        globalDefaultLoading={overrides?.globalDefaultLoading}
        globalDefaultError={overrides?.globalDefaultError}
        onRetryGlobalDefault={overrides?.onRetryGlobalDefault}
        value={overrides?.value ?? null}
        onChange={onChange}
        onClose={() => {}}
      />
    </NextIntlClientProvider>
  )
  return { onChange, ...utils }
}

/** Drive one Radix Select: click its (labeled) trigger, then the option row.
 *  `AgentConfigSection`'s stacked rows pair a bare `<label>` with the trigger
 *  (no htmlFor), so the trigger is located through its row's label text. */
function triggerForLabel(label: string): HTMLElement {
  const row = screen.getByText(label).closest("div")
  const trigger = row?.querySelector<HTMLElement>('[role="combobox"]')
  if (!trigger) throw new Error(`no combobox for label ${label}`)
  return trigger
}

async function pickOption(
  user: ReturnType<typeof userEvent.setup>,
  triggerLabel: string,
  optionText: string
) {
  await user.click(triggerForLabel(triggerLabel))
  await user.click(await screen.findByRole("option", { name: optionText }))
}

beforeEach(() => {
  mockedSnapshot.value = snapshot
  mockedSnapshot.loading = false
  mockedSnapshot.error = null
})

describe("AgentDelegationConfigPopover", () => {
  it("starts from the global default values when no override is set", () => {
    renderPopover()
    expect(triggerForLabel("Model")).toHaveTextContent("Opus 4.1")
    // Rows the global default does not pin show the "Agent default" sentinel.
    expect(triggerForLabel("Permission mode")).toHaveTextContent(
      "Agent default"
    )
    // Nothing overridden yet → the reset action is inert.
    expect(screen.getByRole("button", { name: /use global/i })).toBeDisabled()
  })

  it("emits the full effective selection when one row changes", async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    renderPopover({ onChange })
    await pickOption(user, "Permission mode", "Plan")
    expect(onChange).toHaveBeenCalledTimes(1)
    // The untouched model row keeps the global pin — a per-call value replaces
    // the global wholesale, so a partial override would silently drop it.
    expect(onChange).toHaveBeenCalledWith({
      config_values: { model: "claude-opus-4-1", permission_mode: "plan" },
    })
  })

  it("seeds from an existing override and lets a row reset drop that field", async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    renderPopover({
      value: {
        mode_id: null,
        config_values: { model: "claude-sonnet-4-5", permission_mode: "plan" },
      },
      onChange,
    })
    expect(triggerForLabel("Model")).toHaveTextContent("Sonnet 4.5")
    await pickOption(user, "Model", "Agent default")
    expect(onChange).toHaveBeenCalledWith({
      config_values: { permission_mode: "plan" },
    })
  })

  it("reset button emits null (back to the global default)", async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    renderPopover({
      value: { config_values: { model: "claude-sonnet-4-5" } },
      onChange,
    })
    await user.click(screen.getByRole("button", { name: /use global/i }))
    expect(onChange).toHaveBeenCalledWith(null)
  })

  it("keeps the popover shell usable while the probe is still loading", () => {
    mockedSnapshot.loading = true
    renderPopover()
    expect(screen.getByText("Loading options…")).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: /use global/i })
    ).toBeInTheDocument()
  })

  it("reports a failed probe instead of inventing options", async () => {
    const { useAgentOptions } =
      await import("@/components/automations/use-agent-options")
    // One-shot override — mockReturnValue would leak into later tests.
    vi.mocked(useAgentOptions).mockReturnValueOnce({
      snapshot: null,
      loading: false,
      error: "agent CLI not installed",
      reload: () => {},
      ensure: async () => null,
    } as never)
    renderPopover()
    expect(screen.getByText("agent CLI not installed")).toBeInTheDocument()
  })
})

/** Local state harness replicating the host's value wiring. */
describe("AgentDelegationConfigPopover host wiring", () => {
  it("stays open across edits so several rows can be tweaked", async () => {
    const user = userEvent.setup()
    function Host() {
      const [value, setValue] = useState<AgentDelegationDefaults | null>(null)
      return (
        <NextIntlClientProvider locale="en" messages={enMessages}>
          <AgentDelegationConfigPopover
            agentType="claude_code"
            workingDir="/repo"
            anchorEl={null}
            globalDefault={globalDefault}
            value={value}
            onChange={setValue}
            onClose={() => {}}
          />
        </NextIntlClientProvider>
      )
    }
    render(<Host />)
    await pickOption(user, "Permission mode", "Plan")
    await pickOption(user, "Model", "Sonnet 4.5")
    expect(triggerForLabel("Model")).toHaveTextContent("Sonnet 4.5")
    expect(triggerForLabel("Permission mode")).toHaveTextContent("Plan")
  })
})

// ── Reviewer regressions ──────────────────────────────────────────────────

describe("AgentDelegationConfigPopover global-baseline race", () => {
  it("holds the option rows back until the global default baseline lands", () => {
    // The baseline load is async; editing against an empty baseline would emit
    // a partial override, and the backend REPLACES the global default with it
    // (no merge) — dropping pins the user never touched. So: rows hidden.
    mockedSnapshot.value = snapshot
    renderPopover({ globalDefaultLoading: true })
    expect(screen.getByText("Loading settings...")).toBeInTheDocument()
    expect(document.querySelector('[role="combobox"]')).toBeNull()

    // Baseline lands → rows become editable.
    renderPopover({ globalDefaultLoading: false })
    expect(triggerForLabel("Model")).toBeTruthy()
  })

  it("keeps option rows disabled when the global baseline fails and allows retry", async () => {
    const onRetryGlobalDefault = vi.fn()
    renderPopover({
      globalDefaultError: "database offline",
      onRetryGlobalDefault,
    })
    expect(
      screen.getByText("Failed to load delegation settings: database offline")
    ).toBeInTheDocument()
    expect(document.querySelector('[role="combobox"]')).toBeNull()

    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }))
    expect(onRetryGlobalDefault).toHaveBeenCalledTimes(1)
  })
})

describe("AgentDelegationConfigPopover boolean config options", () => {
  const clineSnapshot: AgentOptionsSnapshot = {
    modes: null,
    config_options: [
      {
        id: "auto_approve",
        name: "Auto-approve tools",
        kind: { type: "boolean", current_value: false },
      },
    ],
    available_commands: [],
  }

  it('renders a boolean option as a toggle and emits "true" on flip', async () => {
    mockedSnapshot.value = clineSnapshot
    const user = userEvent.setup()
    const onChange = vi.fn()
    renderPopover({ agentType: "cline", onChange })
    const toggle = screen.getByRole("button", {
      name: "Auto-approve tools: Off",
    })
    await user.click(toggle)
    // The global default's model pin survives: the emit is the FULL effective
    // selection, because the backend replaces (not merges) the global default.
    expect(onChange).toHaveBeenCalledWith({
      config_values: { model: "claude-opus-4-1", auto_approve: "true" },
    })
  })

  it("displays the override's pinned value instead of the live current_value", () => {
    mockedSnapshot.value = clineSnapshot
    renderPopover({
      agentType: "cline",
      value: { config_values: { auto_approve: "true" } },
    })
    expect(
      screen.getByRole("button", { name: "Auto-approve tools: On" })
    ).toBeInTheDocument()
  })

  it("pins a flipped boolean even when every select is left on Agent default", async () => {
    // A user who only wants to flip the permission toggle must still get a
    // per-call value — the toggle alone satisfies the non-empty override.
    mockedSnapshot.value = clineSnapshot
    const user = userEvent.setup()
    const onChange = vi.fn()
    renderPopover({ agentType: "cline", onChange })
    await user.click(
      screen.getByRole("button", { name: "Auto-approve tools: Off" })
    )
    expect(onChange).toHaveBeenCalledTimes(1)
    expect(onChange).not.toHaveBeenCalledWith(null)
  })
})
