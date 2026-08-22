import { beforeEach, describe, expect, it, vi } from "vitest"
import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import enMessages from "@/i18n/messages/en.json"
import { usePkArenaStore } from "@/stores/pk-arena-store"
import { PkLauncherDialog } from "./pk-launcher-dialog"

const apiMocks = vi.hoisted(() => ({
  acpGetAgentStatus: vi.fn(),
  getGitBranch: vi.fn(),
  pkRoundCreate: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  acpGetAgentStatus: apiMocks.acpGetAgentStatus,
  getFolder: vi.fn(),
  getGitBranch: apiMocks.getGitBranch,
  gitInit: vi.fn(),
  gitLog: vi.fn(),
  pkRoundCreate: apiMocks.pkRoundCreate,
  pkRoundUpdateStatus: vi.fn(),
  pkRoundDelete: vi.fn(),
  pkRoundUpdateJudge: vi.fn(),
}))

vi.mock("@/hooks/use-acp-agents", () => ({
  useAcpAgents: () => ({
    agents: [
      {
        agent_type: "claude_code",
        name: "Claude Code",
        enabled: true,
        available: true,
        installed_version: "1.0.0",
      },
    ],
  }),
}))

vi.mock("@/components/automations/use-agent-options", () => ({
  useAgentOptions: () => ({
    snapshot: {
      modes: null,
      config_options: [
        {
          id: "model",
          name: "Model",
          category: "model",
          kind: {
            type: "select",
            current_value: "sonnet",
            options: [
              { value: "sonnet", name: "Sonnet" },
              { value: "opus", name: "Opus" },
            ],
            groups: [],
          },
        },
      ],
      available_commands: [],
    },
    loading: false,
    error: null,
    reload: vi.fn(),
    ensure: vi.fn(),
  }),
}))

vi.mock("@/stores/tab-store", () => ({
  useTabStore: (selector: (state: unknown) => unknown) =>
    selector({
      activeTabId: "tab-1",
      openPkRoundTab: vi.fn(),
      tabs: [
        {
          id: "tab-1",
          kind: "conversation",
          folderId: 7,
          conversationId: null,
          agentType: "claude-code",
          title: "New conversation",
          isPinned: true,
          workingDir: "/tmp/repo",
        },
      ],
    }),
}))

describe("PkLauncherDialog", () => {
  beforeEach(() => {
    window.localStorage.clear()
    apiMocks.getGitBranch.mockResolvedValue("main")
    apiMocks.acpGetAgentStatus.mockResolvedValue({
      enabled: true,
      available: true,
      installed_version: "1.0.0",
    })
    apiMocks.pkRoundCreate.mockResolvedValue({
      id: 12,
      folder_id: 7,
      task: "compare models",
      config: {
        agents: [],
        permission_mode: "default",
        bare_mode: false,
        effort: "default",
      },
      status: "ready",
      failure_reason: null,
      judge_status: "idle",
      created_at: "2026-08-20T00:00:00Z",
      updated_at: "2026-08-20T00:00:00Z",
      finished_at: null,
    })
    usePkArenaStore.setState({
      rounds: [],
      activeRoundId: null,
      launcherOpen: true,
      pillDismissed: false,
      hydrating: false,
    })
  })

  it("pins the model selected inside each contestant slot", async () => {
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <PkLauncherDialog />
      </NextIntlClientProvider>
    )

    const addContestant = await screen.findByRole("button", {
      name: "Add Claude Code as contestant",
    })
    await user.click(addContestant)
    await user.click(addContestant)

    const modelSelectors = await screen.findAllByRole("combobox", {
      name: "Model",
    })
    expect(modelSelectors).toHaveLength(2)

    await user.click(modelSelectors[1])
    await user.click(await screen.findByRole("option", { name: "Opus" }))
    await user.type(screen.getByLabelText("Task"), "compare models")

    const start = screen.getByRole("button", { name: "Start match" })
    await waitFor(() => expect(start).toBeEnabled())
    await user.click(start)

    await waitFor(() => expect(apiMocks.pkRoundCreate).toHaveBeenCalled())
    expect(apiMocks.pkRoundCreate).toHaveBeenCalledWith(
      7,
      "compare models",
      expect.objectContaining({
        agents: [
          {
            agent: "claude_code",
            label: "Sonnet",
            config_values: { model: "sonnet" },
          },
          {
            agent: "claude_code",
            label: "Opus",
            config_values: { model: "opus" },
          },
        ],
      })
    )
  })
})
