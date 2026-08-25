"use client"

import { useEffect, useRef } from "react"
import { ConversationDetailPanel } from "@/components/conversations/conversation-detail-panel"
import { PkArenaHost } from "@/components/pk/pk-arena-host"
import { usePkArenaStore } from "@/stores/pk-arena-store"
import { useTabActions } from "@/stores/tab-store"

export default function WorkspacePage() {
  const rounds = usePkArenaStore((s) => s.rounds)
  const hydrating = usePkArenaStore((s) => s.hydrating)
  const { openPkRoundTab } = useTabActions()
  const openedRoundRef = useRef<string | null>(null)

  useEffect(() => {
    if (hydrating || typeof window === "undefined") return
    const roundId = new URLSearchParams(window.location.search).get("pkRoundId")
    if (!roundId || openedRoundRef.current === roundId) return
    const round = rounds.find((item) => item.id === roundId)
    if (!round) return
    openedRoundRef.current = roundId
    usePkArenaStore.getState().setActiveRound(round.id)
    openPkRoundTab(round.id, round.folderId, round.task)
  }, [hydrating, openPkRoundTab, rounds])

  return (
    <>
      <ConversationDetailPanel />
      {/* Inside the workspace layout's AcpConnectionsProvider — the arena
          orchestrator needs the connection actions and the acp://event
          subscription. */}
      <PkArenaHost />
    </>
  )
}
