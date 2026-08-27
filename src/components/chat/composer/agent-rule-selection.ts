import type { JSONContent } from "@tiptap/core"

export const AGENT_RULE_SELECTION_NODE = "agentRuleSelection"
export const AGENT_RULES_EXPERT_ID = "agent-rules-picker"

export interface AgentRuleSelectionAttrs {
  version: 1
  ruleIds: string[]
  sourceHash: string
  sources: string[]
  exactText: string
  envelopeNonce: string
}

interface EnvelopeMetadata {
  version: 1
  sourceHash: string
  ruleIds: string[]
  sources: string[]
  nonce: string
}

const INTRO =
  "The following optional workspace instructions were selected in Codeg and apply to this turn and its delegated tasks:"

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string")
}

export function isAgentRuleSelectionAttrs(
  value: unknown
): value is AgentRuleSelectionAttrs {
  if (!value || typeof value !== "object") return false
  const attrs = value as Partial<AgentRuleSelectionAttrs>
  return (
    attrs.version === 1 &&
    isStringArray(attrs.ruleIds) &&
    typeof attrs.sourceHash === "string" &&
    isStringArray(attrs.sources) &&
    typeof attrs.exactText === "string" &&
    typeof attrs.envelopeNonce === "string" &&
    attrs.envelopeNonce.length > 0
  )
}

export function serializeAgentRuleSelection(
  attrs: AgentRuleSelectionAttrs
): string {
  const metadata: EnvelopeMetadata = {
    version: attrs.version,
    sourceHash: attrs.sourceHash,
    ruleIds: attrs.ruleIds,
    sources: attrs.sources,
    nonce: attrs.envelopeNonce,
  }
  return `/${AGENT_RULES_EXPERT_ID}\n<!-- codeg-agent-rules-selection ${JSON.stringify(metadata)} -->\n${INTRO}\n\n${attrs.exactText}<!-- /codeg-agent-rules-selection:${attrs.envelopeNonce} -->`
}

/** Parse only in trusted queue/draft restoration. Generic paste stays literal. */
export function parseAgentRuleSelectionEnvelope(text: string): {
  attrs: AgentRuleSelectionAttrs
  remainder: string
} | null {
  const invocation = text.match(/^\/agent-rules-picker\n/)
  if (!invocation) return null
  const metadataPrefix = "<!-- codeg-agent-rules-selection "
  const metadataStart = invocation[0].length
  if (!text.startsWith(metadataPrefix, metadataStart)) return null
  const metadataEnd = text.indexOf(" -->\n", metadataStart)
  if (metadataEnd < 0) return null
  let metadata: Partial<EnvelopeMetadata>
  try {
    metadata = JSON.parse(
      text.slice(metadataStart + metadataPrefix.length, metadataEnd)
    ) as Partial<EnvelopeMetadata>
  } catch {
    return null
  }
  if (
    metadata.version !== 1 ||
    typeof metadata.sourceHash !== "string" ||
    !isStringArray(metadata.ruleIds) ||
    !isStringArray(metadata.sources) ||
    typeof metadata.nonce !== "string" ||
    metadata.nonce.length === 0
  ) {
    return null
  }
  const bodyPrefix = `${INTRO}\n\n`
  const bodyStart = metadataEnd + " -->\n".length
  if (!text.startsWith(bodyPrefix, bodyStart)) return null
  const exactStart = bodyStart + bodyPrefix.length
  const closing = `<!-- /codeg-agent-rules-selection:${metadata.nonce} -->`
  const closingStart = text.indexOf(closing, exactStart)
  if (closingStart < 0) return null
  const attrs: AgentRuleSelectionAttrs = {
    version: 1,
    ruleIds: metadata.ruleIds,
    sourceHash: metadata.sourceHash,
    sources: metadata.sources,
    exactText: text.slice(exactStart, closingStart),
    envelopeNonce: metadata.nonce,
  }
  return {
    attrs,
    remainder: text.slice(closingStart + closing.length),
  }
}

export function agentRuleSelectionContent(
  attrs: AgentRuleSelectionAttrs
): JSONContent {
  return { type: AGENT_RULE_SELECTION_NODE, attrs }
}
