import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { AgentSkillItem } from "@/lib/types"

const api = vi.hoisted(() => ({
  acpListAgentSkills: vi.fn(),
}))

vi.mock("@/lib/api", () => api)

import { invalidateAgentSkillsCache, useAgentSkills } from "./use-agent-skills"

function skill(id: string, enabled: boolean): AgentSkillItem {
  return {
    id,
    name: id,
    scope: "global",
    layout: "skill_directory",
    path: `/home/test/.codex/skills/${id}/SKILL.md`,
    description: null,
    read_only: false,
    enabled,
    can_toggle: true,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  invalidateAgentSkillsCache()
})

describe("useAgentSkills", () => {
  it("excludes disabled skills from autocomplete results and its cache", async () => {
    api.acpListAgentSkills.mockResolvedValue({
      supported: true,
      message: null,
      locations: [],
      skills: [skill("enabled", true), skill("disabled", false)],
    })

    const first = renderHook(() => useAgentSkills("codex", null))

    await waitFor(() =>
      expect(first.result.current.map((item) => item.id)).toEqual(["enabled"])
    )
    first.unmount()

    const cached = renderHook(() => useAgentSkills("codex", null))
    expect(cached.result.current.map((item) => item.id)).toEqual(["enabled"])
    expect(api.acpListAgentSkills).toHaveBeenCalledTimes(1)
  })

  it("replaces a warm cached result after a focus refresh", async () => {
    api.acpListAgentSkills
      .mockResolvedValueOnce({
        supported: true,
        message: null,
        locations: [],
        skills: [skill("demo", true)],
      })
      .mockResolvedValueOnce({
        supported: true,
        message: null,
        locations: [],
        skills: [skill("demo", false)],
      })

    const prime = renderHook(() => useAgentSkills("codex", null))
    await waitFor(() => expect(prime.result.current).toHaveLength(1))
    prime.unmount()

    const warm = renderHook(() => useAgentSkills("codex", null))
    expect(warm.result.current).toHaveLength(1)

    act(() => {
      window.dispatchEvent(new Event("focus"))
    })

    await waitFor(() => expect(warm.result.current).toEqual([]))
    expect(api.acpListAgentSkills).toHaveBeenCalledTimes(2)
  })
})
