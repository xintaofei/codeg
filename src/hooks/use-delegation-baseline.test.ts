import { act, renderHook, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { AgentDelegationDefaults } from "@/lib/types"

const getDelegationSettings = vi.fn<
  () => Promise<{
    agent_defaults?: Partial<Record<string, AgentDelegationDefaults>>
  }>
>()

vi.mock("@/lib/api", () => ({
  getDelegationSettings: () => getDelegationSettings(),
}))

import { useDelegationGlobalBaseline } from "./use-delegation-baseline"

afterEach(() => {
  getDelegationSettings.mockReset()
})

describe("useDelegationGlobalBaseline", () => {
  it("does not fetch while the popover is closed", () => {
    renderHook(() => useDelegationGlobalBaseline(false))
    expect(getDelegationSettings).not.toHaveBeenCalled()
  })

  it("clears the baseline and loads it on open", async () => {
    getDelegationSettings.mockResolvedValue({
      agent_defaults: { codex: { config_values: { model: "gpt-5.2" } } },
    })
    const { result, rerender } = renderHook(
      ({ active }) => useDelegationGlobalBaseline(active),
      { initialProps: { active: false } }
    )

    rerender({ active: true })
    // Loading state is visible IMMEDIATELY (before any promise settles):
    // baseline is cleared so a stale value can never seed an edit.
    expect(result.current).toMatchObject({
      baseline: null,
      loading: true,
      error: null,
    })

    await waitFor(() =>
      expect(result.current).toMatchObject({
        baseline: { codex: { config_values: { model: "gpt-5.2" } } },
        loading: false,
        error: null,
      })
    )
  })

  it("re-clears and re-fetches on reopen, picking up settings changed in between", async () => {
    // Reviewer regression: the first open cached the baseline; on reopen the
    // old values must NOT serve an edit while the fresh fetch is in flight.
    getDelegationSettings.mockResolvedValueOnce({
      agent_defaults: { codex: { config_values: { model: "old-model" } } },
    })
    const { result, rerender } = renderHook(
      ({ active }) => useDelegationGlobalBaseline(active),
      { initialProps: { active: false } }
    )
    rerender({ active: true })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.baseline?.codex?.config_values?.model).toBe(
      "old-model"
    )

    // The user edits global defaults while the popover is closed.
    getDelegationSettings.mockResolvedValueOnce({
      agent_defaults: { codex: { config_values: { model: "new-model" } } },
    })
    rerender({ active: false })
    expect(result.current).toMatchObject({
      baseline: null,
      loading: false,
      error: null,
    })

    rerender({ active: true })
    // The stale "old-model" baseline is GONE the moment the popover reopens.
    expect(result.current).toMatchObject({
      baseline: null,
      loading: true,
      error: null,
    })
    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(result.current.baseline?.codex?.config_values?.model).toBe(
      "new-model"
    )
  })

  it("cancels a superseded fetch when the popover closes mid-load", async () => {
    let resolve!: (v: { agent_defaults?: Record<string, never> }) => void
    getDelegationSettings.mockReturnValue(
      new Promise((r) => {
        resolve = r
      })
    )
    const { result, rerender } = renderHook(
      ({ active }) => useDelegationGlobalBaseline(active),
      { initialProps: { active: false } }
    )
    rerender({ active: true })
    expect(result.current.loading).toBe(true)

    rerender({ active: false })
    await act(async () => {
      resolve({})
    })
    // The late response must not land after close.
    expect(result.current).toMatchObject({
      baseline: null,
      loading: false,
      error: null,
    })
  })

  it("surfaces a failed fetch and retries without inventing an empty baseline", async () => {
    getDelegationSettings.mockRejectedValueOnce(new Error("database offline"))
    getDelegationSettings.mockResolvedValueOnce({
      agent_defaults: { codex: { config_values: { model: "gpt-5.2" } } },
    })
    const { result, rerender } = renderHook(
      ({ active }) => useDelegationGlobalBaseline(active),
      { initialProps: { active: false } }
    )

    rerender({ active: true })
    await waitFor(() =>
      expect(result.current).toMatchObject({
        baseline: null,
        loading: false,
        error: "database offline",
      })
    )

    act(() => result.current.retry())
    await waitFor(() =>
      expect(result.current).toMatchObject({
        baseline: { codex: { config_values: { model: "gpt-5.2" } } },
        loading: false,
        error: null,
      })
    )
  })
})
