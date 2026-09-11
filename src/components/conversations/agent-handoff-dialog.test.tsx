import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type { HandoffPlan, HandoffResult } from "@/lib/api"
import { getAgentLabel } from "@/lib/custom-agents"
import { TurnBusyError } from "@/lib/turn-busy"
import type { AcpAgentInfo, AgentType } from "@/lib/types"

const h = vi.hoisted(() => ({
  plan: vi.fn(),
  handoff: vi.fn(),
  openTab: vi.fn(),
  closeConversationTab: vi.fn(),
  refreshConversations: vi.fn(async () => {}),
  setExternalId: vi.fn(),
  refetchDetail: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
  prefs: vi.fn(() => ({ modeId: "code", configValues: { model: "gpt-x" } })),
  agents: [] as AcpAgentInfo[],
}))

// Inline SVG marks carry a <title> that would duplicate the agent label.
vi.mock("@/components/agent-icon", () => ({ AgentIcon: () => null }))
vi.mock("sonner", () => ({
  toast: { success: h.toastSuccess, error: h.toastError },
}))
vi.mock("@/lib/api", () => ({
  acpHandoffPlan: h.plan,
  acpHandoff: h.handoff,
}))
vi.mock("@/hooks/use-acp-agents", () => ({
  useAcpAgents: () => ({ agents: h.agents, fresh: true, refresh: vi.fn() }),
}))
vi.mock("@/contexts/tab-context", () => ({
  useTabActions: () => ({
    openTab: h.openTab,
    closeConversationTab: h.closeConversationTab,
  }),
}))
vi.mock("@/stores/app-workspace-store", () => ({
  useAppWorkspaceStore: (selector: (s: unknown) => unknown) =>
    selector({ refreshConversations: h.refreshConversations }),
}))
vi.mock("@/stores/conversation-runtime-store", () => ({
  useConversationRuntimeActions: () => ({
    setExternalId: h.setExternalId,
    refetchDetail: h.refetchDetail,
  }),
}))
vi.mock("@/lib/selector-prefs-storage", () => ({
  getSavedPrefsForConnect: h.prefs,
}))

import { AgentHandoffDialog } from "./agent-handoff-dialog"

function agent(agentType: AgentType): AcpAgentInfo {
  return {
    agent_type: agentType,
    skills_capable: true,
    registry_id: `${agentType}-registry`,
    registry_version: null,
    supports_custom_version: false,
    name: agentType,
    description: "",
    available: true,
    distribution_type: "npx",
    is_acp_adapter: true,
    custom_source: null,
    enabled: true,
    sort_order: 0,
    installed_version: "1.0.0",
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
  }
}

function plan(overrides: Partial<HandoffPlan> = {}): HandoffPlan {
  return {
    sourceAgentType: "claude_code",
    targetAgentType: "codex",
    path: "summary",
    turnCount: 12,
    briefingChars: 4_000,
    briefingTruncated: false,
    verbatimTurns: 6,
    ...overrides,
  }
}

function result(overrides: Partial<HandoffResult> = {}): HandoffResult {
  return {
    conversationId: 7,
    folderId: 3,
    fromAgentType: "claude_code",
    toAgentType: "codex",
    externalId: "S2",
    path: "summary",
    connectionId: "conn-9",
    briefingTruncated: false,
    ...overrides,
  }
}

function renderDialog(onOpenChange = vi.fn()) {
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AgentHandoffDialog
        open
        onOpenChange={onOpenChange}
        conversationId={7}
        folderId={3}
        sourceAgentType="claude_code"
        title="Retry loop"
      />
    </NextIntlClientProvider>
  )
  return { onOpenChange }
}

/** Pick an agent through the selector's pills (labelled by display name). */
function pick(agentType: AgentType) {
  const label = getAgentLabel(agentType)
  const pill = screen
    .getAllByRole("button")
    .find(
      (b) =>
        b.getAttribute("data-slot") === "agent-pill" &&
        b.textContent?.includes(label)
    )
  if (!pill) throw new Error(`no pill for ${agentType}`)
  fireEvent.click(pill)
}

