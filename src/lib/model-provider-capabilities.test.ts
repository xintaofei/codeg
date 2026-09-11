import { describe, expect, it } from "vitest"

import {
  countCompatibleModelProviders,
  getModelProviderApiTypes,
  MODEL_PROVIDER_SOURCE_CARD_AGENT_TYPES,
} from "./model-provider-capabilities"
import type { ModelProviderRecord } from "./model-provider-types"

function provider(
  overrides: Partial<ModelProviderRecord> = {}
): ModelProviderRecord {
  return {
    providerId: "provider",
    api: "openai-completions",
    baseUrl: "https://example.test",
    enabled: true,
    models: [{ id: "model", reasoning: false, input: "text" }],
    apiKeyMasked: "",
    hasApiKey: true,
    compatSupportsDeveloperRole: null,
    ...overrides,
  }
}

describe("model provider capabilities", () => {
  it("declares supported API families per agent", () => {
    expect(getModelProviderApiTypes("claude_code")).toEqual([
      "anthropic-messages",
    ])
    expect(getModelProviderApiTypes("codex")).toEqual(["openai-responses"])
    expect(getModelProviderApiTypes("custom:agent")).toEqual([])
  })

  it("counts enabled providers with a matching API and at least one model", () => {
    const providers = [
      provider({ providerId: "match", api: "openai-responses" }),
      provider({ providerId: "wrong-api", api: "anthropic-messages" }),
      provider({ providerId: "disabled", enabled: false }),
      provider({ providerId: "empty", models: [] }),
    ]
    expect(countCompatibleModelProviders(providers, "codex")).toBe(1)
  })

  // Codex has declared API families but no source card, so its settings panel
  // could never flip agent_setting.model_source and the composer picker never
  // appeared (the panel's own "model provider" auth mode is a legacy binding).
  it("gives codex a settings source card", () => {
    expect(MODEL_PROVIDER_SOURCE_CARD_AGENT_TYPES).toContain("codex")
  })

  it("gives Claude a settings source card", () => {
    expect(MODEL_PROVIDER_SOURCE_CARD_AGENT_TYPES).toContain("claude_code")
  })
})
