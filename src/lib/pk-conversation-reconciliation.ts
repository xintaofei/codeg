import type { PkRound } from "@/stores/pk-arena-store"
import type { DbConversationSummary } from "@/lib/types"

export interface PkConversationStatusRepair {
  conversationId: number
  status: "completed" | "cancelled"
}

/**
 * Derive repairs for PK-owned conversations whose persisted lifecycle no
 * longer agrees with their authoritative round state.
 *
 * Contestant conversations must be terminal once their round is canceled or
 * interrupted. Judge conversations have their own lifecycle because a judge
 * may legitimately continue after contestants are canceled.
 */
export function getPkConversationStatusRepairs(
  rounds: readonly PkRound[],
  conversations: readonly DbConversationSummary[]
): PkConversationStatusRepair[] {
  const roundsById = new Map(rounds.map((round) => [Number(round.id), round]))
  const repairs: PkConversationStatusRepair[] = []

  for (const conversation of conversations) {
    if (conversation.pk_round_id == null) continue
    const round = roundsById.get(conversation.pk_round_id)
    if (!round) continue

    const isJudge = conversation.title?.startsWith("PK Judge ·") ?? false
    if (isJudge) {
      const status =
        round.judgeStatus === "done"
          ? "completed"
          : round.judgeStatus === "error"
            ? "cancelled"
            : null
      if (status && conversation.status !== status) {
        repairs.push({ conversationId: conversation.id, status })
      }
      continue
    }

    if (
      conversation.status === "in_progress" &&
      (round.status === "canceled" || round.status === "interrupted")
    ) {
      repairs.push({
        conversationId: conversation.id,
        status: "cancelled",
      })
    }
  }

  return repairs
}
