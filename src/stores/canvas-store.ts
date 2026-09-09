import { create } from "zustand"
import { canvasListNodes } from "@/lib/api"
import type {
  CanvasChange,
  CanvasNode,
  CanvasNodeMovePayload,
  CanvasSnapshot,
} from "@/lib/types"
import { registerBackendScopedStoreReset } from "@/stores/backend-scoped-store-reset"

/**
 * Canvas node cache + the revision protocol that keeps it convergent.
 *
 * The backend assigns every committed mutation a dense revision and broadcasts
 * exactly one `canvas://changed` event per bump; there is no origin field, so
 * the SAME rules apply to our own mutations and everyone else's:
 *
 *  - The event stream is the only channel that advances `lastRevision`:
 *    `revision <= lastRevision` is stale (drop), `== lastRevision + 1` applies,
 *    `> lastRevision + 1` is a gap — drop and refetch the snapshot.
 *  - A command response NEVER advances `lastRevision`. Its value is applied as
 *    an optimistic confirmation only while its revision is still ahead of
 *    `lastRevision` (i.e. its own event has not arrived yet); otherwise the
 *    event already did the work and the response is dropped. Both arrival
 *    orders converge — this is why no own-origin special case is needed.
 *  - A snapshot is accepted only at `revision >= lastRevision`, and acceptance
 *    is a whole-set replace. Events lost while the WS was down surface as a gap
 *    or are covered by the reconnect refetch.
 */
interface CanvasStoreState {
  /** All persisted canvas nodes, keyed by DB id. Replaced (never mutated) on
   *  every apply so selectors and the derive layer can compare by reference. */
  nodes: ReadonlyMap<number, CanvasNode>
  /** Highest event/snapshot revision applied. 0 until the first snapshot. */
  lastRevision: number
  /** First snapshot landed — the canvas can render (vs. initial spinner). */
  hydrated: boolean
  handleCanvasChanged: (change: CanvasChange) => void
  acceptSnapshot: (snapshot: CanvasSnapshot) => void
  /**
   * Apply a mutation response's payload. `mutate` runs against a copy of the
   * node map only when `revision > lastRevision` (the matching event is still
   * on its way); a response arriving after its event is dropped whole.
   */
  applyResponse: (
    revision: number,
    mutate: (nodes: Map<number, CanvasNode>) => void
  ) => void
  /** Fetch a fresh snapshot (initial hydrate, gap repair, WS reconnect).
   *  Coalesces: at most one fetch in flight, callers share its outcome. */
  refetch: () => Promise<void>
  reset: () => void
}

function applyChange(
  nodes: Map<number, CanvasNode>,
  change: CanvasChange
): void {
  switch (change.kind) {
    case "upsert":
      nodes.set(change.node.id, change.node)
      break
    case "moved":
      for (const move of change.moves) {
        const existing = nodes.get(move.id)
        if (existing) nodes.set(move.id, { ...existing, x: move.x, y: move.y })
      }
      break
    case "deleted":
      nodes.delete(change.id)
      break
    case "detached": {
      // One transaction server-side: membership removal (custom regions only)
      // plus the new pin. The region's new member list is not in the payload,
      // so scrub it here — retain() is idempotent, matching the event contract.
      const convId = change.node.conversation_id
      if (change.removed_from != null && convId != null) {
        const region = nodes.get(change.removed_from)
        if (region) {
          nodes.set(change.removed_from, {
            ...region,
            member_ids: region.member_ids.filter((m) => m !== convId),
          })
        }
      }
      nodes.set(change.node.id, change.node)
      break
    }
    case "grouped":
      // Delete before insert: the absorbed pins and the region that swallowed
      // them committed together, and the new region's id can never collide with
      // one of them (it was just allocated), so the order is only about reading
      // as one step. Both halves are idempotent.
      for (const id of change.deleted_ids) nodes.delete(id)
      nodes.set(change.node.id, change.node)
      break
    case "pruned":
      for (const id of change.deleted_ids) nodes.delete(id)
      for (const node of change.updated) nodes.set(node.id, node)
      break
  }
}

let refetchInFlight: Promise<void> | null = null
/** Highest revision seen on any gapped (dropped) event. A refetch that lands
 *  BELOW this — the snapshot was read before that mutation committed — hasn't
 *  actually repaired the gap, so another round is scheduled. Cleared by reset. */
let gapHighWater = 0
/** Pending failure-retry timer, so a fetch error during gap repair still
 *  self-heals on a quiet canvas (no later event to re-trigger it). */
let retryTimer: ReturnType<typeof setTimeout> | null = null
const RETRY_DELAY_MS = 3000
/** Fetch generation, bumped by reset(): a snapshot from a pre-reset fetch must
 *  neither write into the new scope nor clobber its dedup handle. */
let fetchEpoch = 0

