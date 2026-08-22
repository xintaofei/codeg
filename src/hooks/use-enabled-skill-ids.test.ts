import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { AcpAgentInfo, ExpertInstallStatus } from "@/lib/types"

// All three status scans are mocked so we can count how many times a window
// focus triggers them across multiple mounted consumers. `acpListAgents` is
// pulled in transitively via `useAcpAgents` (the hook reads each agent's
// env_json to spot a custom-dir pi); it returns no agents here, so detection
// stays inert.
vi.mock("@/lib/api", () => ({
  expertsListAllInstallStatuses: vi.fn(),
  officecliSkillListAllInstallStatuses: vi.fn(),
  scienceListAllInstallStatuses: vi.fn(),
  acpListAgents: vi.fn().mockResolvedValue([]),
}))

// `useAcpAgents` subscribes through the platform layer; stub it to inert
// disposers so the hook mounts cleanly under jsdom.
vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn(async () => () => {}),
  onTransportReconnect: vi.fn(() => () => {}),
}))

// The hook caches snapshot + focus-listener state at module scope; reset the
// module registry per test so each starts uncached with a fresh refcount.
beforeEach(() => {
  vi.resetModules()
})

async function setup() {
  const api = await import("@/lib/api")
  vi.mocked(api.expertsListAllInstallStatuses).mockResolvedValue([])
  vi.mocked(api.officecliSkillListAllInstallStatuses).mockResolvedValue([])
  vi.mocked(api.scienceListAllInstallStatuses).mockResolvedValue([])
  const hook = await import("./use-enabled-skill-ids")
  return { api, hook }
}

describe("useEnabledSkillIds — focus refresh coalescing", () => {
  it("runs a single (experts + office + science) refresh per focus regardless of how many consumers are mounted", async () => {
    const { api, hook } = await setup()
    // Two mounted consumers — e.g. two tiled conversation composers.
    const a = renderHook(() => hook.useEnabledSkillIds("claude_code"))
    const b = renderHook(() => hook.useEnabledSkillIds("codex"))
    await waitFor(() => {
      expect(a.result.current.ready).toBe(true)
      expect(b.result.current.ready).toBe(true)
    })

    vi.mocked(api.expertsListAllInstallStatuses).mockClear()
    vi.mocked(api.officecliSkillListAllInstallStatuses).mockClear()
    vi.mocked(api.scienceListAllInstallStatuses).mockClear()

    // A window focus must coalesce to ONE refresh — not one scan per instance
    // (the pre-fix behavior cleared `inflight` per listener, defeating dedup).
    await act(async () => {
      window.dispatchEvent(new Event("focus"))
      await Promise.resolve()
    })

    await waitFor(() => {
      expect(api.expertsListAllInstallStatuses).toHaveBeenCalledTimes(1)
      expect(api.officecliSkillListAllInstallStatuses).toHaveBeenCalledTimes(1)
      expect(api.scienceListAllInstallStatuses).toHaveBeenCalledTimes(1)
    })
  })

  it("detaches the shared listener once the last consumer unmounts", async () => {
    const { api, hook } = await setup()
    const a = renderHook(() => hook.useEnabledSkillIds("claude_code"))
    const b = renderHook(() => hook.useEnabledSkillIds("claude_code"))
    await waitFor(() => expect(a.result.current.ready).toBe(true))

    a.unmount()
    b.unmount()
    vi.mocked(api.expertsListAllInstallStatuses).mockClear()
    vi.mocked(api.officecliSkillListAllInstallStatuses).mockClear()
    vi.mocked(api.scienceListAllInstallStatuses).mockClear()

    await act(async () => {
      window.dispatchEvent(new Event("focus"))
      await Promise.resolve()
    })

    expect(api.expertsListAllInstallStatuses).not.toHaveBeenCalled()
    expect(api.officecliSkillListAllInstallStatuses).not.toHaveBeenCalled()
    expect(api.scienceListAllInstallStatuses).not.toHaveBeenCalled()
  })
})

