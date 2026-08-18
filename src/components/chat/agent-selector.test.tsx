import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest"

// MUST mock the hook BEFORE importing the component under test.
vi.mock("@/hooks/use-acp-agents", () => ({
  useAcpAgents: vi.fn(),
}))

import { AgentSelector } from "./agent-selector"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import enMessages from "@/i18n/messages/en.json"
import { getAgentLabel } from "@/lib/custom-agents"
import type { AcpAgentInfo, AgentType } from "@/lib/types"

const mockUseAcpAgents = vi.mocked(useAcpAgents)

function agent(
  agentType: AgentType,
  overrides: Partial<AcpAgentInfo> = {}
): AcpAgentInfo {
  return {
    agent_type: agentType,
    skills_capable: true,
    registry_id: `${agentType}-registry`,
    registry_version: null,
    name: agentType,
    description: "",
    available: true,
    distribution_type: "system",
    is_acp_adapter: false,
    custom_source: null,
    enabled: true,
    sort_order: 0,
    installed_version: null,
    host_tools_agent_mode: false,
    env: {},
    config_json: null,
    config_file_path: null,
    opencode_auth_json: null,
    codex_auth_json: null,
    codex_config_toml: null,
    codex_model_catalog: null,
    codex_sandbox_settings: null,
    grok_config_toml: null,
    grok_settings: null,
    cline_secrets_json: null,
    hermes_config_yaml: null,
    cursor_cli_config_json: null,
    cursor_settings: null,
    model_provider_id: null,
    icon_url: null,
    ...overrides,
  }
}

