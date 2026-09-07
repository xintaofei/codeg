import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type {
  AcpAgentInfo,
  AgentSkillItem,
  AgentSkillsListResult,
} from "@/lib/types"

const api = vi.hoisted(() => ({
  acpDeleteAgentSkill: vi.fn(),
  acpListAgents: vi.fn(),
  acpListAgentSkills: vi.fn(),
  acpReadAgentSkill: vi.fn(),
  acpSaveAgentSkill: vi.fn(),
  acpSetAgentSkillEnabled: vi.fn(),
  loadFolderHistory: vi.fn(),
  openFolder: vi.fn(),
}))

const toast = vi.hoisted(() => ({
  error: vi.fn(),
  success: vi.fn(),
}))

vi.mock("@/lib/api", () => api)
vi.mock("sonner", () => ({ toast }))

import { SkillsSettings } from "./skills-settings"

const messages = {
  ...enMessages,
  SkillsSettings: {
    ...enMessages.SkillsSettings,
    availability: {
      enabled: "Enabled",
      disabled: "Disabled",
      toggleAria: "Toggle {skill} for {agent}",
      readOnly: "Built-in skills are always available.",
      cannotIsolate: "This shared skill cannot be toggled independently.",
    },
    toasts: {
      ...enMessages.SkillsSettings.toasts,
      enabled: "Skill enabled",
      disabled: "Skill disabled",
      toggleFailed: "Failed to update skill availability",
    },
  },
}

const agent = {
  agent_type: "codex",
  name: "Codex",
  sort_order: 0,
} as AcpAgentInfo

const claudeAgent = {
  agent_type: "claude_code",
  name: "Claude Code",
  sort_order: 1,
} as AcpAgentInfo

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}

function skill(overrides: Partial<AgentSkillItem> = {}): AgentSkillItem {
  return {
    id: "demo",
    name: "Demo Skill",
    scope: "global",
    layout: "skill_directory",
    path: "/home/test/.codex/skills/demo/SKILL.md",
    description: "Demo",
    read_only: false,
    enabled: true,
    can_toggle: true,
    ...overrides,
  }
}

function listResult(item: AgentSkillItem): AgentSkillsListResult {
  return {
    supported: true,
    message: null,
    locations: [
      {
        scope: item.scope,
        path:
          item.scope === "project"
            ? "/work/project/.codex/skills"
            : "/home/test/.codex/skills",
        exists: true,
      },
    ],
    skills: [item],
  }
}

function renderSettings() {
  return render(
    <NextIntlClientProvider locale="en" messages={messages}>
      <SkillsSettings />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  api.acpListAgents.mockResolvedValue([agent])
  api.acpListAgentSkills.mockResolvedValue(listResult(skill()))
  api.acpReadAgentSkill.mockResolvedValue({
    skill: skill(),
    content: "# Demo",
  })
  api.acpSaveAgentSkill.mockResolvedValue(undefined)
  api.acpDeleteAgentSkill.mockResolvedValue(undefined)
  api.acpSetAgentSkillEnabled.mockResolvedValue(skill({ enabled: false }))
  api.loadFolderHistory.mockResolvedValue([])
  api.openFolder.mockResolvedValue(undefined)
})

