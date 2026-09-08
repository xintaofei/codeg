import type { AgentType } from "./types"
import type {
  ModelProviderApiType,
  ModelProviderRecord,
} from "./model-provider-types"

/** Frontend capability declarations for the shared Model Provider source.
 * Backend adapters still need to support the same wire formats before launch. */
export const AGENT_MODEL_PROVIDER_API_TYPES: Partial<
  Record<AgentType, ModelProviderApiType[]>
> = {
  claude_code: ["anthropic-messages"],
  codex: ["openai-responses"],
  gemini: ["google-generative-ai"],
  open_code: [
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
    "google-generative-ai",
  ],
  cline: ["anthropic-messages", "openai-completions", "google-generative-ai"],
  grok: ["openai-completions", "openai-responses", "anthropic-messages"],
  pi: [
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
    "google-generative-ai",
  ],
  hermes: ["openai-completions", "anthropic-messages", "google-generative-ai"],
  code_buddy: ["openai-completions"],
  kimi_code: [
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
    "google-generative-ai",
  ],
  deepseek: ["openai-completions"],
  antigravity: ["google-generative-ai"],
}

/** Agents whose existing panel does not already have a Model Provider mode. */
export const MODEL_PROVIDER_SOURCE_CARD_AGENT_TYPES: AgentType[] = [
  "claude_code",
  "codex",
  "open_code",
  "cline",
  "hermes",
  "code_buddy",
  "kimi_code",
  "pi",
  "grok",
  "deepseek",
  "antigravity",
]

/** Stable ids are intentionally shown in the settings UI: provider and model
 * display names are not configurable, and API families are technical ids. */
export function getModelProviderApiTypes(
  agentType: AgentType
): ModelProviderApiType[] {
  return AGENT_MODEL_PROVIDER_API_TYPES[agentType] ?? []
}

export type CompatibleModelProvider = Pick<
  ModelProviderRecord,
  "api" | "enabled" | "models"
>

/** Count only providers a source can actually offer: enabled, a matching API
 * family, and at least one model id. */
export function countCompatibleModelProviders(
  providers: readonly CompatibleModelProvider[],
  agentType: AgentType
): number {
  const supported = new Set(getModelProviderApiTypes(agentType))
  return providers.filter(
    (provider) =>
      provider.enabled &&
      supported.has(provider.api) &&
      provider.models.length > 0
  ).length
}
