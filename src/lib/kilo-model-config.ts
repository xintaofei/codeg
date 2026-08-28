export type KiloModelEntry = {
  providerId: string
  modelId: string
  reasoning: boolean
}
type JsonObject = Record<string, unknown>

function asObject(value: unknown): JsonObject | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonObject)
    : null
}

export function kiloModelsFromConfig(config: JsonObject): KiloModelEntry[] {
  const providers = asObject(config.provider)
  if (!providers) return []

  return Object.entries(providers).flatMap(([providerId, value]) => {
    const models = asObject(asObject(value)?.models)
    if (!models) return []

    return Object.entries(models).map(([modelId, model]) => ({
      providerId,
      modelId,
      reasoning: asObject(model)?.reasoning !== false,
    }))
  })
}

export function addKiloModel(
  config: JsonObject,
  providerId: string,
  modelId: string
): JsonObject {
  return addKiloModels(config, providerId, [modelId])
}

export function addKiloModels(
  config: JsonObject,
  providerId: string,
  modelIds: string[]
): JsonObject {
  const providers = { ...(asObject(config.provider) ?? {}) }
  const provider = { ...(asObject(providers[providerId]) ?? {}) }
  const models = { ...(asObject(provider.models) ?? {}) }

  for (const modelId of modelIds) {
    const existing = asObject(models[modelId])
    models[modelId] = existing
      ? { ...existing }
      : { name: modelId, reasoning: true }
  }
  provider.models = models
  providers[providerId] = provider

  return { ...config, provider: providers }
}

export function setKiloModelReasoning(
  config: JsonObject,
  entry: KiloModelEntry,
  reasoning: boolean
): JsonObject {
  const providers = { ...(asObject(config.provider) ?? {}) }
  const provider = { ...(asObject(providers[entry.providerId]) ?? {}) }
  const models = { ...(asObject(provider.models) ?? {}) }
  const model = { ...(asObject(models[entry.modelId]) ?? {}) }

  models[entry.modelId] = {
    ...model,
    name:
      typeof model.name === "string" && model.name.trim()
        ? model.name
        : entry.modelId,
    reasoning,
  }
  provider.models = models
  providers[entry.providerId] = provider

  return { ...config, provider: providers }
}

export function setKiloModelVariantEnabled(
  config: JsonObject,
  providerId: string,
  modelId: string,
  level: string,
  enabled: boolean
): JsonObject {
  const providers = { ...(asObject(config.provider) ?? {}) }
  const provider = { ...(asObject(providers[providerId]) ?? {}) }
  const models = { ...(asObject(provider.models) ?? {}) }
  const model = { ...(asObject(models[modelId]) ?? {}) }
  const variants = { ...(asObject(model.variants) ?? {}) }

  for (const [variantId, value] of Object.entries(variants)) {
    const variant = asObject(value)
    if (variant?.disabled === false) {
      const nextVariant = { ...variant }
      delete nextVariant.disabled
      variants[variantId] = nextVariant
    }
  }

  const variant: JsonObject = {
    ...(asObject(variants[level]) ?? {}),
    reasoningEffort: level,
  }
  if (enabled) delete variant.disabled
  else variant.disabled = true
  variants[level] = variant
  model.variants = variants
  models[modelId] = model
  provider.models = models
  providers[providerId] = provider

  return { ...config, provider: providers }
}