describe("SkillsSettings availability", () => {
  it("toggles a skill without opening its row and reloads the authoritative list", async () => {
    api.acpListAgentSkills
      .mockResolvedValueOnce(listResult(skill()))
      .mockResolvedValueOnce(listResult(skill()))
      .mockResolvedValueOnce(listResult(skill({ enabled: false })))

    renderSettings()

    const availability = await screen.findByRole("switch", {
      name: "Toggle Demo Skill for Codex",
    })
    expect(availability).toBeChecked()
    await waitFor(() => expect(api.acpReadAgentSkill).toHaveBeenCalledTimes(1))

    fireEvent.click(availability)

    await waitFor(() =>
      expect(api.acpSetAgentSkillEnabled).toHaveBeenCalledWith({
        agentType: "codex",
        scope: "global",
        skillId: "demo",
        workspacePath: null,
        enabled: false,
      })
    )
    await waitFor(() => expect(api.acpListAgentSkills).toHaveBeenCalledTimes(3))
    await waitFor(() => expect(availability).not.toBeChecked())
    expect(api.acpReadAgentSkill).toHaveBeenCalledTimes(1)
    expect(toast.success).toHaveBeenCalledWith("Skill disabled")
  })

  it("disables non-toggleable skills and exposes an unavailable hint", async () => {
    api.acpListAgentSkills.mockResolvedValue(
      listResult(skill({ read_only: true, can_toggle: false }))
    )

    renderSettings()

    const availability = await screen.findByRole("switch", {
      name: "Toggle Demo Skill for Codex",
    })
    expect(availability).toBeDisabled()
    expect(availability).toHaveAttribute(
      "title",
      "Built-in skills are always available."
    )
  })

  it("explains when a shared skill cannot be toggled independently", async () => {
    api.acpListAgentSkills.mockResolvedValue(
      listResult(skill({ read_only: false, can_toggle: false }))
    )

    renderSettings()

    const availability = await screen.findByRole("switch", {
      name: "Toggle Demo Skill for Codex",
    })
    expect(availability).toBeDisabled()
    expect(availability).toHaveAttribute(
      "title",
      "This shared skill cannot be toggled independently."
    )
  })

  it("reloads authoritative state and reports a localized error after failure", async () => {
    const reload = deferred<AgentSkillsListResult>()
    api.acpSetAgentSkillEnabled.mockRejectedValue(
      new Error("permission denied")
    )
    api.acpListAgentSkills
      .mockResolvedValueOnce(listResult(skill()))
      .mockResolvedValueOnce(listResult(skill()))
      .mockReturnValueOnce(reload.promise)

    renderSettings()

    const availability = await screen.findByRole("switch", {
      name: "Toggle Demo Skill for Codex",
    })
    availability.focus()
    fireEvent.click(availability)

    await waitFor(() => expect(api.acpListAgentSkills).toHaveBeenCalledTimes(3))
    const switchDuringReload = screen.queryByRole("switch", {
      name: "Toggle Demo Skill for Codex",
    })
    const focusStayedOnSwitch = document.activeElement === switchDuringReload

    await act(async () => {
      reload.resolve(listResult(skill({ enabled: false })))
      await reload.promise
    })

    const authoritativeSwitch = await screen.findByRole("switch", {
      name: "Toggle Demo Skill for Codex",
    })
    await waitFor(() => expect(authoritativeSwitch).not.toBeChecked())
    expect(switchDuringReload).not.toBeNull()
    expect(focusStayedOnSwitch).toBe(true)
    expect(toast.error).toHaveBeenCalledWith(
      "Failed to update skill availability",
      { description: "permission denied" }
    )
  })

  it("ignores an old agent reload after the selected target changes", async () => {
    const staleCodexReload = deferred<AgentSkillsListResult>()
    let codexLoads = 0
    const codexSkill = skill({ name: "Codex Skill" })
    const claudeSkill = skill({
      id: "claude-demo",
      name: "Claude Skill",
      path: "/home/test/.claude/skills/claude-demo/SKILL.md",
    })

    api.acpListAgents.mockResolvedValue([agent, claudeAgent])
    api.acpListAgentSkills.mockImplementation(
      (params: { agentType: string; workspacePath?: string | null }) => {
        if (!("workspacePath" in params)) {
          return Promise.resolve(
            listResult(params.agentType === "codex" ? codexSkill : claudeSkill)
          )
        }
        if (params.agentType === "codex") {
          codexLoads += 1
          return codexLoads === 1
            ? Promise.resolve(listResult(codexSkill))
            : staleCodexReload.promise
        }
        return Promise.resolve(listResult(claudeSkill))
      }
    )

    renderSettings()

    fireEvent.click(
      await screen.findByRole("switch", {
        name: "Toggle Codex Skill for Codex",
      })
    )
    await waitFor(() => expect(codexLoads).toBe(2))

    const user = userEvent.setup()
    await user.click(screen.getByRole("combobox"))
    await user.click(await screen.findByRole("option", { name: "Claude Code" }))
    await screen.findByRole("switch", {
      name: "Toggle Claude Skill for Claude Code",
    })

    await act(async () => {
      staleCodexReload.resolve(
        listResult(skill({ name: "Stale Codex Skill", enabled: false }))
      )
      await staleCodexReload.promise
    })

    expect(
      screen.getByRole("switch", {
        name: "Toggle Claude Skill for Claude Code",
      })
    ).toBeChecked()
    expect(screen.queryByText("Stale Codex Skill")).not.toBeInTheDocument()
  })

  it("sends the selected project workspace when toggling a folder skill", async () => {
    const projectSkill = skill({
      id: "project-demo",
      name: "Project Skill",
      scope: "project",
      path: "/work/project/.codex/skills/project-demo/SKILL.md",
    })
    api.loadFolderHistory.mockResolvedValue([
      {
        id: 1,
        name: "Project",
        path: "/work/project",
        last_opened_at: "2026-09-07T00:00:00Z",
      },
    ])
    api.acpListAgentSkills.mockImplementation(
      (params: { workspacePath?: string | null }) =>
        Promise.resolve(
          params.workspacePath === "/work/project"
            ? listResult(projectSkill)
            : listResult(skill())
        )
    )

    renderSettings()

    const user = userEvent.setup()
    await user.click(await screen.findByRole("button", { name: "Folder" }))
    await waitFor(() => expect(api.loadFolderHistory).toHaveBeenCalledTimes(1))
    await user.click(screen.getAllByRole("combobox")[1])
    await user.click(
      await screen.findByRole("option", { name: /Project.*\/work\/project/ })
    )
    fireEvent.click(
      await screen.findByRole("switch", {
        name: "Toggle Project Skill for Codex",
      })
    )

    await waitFor(() =>
      expect(api.acpSetAgentSkillEnabled).toHaveBeenCalledWith({
        agentType: "codex",
        scope: "project",
        skillId: "project-demo",
        workspacePath: "/work/project",
        enabled: false,
      })
    )
  })
})