describe("AgentHandoffDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    h.agents = [agent("claude_code"), agent("codex"), agent("grok")]
    h.plan.mockResolvedValue(plan())
    h.handoff.mockResolvedValue(result())
  })

  it("explains the summary path for the chosen target and hands off with the note", async () => {
    const { onOpenChange } = renderDialog()
    pick("codex")
    await waitFor(() => expect(h.plan).toHaveBeenCalledWith(7, "codex"))
    expect(
      await screen.findByText(
        "Codex starts a new session seeded with a briefing: 12 turns summarized, the last 6 in full."
      )
    ).toBeInTheDocument()

    fireEvent.change(screen.getByLabelText(/focus on/), {
      target: { value: "  finish the tests  " },
    })
    const confirm = screen.getByRole("button", { name: "Hand off" })
    expect(confirm).toBeEnabled()
    fireEvent.click(confirm)

    await waitFor(() => expect(h.handoff).toHaveBeenCalledTimes(1))
    // The note is trimmed and the TARGET's own saved prefs travel, never the
    // source agent's model.
    expect(h.prefs).toHaveBeenCalledWith("codex")
    expect(h.handoff).toHaveBeenCalledWith(
      7,
      "codex",
      "finish the tests",
      "code",
      {
        model: "gpt-x",
      }
    )
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false))
    // Runtime re-pointed at the new session, sidebar refreshed, tab swapped:
    // the new tab opens BEFORE the old one closes.
    expect(h.setExternalId).toHaveBeenCalledWith(7, "S2")
    expect(h.refreshConversations).toHaveBeenCalled()
    expect(h.openTab).toHaveBeenCalledWith(3, 7, "codex", false, "Retry loop")
    expect(h.closeConversationTab).toHaveBeenCalledWith(3, 7, "claude_code")
    expect(h.openTab.mock.invocationCallOrder[0]).toBeLessThan(
      h.closeConversationTab.mock.invocationCallOrder[0]
    )
    expect(h.refetchDetail).toHaveBeenCalledWith(7)
    expect(h.toastSuccess).toHaveBeenCalledWith("Handed off to Codex")
  })

  it("states a native transfer plainly", async () => {
    h.plan.mockResolvedValue(
      plan({ targetAgentType: "grok", path: "native", verbatimTurns: 0 })
    )
    renderDialog()
    pick("grok")
    expect(
      await screen.findByText(
        "Grok will load the full session natively. Nothing is summarized."
      )
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Hand off" })).toBeEnabled()
  })

  it("says when the briefing was shortened and when native was demoted", async () => {
    h.plan.mockResolvedValue(
      plan({ briefingTruncated: true, nativeReason: "transcript_missing" })
    )
    renderDialog()
    pick("codex")
    const statement = await screen.findByText(
      /Codex starts a new session seeded with a briefing\. The conversation is long/
    )
    expect(statement.textContent).toContain(
      "The original transcript is no longer in Claude Code's store"
    )
  })

  it("refuses the current agent and a blocked target without calling the backend", async () => {
    renderDialog()
    pick("claude_code")
    expect(
      await screen.findByText("This conversation already runs on Claude Code.")
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Hand off" })).toBeDisabled()
    expect(h.plan).not.toHaveBeenCalledWith(7, "claude_code")

    h.plan.mockResolvedValue(
      plan({ targetAgentType: "grok", blocked: "not_installed" })
    )
    pick("grok")
    expect(
      await screen.findByText(
        "Grok is not installed. Install it in Agent Settings first."
      )
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Hand off" })).toBeDisabled()
    expect(h.handoff).not.toHaveBeenCalled()
  })

  it("keeps the tab where it was when the backend fails, and names a busy turn", async () => {
    h.handoff.mockRejectedValueOnce(new Error("codex did not open a session"))
    const { onOpenChange } = renderDialog()
    pick("codex")
    await screen.findByText(/starts a new session seeded/)
    fireEvent.click(screen.getByRole("button", { name: "Hand off" }))
    expect(
      await screen.findByText("Handoff failed: codex did not open a session")
    ).toBeInTheDocument()
    expect(h.openTab).not.toHaveBeenCalled()
    expect(h.closeConversationTab).not.toHaveBeenCalled()
    expect(h.setExternalId).not.toHaveBeenCalled()
    expect(onOpenChange).not.toHaveBeenCalledWith(false)
    // The dialog is usable again: a second try goes out.
    h.handoff.mockRejectedValueOnce(new TurnBusyError())
    fireEvent.click(screen.getByRole("button", { name: "Hand off" }))
    expect(
      await screen.findByText(
        "A turn is still running. Stop it or wait for it to finish, then hand off."
      )
    ).toBeInTheDocument()
    expect(h.handoff).toHaveBeenCalledTimes(2)
  })

  it("ignores a plan that arrives for an agent the user already moved away from", async () => {
    let resolveCodex: (p: HandoffPlan) => void = () => {}
    h.plan.mockImplementation((_id: number, target: AgentType) => {
      if (target === "codex") {
        return new Promise<HandoffPlan>((resolve) => {
          resolveCodex = resolve
        })
      }
      return Promise.resolve(
        plan({ targetAgentType: "grok", path: "native", verbatimTurns: 0 })
      )
    })
    renderDialog()
    pick("codex")
    pick("grok")
    expect(
      await screen.findByText(
        "Grok will load the full session natively. Nothing is summarized."
      )
    ).toBeInTheDocument()
    resolveCodex(plan({ briefingTruncated: true }))
    await new Promise((r) => setTimeout(r, 0))
    expect(
      screen.queryByText(/The conversation is long/)
    ).not.toBeInTheDocument()
    expect(
      screen.getByText(
        "Grok will load the full session natively. Nothing is summarized."
      )
    ).toBeInTheDocument()
  })
})
