"use client"

import { useEffect, useRef } from "react"
import { PkLauncherDialog } from "@/components/pk/pk-launcher-dialog"
import { PkMinimizedPill } from "@/components/pk/pk-minimized-pill"
import { usePkRound, fetchUsage } from "@/hooks/use-pk-round"
import {
  usePkArenaStore,
  dbRoundToStoreRound,
  type PkRound,
} from "@/stores/pk-arena-store"
import { pkRoundList, updateConversationStatus } from "@/lib/api"
import { getPkConversationStatusRepairs } from "@/lib/pk-conversation-reconciliation"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"

/**
 * Arena mount point — renders global launch/minimized controls and drives the
 * orchestrator for rounds created by the launcher. Must live inside
 * `AcpConnectionsProvider` (the workspace layout provides it): the
 * orchestrator calls `connect`/`sendPrompt` and subscribes to `acp://event`.
 *
 * The launcher only writes the round into the store; this host picks it up,
 * so round creation works from anywhere (composer menu, future entries)
 * without prop-drilling.
 *
 * On mount, hydrates the store from the DB so finished rounds' scoreboards and
 * diffs remain viewable after a restart. The folder's path is needed to map
 * each DB round's folderId to its workingDir.
 */
export function PkArenaHost() {
  const { startRound } = usePkRound()
  const rounds = usePkArenaStore((s) => s.rounds)
  const hydrating = usePkArenaStore((s) => s.hydrating)
  const hydrateFromDb = usePkArenaStore((s) => s.hydrateFromDb)
  const folders = useAppWorkspaceStore((s) => s.allFolders)
  const conversations = useAppWorkspaceStore((s) => s.conversations)
  const conversationsLoading = useAppWorkspaceStore(
    (s) => s.conversationsLoading
  )
  const reconciledConversationIdsRef = useRef(new Set<number>())

  // Repair persisted PK conversation rows whose lifecycle no longer agrees
  // with the authoritative round. This covers both legacy judge rows and
  // contestant rows left live by an older cancellation path. The normal
  // conversation event updates the sidebar in place.
  useEffect(() => {
    for (const repair of getPkConversationStatusRepairs(
      rounds,
      conversations
    )) {
      if (reconciledConversationIdsRef.current.has(repair.conversationId)) {
        continue
      }
      reconciledConversationIdsRef.current.add(repair.conversationId)
      void updateConversationStatus(repair.conversationId, repair.status).catch(
        () => {
          reconciledConversationIdsRef.current.delete(repair.conversationId)
        }
      )
    }
  }, [conversations, rounds])

  // Hydrate each store instance once. Fast Refresh can replace the Zustand
  // store while preserving this host's React refs; keying the guard by the
  // store's rounds array lets the replacement hydrate again without issuing
  // duplicate requests during React Strict Mode's repeated effects.
  const hydrationSourceRef = useRef<readonly PkRound[] | null>(null)
  useEffect(() => {
    if (
      !hydrating ||
      hydrationSourceRef.current === rounds ||
      folders.length === 0 ||
      conversationsLoading
    ) {
      return
    }
    hydrationSourceRef.current = rounds
    void (async () => {
      try {
        const dbRounds = await pkRoundList()
        const storeRounds = dbRounds
          .map((info) => {
            const folder = folders.find((f) => f.id === info.folder_id)
            const workingDir = folder?.path ?? ""
            return dbRoundToStoreRound(info, workingDir, conversations)
          })
          .filter((r) => r.workingDir !== "")
        hydrateFromDb(storeRounds)
        // Backfill usage for finished contestants — usage is live-only in
        // the store (issue #4 / #16), so after a restart it's null. Fetch
        // it from the conversation turns for any contestant that has a
        // conversationId and is done/error/canceled.
        for (const round of storeRounds) {
          for (const c of round.contestants) {
            if (
              c.conversationId != null &&
              (c.status === "done" ||
                c.status === "error" ||
                c.status === "canceled")
            ) {
              const usage = await fetchUsage(c.conversationId)
              if (usage) {
                usePkArenaStore
                  .getState()
                  .updateContestant(round.id, c.slot, { usage })
              }
            }
          }
        }
      } catch {
        hydrateFromDb([])
      }
    })()
  }, [
    conversations,
    conversationsLoading,
    folders,
    hydrateFromDb,
    hydrating,
    rounds,
  ])

  // Drive any round that still has contestants in "preparing" — exactly the
  // state the launcher leaves behind. Restarted (interrupted) rounds come
  // back with settled statuses, so they are never re-driven.
  const drivenRef = useRef(new Set<string>())
  useEffect(() => {
    if (hydrating) return
    for (const round of rounds) {
      if (drivenRef.current.has(round.id)) continue
      if (!round.contestants.some((c) => c.status === "preparing")) continue
      drivenRef.current.add(round.id)
      void startRound(round)
    }
  }, [rounds, startRound, hydrating])

  return (
    <>
      <PkLauncherDialog />
      <PkMinimizedPill />
    </>
  )
}
