"use client"

import { useEffect, useState } from "react"

import { describeAgentOptions } from "@/lib/api"
import type { AgentType } from "@/lib/types"

export interface AgentModelChoice {
  value: string
  label: string
}

/**
 * The models a given agent says it accepts, probed live.
 *
 * The ids differ per agent and change with the adapter version, so hardcoding
 * them would go stale silently. The answer is stamped with the agent it
 * belongs to: switching agents cannot show the previous one's models, and the
 * caller can tell "still asking" from "this agent offers none".
 *
 * A probe that fails resolves to an empty list rather than throwing — every
 * caller has to keep working when the agent is not up, and a blank list is the
 * honest answer in that case.
 */
export function useAgentModels(
  agentType: string | null | undefined,
  enabled = true
): { choices: AgentModelChoice[]; probing: boolean } {
  const [probe, setProbe] = useState<{
    agent: string
    choices: AgentModelChoice[]
  } | null>(null)

  useEffect(() => {
    if (!enabled || !agentType) return
    let cancelled = false
    void describeAgentOptions(agentType as AgentType)
      .then((snapshot) => {
        if (cancelled) return
        const models = snapshot.config_options.find(
          (o) => o.id === "model" && o.kind.type === "select"
        )
        if (!models || models.kind.type !== "select") {
          setProbe({ agent: agentType, choices: [] })
          return
        }
        // Grouped agents (Antigravity groups its models by family) list their
        // options only inside the groups, so both places have to be read.
        const flat = [
          ...models.kind.options,
          ...models.kind.groups.flatMap((g) => g.options),
        ]
        setProbe({
          agent: agentType,
          choices: flat.map((o) => ({
            value: o.value,
            label: o.name || o.value,
          })),
        })
      })
      .catch((e) => {
        console.error("[useAgentModels] probe failed:", e)
        if (!cancelled) setProbe({ agent: agentType, choices: [] })
      })
    return () => {
      cancelled = true
    }
  }, [agentType, enabled])

  const answered = !!agentType && probe?.agent === agentType
  return {
    choices: answered ? (probe?.choices ?? []) : [],
    probing: !!agentType && enabled && !answered,
  }
}
