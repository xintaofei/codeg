import { describe, expect, it } from "vitest"
import type { DbConversationSummary } from "@/lib/types"
import type { PkRound } from "@/stores/pk-arena-store"
import { getPkConversationStatusRepairs } from "./pk-conversation-reconciliation"

describe("getPkConversationStatusRepairs", () => {
  it("settles stale contestant rows after their PK round was canceled", () => {
    const round = {
      id: "4",
      status: "canceled",
      judgeStatus: "idle",
    } as PkRound
    const conversations = [89, 90, 91, 92, 93, 94].map(
      (id) =>
        ({
          id,
          pk_round_id: 4,
          title: "PK · 用单个 HTML 文件写一个俄罗斯方块游戏",
          status: "in_progress",
        }) as DbConversationSummary
    )

    expect(getPkConversationStatusRepairs([round], conversations)).toEqual(
      conversations.map(({ id }) => ({
        conversationId: id,
        status: "cancelled",
      }))
    )
  })

  it("does not cancel a judge that may still run after contestant cancellation", () => {
    const round = {
      id: "4",
      status: "canceled",
      judgeStatus: "running",
    } as PkRound
    const judge = {
      id: 95,
      pk_round_id: 4,
      title: "PK Judge · task",
      status: "in_progress",
    } as DbConversationSummary

    expect(getPkConversationStatusRepairs([round], [judge])).toEqual([])
  })
})
