import type { PkRound } from "@/stores/pk-arena-store"

const isLiveRound = (round: PkRound) =>
  round.status === "ready" || round.status === "running"

/** Pick the live round represented by the minimized entry. */
export function getArenaPillRound(
  rounds: readonly PkRound[],
  activeRoundId: string | null
): PkRound | null {
  const activeRound = rounds.find((round) => round.id === activeRoundId)
  if (activeRound && isLiveRound(activeRound)) return activeRound
  return rounds.find(isLiveRound) ?? null
}

export type PkEffortControl =
  | {
      kind: "select"
      configId: string
      options: readonly string[]
    }
  | { kind: "unsupported" }

/** Preserve each agent's advertised effort levels; never invent global ones. */
export function getEffortControl(
  options: readonly string[],
  configId: string | null
): PkEffortControl {
  if (!configId || options.length === 0) return { kind: "unsupported" }
  return { kind: "select", configId, options }
}
