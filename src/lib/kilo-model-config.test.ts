import { describe, expect, it } from "vitest"

import {
  addKiloModels,
  kiloModelsFromConfig,
  setKiloModelReasoning,
  setKiloModelVariantEnabled,
} from "./kilo-model-config"

describe("kilo model config helpers", () => {
  it("lists configured models and defaults reasoning to enabled", () => {
    expect(
      kiloModelsFromConfig({
        provider: {
          openai: {
            models: {
              fast: { name: "fast" },
              careful: { name: "careful", reasoning: false },
            },
          },
        },
      })
    ).toEqual([
      { providerId: "openai", modelId: "fast", reasoning: true },
      { providerId: "openai", modelId: "careful", reasoning: false },
    ])
  })

  it("adds models without mutating the source config", () => {
    const config = { provider: { openai: { models: {} } } }
    const next = addKiloModels(config, "openai", ["fast", "careful"])

    expect(config).toEqual({ provider: { openai: { models: {} } } })
    expect(next).toMatchObject({
      provider: {
        openai: {
          models: {
            fast: { name: "fast", reasoning: true },
            careful: { name: "careful", reasoning: true },
          },
        },
      },
    })
  })

  it("does not overwrite an existing model declaration", () => {
    const next = addKiloModels(
      {
        provider: {
          openai: {
            models: {
              fast: { name: "Fast", reasoning: false, custom: "value" },
            },
          },
        },
      },
      "openai",
      ["fast"]
    )

    expect(next).toMatchObject({
      provider: {
        openai: {
          models: {
            fast: { name: "Fast", reasoning: false, custom: "value" },
          },
        },
      },
    })
  })

  it("updates reasoning while preserving model fields", () => {
    const config = {
      provider: {
        openai: {
          models: {
            fast: { name: "Fast", custom: "value", reasoning: true },
          },
        },
      },
    }
    const next = setKiloModelReasoning(
      config,
      { providerId: "openai", modelId: "fast", reasoning: true },
      false
    )

    expect(next).toMatchObject({
      provider: {
        openai: {
          models: {
            fast: { name: "Fast", custom: "value", reasoning: false },
          },
        },
      },
    })
    expect(config.provider.openai.models.fast.reasoning).toBe(true)
  })

  it("updates variant enablement immutably", () => {
    const config = {
      provider: {
        openai: {
          models: {
            fast: {
              variants: {
                low: { reasoningEffort: "low", disabled: false },
                high: { reasoningEffort: "high", label: "High" },
              },
            },
          },
        },
      },
    }
    const next = setKiloModelVariantEnabled(
      config,
      "openai",
      "fast",
      "high",
      false
    )

    expect(next).toMatchObject({
      provider: {
        openai: {
          models: {
            fast: {
              variants: {
                low: { reasoningEffort: "low" },
                high: {
                  reasoningEffort: "high",
                  label: "High",
                  disabled: true,
                },
              },
            },
          },
        },
      },
    })
    expect(config.provider.openai.models.fast.variants.low.disabled).toBe(false)
  })
})
