import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  applyDraftModelSelection,
  type ModelSelectionConnection,
} from "./apply-conversation-model-selection"
import { loadRememberedAgentModelSelection } from "./remembered-agent-model-selection"
import {
  getModelProviderDraftSelection,
  resetModelProviderSelectionStore,
  setModelProviderDraftSelection,
} from "@/stores/model-provider-selection-store"

vi.mock("@/lib/api", () => ({
  updateConversationModelSelection: vi.fn(),
}))

const { updateConversationModelSelection } = vi.mocked(
  await import("@/lib/api")
)

const selection = { providerId: "anthropic", modelId: "claude-opus-4-5" }
const agentType = "claude_code"

function connection(
  overrides: Partial<ModelSelectionConnection> = {}
): ModelSelectionConnection {
  return {
    isViewer: false,
    status: "connected",
    reapplyConfig: vi.fn(async () => true),
    ...overrides,
  }
}

describe("applyDraftModelSelection", () => {
  beforeEach(() => {
    resetModelProviderSelectionStore()
    updateConversationModelSelection.mockReset()
    localStorage.clear()
  })

  it("saves the draft, applies it to a fresh session, and remembers it", async () => {
    setModelProviderDraftSelection("tab-1", selection)
    updateConversationModelSelection.mockResolvedValue(undefined)
    const conn = connection()

    await expect(
      applyDraftModelSelection({
        tabId: "tab-1",
        conversationId: 42,
        agentType,
        connection: conn,
        freshSession: true,
      })
    ).resolves.toEqual(selection)

    expect(updateConversationModelSelection).toHaveBeenCalledWith(
      42,
      selection.providerId,
      selection.modelId
    )
    expect(conn.reapplyConfig).toHaveBeenCalledWith(42, { freshSession: true })
    expect(getModelProviderDraftSelection("tab-1")).toBeNull()
    expect(loadRememberedAgentModelSelection(agentType)).toEqual(selection)
  })

  it("keeps resume semantics for an existing conversation", async () => {
    setModelProviderDraftSelection("tab-1", selection)
    updateConversationModelSelection.mockResolvedValue(undefined)
    const conn = connection()

    await applyDraftModelSelection({
      tabId: "tab-1",
      conversationId: 42,
      agentType,
      connection: conn,
    })

    expect(conn.reapplyConfig).toHaveBeenCalledWith(42, {
      freshSession: false,
    })
    expect(loadRememberedAgentModelSelection(agentType)).toEqual(selection)
  })

  it("does nothing when the draft has no selection", async () => {
    const conn = connection()

    await expect(
      applyDraftModelSelection({
        tabId: "tab-1",
        conversationId: 42,
        agentType,
        connection: conn,
      })
    ).resolves.toBeNull()

    expect(updateConversationModelSelection).not.toHaveBeenCalled()
    expect(conn.reapplyConfig).not.toHaveBeenCalled()
    expect(loadRememberedAgentModelSelection(agentType)).toBeNull()
  })

  it("restores the draft and skips reconnect when saving fails", async () => {
    setModelProviderDraftSelection("tab-1", selection)
    updateConversationModelSelection.mockRejectedValue(new Error("save failed"))
    const conn = connection()

    await expect(
      applyDraftModelSelection({
        tabId: "tab-1",
        conversationId: 42,
        agentType,
        connection: conn,
      })
    ).rejects.toThrow("save failed")

    expect(conn.reapplyConfig).not.toHaveBeenCalled()
    expect(getModelProviderDraftSelection("tab-1")).toEqual(selection)
    expect(loadRememberedAgentModelSelection(agentType)).toBeNull()
  })

  it("restores the draft when applying the saved selection fails", async () => {
    setModelProviderDraftSelection("tab-1", selection)
    updateConversationModelSelection.mockResolvedValue(undefined)
    const conn = connection({
      reapplyConfig: vi.fn(async () => {
        throw new Error("reconnect failed")
      }),
    })

    await expect(
      applyDraftModelSelection({
        tabId: "tab-1",
        conversationId: 42,
        agentType,
        connection: conn,
      })
    ).rejects.toThrow("reconnect failed")

    expect(updateConversationModelSelection).toHaveBeenCalledTimes(1)
    expect(getModelProviderDraftSelection("tab-1")).toEqual(selection)
    // The save succeeded, so the memory is written even though the reconnect
    // afterwards failed (the retry will re-apply the same choice).
    expect(loadRememberedAgentModelSelection(agentType)).toEqual(selection)
  })
})
