export interface PkJudgeContestantRef {
  slot: number
  agentType: string
  label?: string | null
}

export interface PkJudgeScoreRef {
  slot?: number
  agentType: string
}

/**
 * Give legacy judge scores a stable contestant identity.
 *
 * New verdicts carry `slot`. Older verdicts only carried `agentType`, so two
 * models from the same agent were indistinguishable. For those rows, consume
 * matching contestant slots in their original arena order.
 */
export function assignJudgeScoreSlots<T extends PkJudgeScoreRef>(
  scores: readonly T[],
  contestants: readonly PkJudgeContestantRef[]
): Array<T & { slot?: number }> {
  const usedSlots = new Set<number>()

  return scores.map((score) => {
    const explicit =
      score.slot == null
        ? undefined
        : contestants.find(
            (contestant) =>
              contestant.slot === score.slot &&
              contestant.agentType === score.agentType &&
              !usedSlots.has(contestant.slot)
          )
    const matched =
      explicit ??
      contestants.find(
        (contestant) =>
          contestant.agentType === score.agentType &&
          !usedSlots.has(contestant.slot)
      )

    if (!matched) return { ...score }
    usedSlots.add(matched.slot)
    return { ...score, slot: matched.slot }
  })
}

export function contestantForJudgeScore(
  score: PkJudgeScoreRef,
  contestants: readonly PkJudgeContestantRef[]
): PkJudgeContestantRef | undefined {
  if (score.slot == null) return undefined
  return contestants.find((contestant) => contestant.slot === score.slot)
}