function renderWithIntl(ui: React.ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

beforeAll(() => {
  // jsdom doesn't implement ResizeObserver; the component constructs one in
  // a useEffect to drive the sliding-indicator animation. Stub before any
  // test renders so render() doesn't throw.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  ;(globalThis as any).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
})

/**
 * jsdom has no layout engine, so the overflow math would otherwise see zeros
 * everywhere. Feed it widths keyed off `data-slot`, which is exactly what the
 * component measures: the wrapper (how much room there is), an icon-only pill
 * (the unit every fit is counted in) and the "more" button.
 */
function stubLayout({
  wrapper = 300,
  pill = 38,
  more = 32,
}: { wrapper?: number; pill?: number; more?: number } = {}) {
  return vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function (this: HTMLElement) {
      const slot = this.dataset.slot
      const width =
        slot === "agent-selector"
          ? wrapper
          : slot === "agent-pill"
            ? pill
            : slot === "agent-selector-more"
              ? more
              : 0
      return {
        width,
        height: 32,
        top: 0,
        left: 0,
        right: width,
        bottom: 32,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect
    })
}

const MANY_AGENTS: AgentType[] = [
  "claude_code",
  "codex",
  "open_code",
  "gemini",
  "open_claw",
  "cline",
  "hermes",
  "code_buddy",
  "kimi_code",
  "pi",
  "grok",
  "cursor",
  "deepseek",
]

function pills(container: HTMLElement) {
  return Array.from(container.querySelectorAll('[data-slot="agent-pill"]'))
}

beforeEach(() => {
  mockUseAcpAgents.mockReset()
  vi.restoreAllMocks()
})

describe("AgentSelector", () => {
  it("shows the empty state when no enabled agents are returned", () => {
    mockUseAcpAgents.mockReturnValue({
      agents: [],
      fresh: true,
      refresh: async () => {},
    })
    const onOpenSettings = vi.fn()
    renderWithIntl(
      <AgentSelector
        onSelect={() => {}}
        onOpenAgentsSettings={onOpenSettings}
      />
    )
    expect(screen.getByText("No enabled agents")).toBeInTheDocument()
    fireEvent.click(
      screen.getByRole("button", { name: "Open Agents settings" })
    )
    expect(onOpenSettings).toHaveBeenCalledTimes(1)
  })

  it("clicking an available agent invokes onSelect with its agent_type", () => {
    mockUseAcpAgents.mockReturnValue({
      agents: [agent("claude_code"), agent("codex")],
      fresh: true,
      refresh: async () => {},
    })
    const onSelect = vi.fn()
    renderWithIntl(
      <AgentSelector
        defaultAgentType="claude_code"
        onSelect={onSelect}
        onFallback={vi.fn()}
      />
    )
    // The agent_type's display label comes from AGENT_LABELS; clicking the
    // codex button by role/title is the most stable selector.
    const buttons = screen.getAllByRole("button")
    // First button is the selected (claude_code), second is codex.
    fireEvent.click(buttons[1])
    expect(onSelect).toHaveBeenCalledTimes(1)
    expect(onSelect).toHaveBeenCalledWith("codex")
  })

  it("fires onFallback (not onSelect) when the preferred agent is unavailable", async () => {
    // Regression guard: this is the fix path for stale-default resolution.
    // If onFallback is supplied it must receive the substitute; onSelect
    // must NOT fire, because the caller treats onSelect as a confirmed user
    // choice (which would mask a downstream correction effect).
    mockUseAcpAgents.mockReturnValue({
      agents: [
        agent("claude_code", { available: false }),
        agent("codex"),
        agent("gemini"),
      ],
      fresh: true,
      refresh: async () => {},
    })
    const onSelect = vi.fn()
    const onFallback = vi.fn()
    renderWithIntl(
      <AgentSelector
        defaultAgentType="claude_code"
        onSelect={onSelect}
        onFallback={onFallback}
      />
    )
    await waitFor(() => {
      expect(onFallback).toHaveBeenCalledTimes(1)
    })
    expect(onFallback).toHaveBeenCalledWith("codex")
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("falls back through onSelect when onFallback is not provided (legacy)", async () => {
    // Without onFallback, the auto-pick must still surface as onSelect for
    // backwards compatibility with older call sites.
    mockUseAcpAgents.mockReturnValue({
      agents: [agent("codex"), agent("gemini")],
      fresh: true,
      refresh: async () => {},
    })
    const onSelect = vi.fn()
    renderWithIntl(
      <AgentSelector defaultAgentType={undefined} onSelect={onSelect} />
    )
    await waitFor(() => {
      expect(onSelect).toHaveBeenCalled()
    })
    expect(onSelect).toHaveBeenCalledWith("codex")
  })

  it("keeps the icon wrapper a flex box so the mark stays vertically centered", () => {
    // jsdom has no layout engine, so this guards the cause rather than the
    // symptom: as a bare inline box the wrapper lays the icon out on a text
    // baseline and grows by the strut's descender (20px around a 16px mark),
    // which pushes every icon ~2px above the pill's centerline while the
    // label sits dead center. Measured in a real engine; assert the class
    // here so a future markup cleanup can't silently drop it.
    mockUseAcpAgents.mockReturnValue({
      agents: [agent("codex")],
      fresh: true,
      refresh: async () => {},
    })
    renderWithIntl(
      <AgentSelector defaultAgentType="codex" onSelect={() => {}} />
    )
    const wrapper = screen.getByRole("button").firstElementChild
    expect(wrapper).toHaveClass("inline-flex")
  })

  it("keeps every agent in the row while nothing can be measured", () => {
    // No layout stub: every box reports 0, which is also what the very first
    // client render sees. Guessing a cut from that would hide agents that had
    // room all along, so the row must stay whole.
    mockUseAcpAgents.mockReturnValue({
      agents: MANY_AGENTS.map((type) => agent(type)),
      fresh: true,
      refresh: async () => {},
    })
    const { container } = renderWithIntl(
      <AgentSelector defaultAgentType="claude_code" onSelect={() => {}} />
    )
    expect(pills(container)).toHaveLength(MANY_AGENTS.length)
    expect(screen.queryByLabelText(/More agents/)).not.toBeInTheDocument()
  })

  it("collapses the agents that don't fit into the more popover", () => {
    // 300px of room, 38px per icon-only pill, 46px for the selected one
    // (38 + the 8px its wider padding adds; the label measures 0 in jsdom) and
    // 32px for the ⋯ button leaves room for 5 of the 12 unselected agents.
    stubLayout()
    mockUseAcpAgents.mockReturnValue({
      agents: MANY_AGENTS.map((type) => agent(type)),
      fresh: true,
      refresh: async () => {},
    })
    const { container } = renderWithIntl(
      <AgentSelector defaultAgentType="claude_code" onSelect={() => {}} />
    )
    expect(pills(container)).toHaveLength(6)
    expect(screen.getByLabelText("More agents (7)")).toBeInTheDocument()
  })

  it("keeps the selected agent in the row even when it sits past the cut", () => {
    // Regression guard for the whole point of the overflow design: an agent
    // picked out of the popover must not vanish into it. It lands at the end
    // of the row instead, still wearing the droplet.
    stubLayout()
    mockUseAcpAgents.mockReturnValue({
      agents: MANY_AGENTS.map((type) => agent(type)),
      fresh: true,
      refresh: async () => {},
    })
    const { container } = renderWithIntl(
      <AgentSelector defaultAgentType="cursor" onSelect={() => {}} />
    )
    const row = pills(container)
    expect(row).toHaveLength(6)
    const selected = row[row.length - 1]
    expect(selected).toHaveAttribute("aria-pressed", "true")
    expect(selected).toHaveTextContent(getAgentLabel("cursor"))
  })

  it("selects a collapsed agent from the more popover", async () => {
    stubLayout()
    mockUseAcpAgents.mockReturnValue({
      agents: MANY_AGENTS.map((type) => agent(type)),
      fresh: true,
      refresh: async () => {},
    })
    const onSelect = vi.fn()
    const { container } = renderWithIntl(
      <AgentSelector defaultAgentType="claude_code" onSelect={onSelect} />
    )
    // `deepseek` is last in the list, so it is certainly one of the collapsed
    // ones — it has no pill in the row to click.
    const label = getAgentLabel("deepseek")
    expect(
      pills(container).some((pill) => pill.textContent?.includes(label))
    ).toBe(false)

    fireEvent.click(screen.getByLabelText("More agents (7)"))
    // Queried by slot, not by accessible name: each AgentIcon carries an SVG
    // <title> with the same agent name, so a role+name lookup sees it twice.
    await waitFor(() => {
      expect(
        document.querySelectorAll('[data-slot="agent-option"]').length
      ).toBe(7)
    })
    const option = Array.from(
      document.querySelectorAll('[data-slot="agent-option"]')
    ).find((el) => el.textContent?.includes(label))
    expect(option).toBeDefined()
    fireEvent.click(option as Element)
    expect(onSelect).toHaveBeenCalledWith("deepseek")
  })

  it("notifies onAgentsLoaded with the current agent list", async () => {
    const codex = agent("codex")
    mockUseAcpAgents.mockReturnValue({
      agents: [codex],
      fresh: true,
      refresh: async () => {},
    })
    const onAgentsLoaded = vi.fn()
    renderWithIntl(
      <AgentSelector
        defaultAgentType="codex"
        onSelect={() => {}}
        onAgentsLoaded={onAgentsLoaded}
      />
    )
    await waitFor(() => {
      expect(onAgentsLoaded).toHaveBeenCalled()
    })
    const calls = onAgentsLoaded.mock.calls
    const lastCall = calls[calls.length - 1]?.[0]
    expect(lastCall).toEqual([codex])
  })
})
