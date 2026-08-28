import { acpListAgents, getDelegationSettings } from "@/lib/api"

import type { PromptInputBlock } from "@/lib/types"

/**
 * An `@<agent>` mention delegates work to another agent only when BOTH gates
 * are open: multi-agent delegation is enabled in settings (the tool is
 * injected per connection), and the target agent itself is enabled. When a
 * gate is closed the host model silently answers the mention itself — to the
 * sender `@` just looks broken (upstream issue #545). These helpers let the
 * send path surface that as a hint at the exact moment it becomes true.
 */

/**
 * Agent mentions serialize as `[@label](codeg://agent/<agent_type>)` (see
 * `reference-text.ts`), so we anchor on the routing URI — free-standing
 * `@label` prose the user typed must not trigger the hint. Clean URIs stay
 * unescaped in the destination; ids containing spaces/parens (rare custom
 * agents) end up in the `<…>` form whose captures we don't attempt to parse.
 */
const AGENT_MENTION_URI = /codeg:\/\/agent\/([^)\s\\]+)/g

export function extractMentionedAgentTypes(text: string): string[] {
  const found = new Set<string>()
  for (const match of text.matchAll(AGENT_MENTION_URI)) {
    if (match[1]) found.add(match[1])
  }
  return [...found]
}

/** Scan the SEND wire blocks — agent mentions live in the prose text block. */
export function mentionedAgentTypesFromBlocks(
  blocks: PromptInputBlock[]
): string[] {
  const prose = blocks
    .flatMap((block) => (block.type === "text" ? [block.text] : []))
    .join("\n")
  return extractMentionedAgentTypes(prose)
}

interface GateSnapshot {
  delegationEnabled: boolean
  /** `agent_type → display name` for every agent disabled in Agents 管理. */
  disabledAgents: Map<string, string>
}

const SNAPSHOT_TTL_MS = 30_000
let cache: { at: number; value: Promise<GateSnapshot> } | null = null

function gateSnapshot(): Promise<GateSnapshot> {
  const now = Date.now()
  if (cache && now - cache.at < SNAPSHOT_TTL_MS) return cache.value
  const value = (async () => {
    const [settings, agents] = await Promise.all([
      getDelegationSettings(),
      acpListAgents(),
    ])
    const disabledAgents = new Map<string, string>()
    for (const agent of agents) {
      if (!agent.enabled) {
        disabledAgents.set(agent.agent_type, agent.name || agent.agent_type)
      }
    }
    return { delegationEnabled: settings.enabled, disabledAgents }
  })()
  cache = { at: now, value }
  return value
}

export interface BlockedAgentMentions {
  /** Multi-agent delegation is off — no mention can delegate. */
  delegationOff: boolean
  /** Delegation is on, but these mentioned agents are disabled in settings. */
  disabledAgents: Array<{ type: string; label: string }>
}

/**
 * Best-effort classification of the mentions in a sent draft. Throws only if
 * the settings/agents lookup fails — callers are expected to swallow that
 * (a missing hint must never break the send).
 */
export async function findBlockedAgentMentions(
  mentionedTypes: string[]
): Promise<BlockedAgentMentions> {
  if (mentionedTypes.length === 0) {
    return { delegationOff: false, disabledAgents: [] }
  }
  const snapshot = await gateSnapshot()
  if (!snapshot.delegationEnabled) {
    return { delegationOff: true, disabledAgents: [] }
  }
  return {
    delegationOff: false,
    disabledAgents: mentionedTypes
      .filter((type) => snapshot.disabledAgents.has(type))
      .map((type) => ({
        type,
        label: snapshot.disabledAgents.get(type) ?? type,
      })),
  }
}