export const useCanvasStore = create<CanvasStoreState>((set, get) => ({
  nodes: new Map(),
  lastRevision: 0,
  hydrated: false,

  handleCanvasChanged: (change) => {
    const { lastRevision, nodes } = get()
    if (change.revision <= lastRevision) return
    if (change.revision > lastRevision + 1) {
      // Gap: an event was lost (WS drop, subscribe race). The change is NOT
      // applied — payloads are only guaranteed coherent against the exact
      // predecessor state — the snapshot repairs everything at once. Record
      // the revision so a refetch already in flight (whose snapshot may
      // predate this mutation) knows it still has work to do.
      gapHighWater = Math.max(gapHighWater, change.revision)
      void get().refetch()
      return
    }
    const next = new Map(nodes)
    applyChange(next, change)
    set({ nodes: next, lastRevision: change.revision })
  },

  acceptSnapshot: (snapshot) => {
    if (snapshot.revision < get().lastRevision) return
    set({
      nodes: new Map(snapshot.nodes.map((n) => [n.id, n])),
      lastRevision: snapshot.revision,
      hydrated: true,
    })
  },

  applyResponse: (revision, mutate) => {
    if (revision <= get().lastRevision) return
    const next = new Map(get().nodes)
    mutate(next)
    set({ nodes: next })
  },

  refetch: () => {
    if (refetchInFlight) return refetchInFlight
    if (retryTimer) {
      clearTimeout(retryTimer)
      retryTimer = null
    }
    const epoch = fetchEpoch
    refetchInFlight = canvasListNodes()
      .then((snapshot) => {
        // A reset raced this fetch: the result belongs to the previous scope
        // — don't write it, and don't touch the new scope's bookkeeping.
        if (epoch !== fetchEpoch) return
        get().acceptSnapshot(snapshot)
        refetchInFlight = null
        // The snapshot predates a gapped event we already saw: go again
        // (bounded — the backend revision is monotonic, so this converges).
        if (gapHighWater > get().lastRevision) void get().refetch()
      })
      .catch((e) => {
        if (epoch !== fetchEpoch) return
        console.error("[canvas] snapshot fetch failed:", e)
        refetchInFlight = null
        // Quiet-canvas self-heal: no later event may ever re-trigger this.
        retryTimer = setTimeout(() => {
          retryTimer = null
          void get().refetch()
        }, RETRY_DELAY_MS)
      })
    return refetchInFlight
  },

  reset: () => {
    // In-flight fetches are not aborted, but the epoch bump strands their
    // results: they can neither write into the new scope nor clobber its
    // dedup handle (see the epoch guard in refetch).
    fetchEpoch++
    refetchInFlight = null
    gapHighWater = 0
    if (retryTimer) {
      clearTimeout(retryTimer)
      retryTimer = null
    }
    // Whatever is still in the air belongs to the old scope, and its ids mean
    // nothing in the new one. The epoch bump is what makes dropping the table
    // safe: a stranded write's handle no longer matches, so its completion
    // cannot decrement a reused id in the scope that replaced it.
    contentWriteEpoch++
    contentWrites.clear()
    set({ nodes: new Map(), lastRevision: 0, hydrated: false })
  },
}))

/**
 * Notes whose text has been sent but not yet acknowledged, by COUNT.
 *
 * A note commits its text on the way out of the editor and `nodes` only learns
 * it when the backend answers, so for one round-trip the cached row still reads
 * EMPTY. The delete confirmations read that row, so without this, text typed
 * into a fresh note and deleted right after (Escape, then Delete) looks like an
 * empty note and is taken without asking.
 *
 * A COUNT, not a set: a note can be saved twice in quick succession (edit,
 * blur, edit again, blur) and the first response must not clear a mark the
 * second still needs.
 *
 * Module state rather than component state, for two reasons. It outlives the
 * board — a save fired from a note's unmount is exactly the one still in the
 * air when the route switches, and a fresh ref on remount would not know about
 * it. And it is not rendered, so putting it in the store proper would wake
 * every subscriber for something none of them draw.
 */
const contentWrites = new Map<number, number>()
let contentWriteEpoch = 0

/**
 * Mark a note's text as sent, and return the function that un-marks it. The
 * caller cannot un-mark without having marked, and the handle carries the epoch
 * it was minted in — a write still in the air across a [`reset`] belongs to the
 * old scope, and node ids are per-scope, so letting its completion decrement a
 * REUSED id would clear a mark the new scope still needs. Same reasoning as
 * `fetchEpoch`, which strands stale snapshot fetches the same way.
 */
export function beginContentWrite(nodeId: number): () => void {
  const epoch = contentWriteEpoch
  contentWrites.set(nodeId, (contentWrites.get(nodeId) ?? 0) + 1)
  return () => {
    if (epoch !== contentWriteEpoch) return
    const left = (contentWrites.get(nodeId) ?? 0) - 1
    // Never below zero: an end without a live begin (already cleared) must not
    // wedge the node at a negative count that can never read as "settled".
    if (left > 0) contentWrites.set(nodeId, left)
    else contentWrites.delete(nodeId)
  }
}

export function hasContentWriteInFlight(nodeId: number): boolean {
  return (contentWrites.get(nodeId) ?? 0) > 0
}

/** Event-shaped move apply for optimistic drag confirmation, shared with the
 *  view's `applyResponse` callbacks so both paths write identical state. */
export function applyMovesTo(
  nodes: Map<number, CanvasNode>,
  moves: CanvasNodeMovePayload[]
): void {
  for (const move of moves) {
    const existing = nodes.get(move.id)
    if (existing) nodes.set(move.id, { ...existing, x: move.x, y: move.y })
  }
}

registerBackendScopedStoreReset(() => useCanvasStore.getState().reset())
