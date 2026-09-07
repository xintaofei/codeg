import { fireEvent, render, screen, waitFor } from "@testing-library/react"
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
        scope: "global",
        path: "/home/test/.codex/skills",
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
    api.acpSetAgentSkillEnabled.mockRejectedValue(
      new Error("permission denied")
    )
    api.acpListAgentSkills
      .mockResolvedValueOnce(listResult(skill()))
      .mockResolvedValueOnce(listResult(skill()))
      .mockResolvedValueOnce(listResult(skill()))

    renderSettings()

    const availability = await screen.findByRole("switch", {
      name: "Toggle Demo Skill for Codex",
    })
    fireEvent.click(availability)

    await waitFor(() => expect(api.acpListAgentSkills).toHaveBeenCalledTimes(3))
    expect(availability).toBeChecked()
    expect(toast.error).toHaveBeenCalledWith(
      "Failed to update skill availability",
      { description: "permission denied" }
    )
  })
})
