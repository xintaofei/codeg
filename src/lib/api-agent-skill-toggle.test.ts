import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  call: vi.fn(),
  invoke: vi.fn(),
}))

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: mocks.call }),
  getShellTransport: () => ({ call: vi.fn() }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
}))

import { acpSetAgentSkillEnabled as callAgentSkillToggle } from "@/lib/api"
import { acpSetAgentSkillEnabled as invokeAgentSkillToggle } from "@/lib/tauri"
import type { AgentSkillItem } from "@/lib/types"

const TOGGLE_RESULT: AgentSkillItem = {
  id: "example",
  name: "Example",
  scope: "project",
  layout: "skill_directory",
  path: "/workspace/.agents/skills/example",
  description: null,
  read_only: false,
  enabled: false,
  can_toggle: true,
}

describe("acpSetAgentSkillEnabled", () => {
  beforeEach(() => {
    mocks.call.mockReset()
    mocks.invoke.mockReset()
  })

  it("sends camelCase params through the shared transport", async () => {
    mocks.call.mockResolvedValue(TOGGLE_RESULT)

    await expect(
      callAgentSkillToggle({
        agentType: "codex",
        scope: "project",
        skillId: "example",
        enabled: false,
      })
    ).resolves.toBe(TOGGLE_RESULT)

    expect(mocks.call).toHaveBeenCalledWith("acp_set_agent_skill_enabled", {
      agentType: "codex",
      scope: "project",
      skillId: "example",
      workspacePath: null,
      enabled: false,
    })
  })

  it("uses the same command contract for desktop invoke", async () => {
    mocks.invoke.mockResolvedValue(TOGGLE_RESULT)

    await expect(
      invokeAgentSkillToggle({
        agentType: "claude",
        scope: "global",
        skillId: "example",
        workspacePath: "/workspace",
        enabled: true,
      })
    ).resolves.toBe(TOGGLE_RESULT)

    expect(mocks.invoke).toHaveBeenCalledWith("acp_set_agent_skill_enabled", {
      agentType: "claude",
      scope: "global",
      skillId: "example",
      workspacePath: "/workspace",
      enabled: true,
    })
  })
})