// Only the fields the hook + useAcpAgents read; the rest of AcpAgentInfo is
// irrelevant to skill-management gating.
function piAgent(env: Record<string, string>): AcpAgentInfo {
  return {
    agent_type: "pi",
    name: "Pi",
    sort_order: 0,
    env,
  } as unknown as AcpAgentInfo
}

function piLinkedStatus(expertId: string): ExpertInstallStatus {
  return {
    expertId,
    agentType: "pi",
    state: "linked_to_codeg",
    linkPath: "",
    targetPath: null,
    expectedTargetPath: "",
    copyMode: false,
  }
}

function linkedStatus(
  expertId: string,
  agentType: ExpertInstallStatus["agentType"]
): ExpertInstallStatus {
  return {
    ...piLinkedStatus(expertId),
    agentType,
  }
}

describe("useEnabledSkillIds — project ownership", () => {
  it("merges global and selected-project links without leaking another folder", async () => {
    const { api, hook } = await setup()
    vi.mocked(api.expertsListAllInstallStatuses).mockImplementation(
      async (params) => {
        if (params.scope === "global") {
          return [linkedStatus("global-skill", "claude_code")]
        }
        return [
          linkedStatus(
            params.workspacePath === "D:\\Work\\Alpha"
              ? "alpha-skill"
              : "beta-skill",
            "claude_code"
          ),
        ]
      }
    )

    const alpha = renderHook(() =>
      hook.useEnabledSkillIds("claude_code", "D:\\Work\\Alpha")
    )

    await waitFor(() => expect(alpha.result.current.ready).toBe(true))
    expect(Array.from(alpha.result.current.enabledIds).sort()).toEqual([
      "alpha-skill",
      "global-skill",
    ])
    expect(api.expertsListAllInstallStatuses).toHaveBeenCalledWith({
      scope: "project",
      workspacePath: "D:\\Work\\Alpha",
    })
    expect(alpha.result.current.enabledIds.has("beta-skill")).toBe(false)
  })
})

describe("useEnabledSkillIds — custom-dir pi gating", () => {
  it("never exposes managed skills for a custom-dir pi, before or after the registry resolves", async () => {
    const { api, hook } = await setup()
    // A pi pinned to a custom PI_CODING_AGENT_DIR, plus a default-dir expert
    // link that must NOT be surfaced for it.
    vi.mocked(api.acpListAgents).mockResolvedValue([
      piAgent({ PI_CODING_AGENT_DIR: "/custom/pi" }),
    ])
    vi.mocked(api.expertsListAllInstallStatuses).mockResolvedValue([
      piLinkedStatus("writer"),
    ])

    const { result } = renderHook(() => hook.useEnabledSkillIds("pi"))

    // Optimistic window: registry not yet loaded → pessimistically unmanaged,
    // so no default-dir link leaks even though the snapshot may resolve first.
    expect(result.current.supported).toBe(false)
    expect(result.current.enabledIds.size).toBe(0)

    // Let the agent registry and the skill snapshot both resolve.
    await waitFor(() => expect(api.acpListAgents).toHaveBeenCalled())
    await waitFor(() =>
      expect(api.expertsListAllInstallStatuses).toHaveBeenCalled()
    )
    await act(async () => {
      await Promise.resolve()
    })

    // Custom dir now known → still unmanaged, link still suppressed.
    expect(result.current.supported).toBe(false)
    expect(result.current.enabledIds.size).toBe(0)
  })

  it("manages a default-dir pi once the registry resolves", async () => {
    const { api, hook } = await setup()
    vi.mocked(api.acpListAgents).mockResolvedValue([piAgent({})])
    vi.mocked(api.expertsListAllInstallStatuses).mockResolvedValue([
      piLinkedStatus("writer"),
    ])

    const { result } = renderHook(() => hook.useEnabledSkillIds("pi"))

    // Held unmanaged until the registry is known (pessimistic during load).
    expect(result.current.supported).toBe(false)

    // Once the registry is fresh and the agent is default-dir, pi is managed
    // and its linked skill surfaces.
    await waitFor(() => expect(result.current.supported).toBe(true))
    await waitFor(() =>
      expect(result.current.enabledIds.has("writer")).toBe(true)
    )
  })
})
