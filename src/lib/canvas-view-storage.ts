"use client"

import type { AgentType } from "@/lib/types"

/**
 * Device-local memory of how the canvas was left: where the viewport sat, which
 * cards and regions were open, whether the map was showing, any conversation
 * started there but never sent, and the connection key a card inherited from
 * the draft that created it.
 *
 * The canvas is a full-page route, and `WorkbenchRoutePage` unmounts it whenever
 * the user goes back to the workspace — so without this, every visit reopens a
 * board the user has to re-navigate and re-expand. Persisting is what makes
 * "come back to where I was" true across route switches AND app restarts.
 *
 * Advisory, not authoritative, and deliberately not scoped per backend — the
 * same contract `last-active-context-storage.ts` has. The authoritative board is
 * always `canvas_node`. That un-scoping is the sharp edge of this file: a node
 * id is only meaningful WITHIN one database, and two databases behind one origin
 * hand out the same low `AUTOINCREMENT` ids to entirely different nodes. Most
 * entries degrade harmlessly under that (an unmatched expanded id opens nothing;
 * a restored draft whose folder is gone falls back to a card with no working
 * directory), so the caller's prune against the live board is all they need. An
 * entry that would carry BEHAVIOUR across — the connection key below — has to
 * name the card itself instead, and does.
 */

const VIEWPORT_KEY = "workspace:canvas-viewport"
const EXPANDED_CARDS_KEY = "workspace:canvas-expanded-cards"
const EXPANDED_REGIONS_KEY = "workspace:canvas-expanded-regions"
const DRAFTS_KEY = "workspace:canvas-drafts"
const MINIMAP_KEY = "workspace:canvas-minimap"
const SURFACE_KEYS_KEY = "workspace:canvas-surface-keys"

/** Mirrors ReactFlow's `Viewport`. Zoom is clamped to the same range the flow
 *  is configured with, so a corrupted entry can never strand the board at a
 *  zoom the user cannot recover from. */
export interface CanvasViewport {
  x: number
  y: number
  zoom: number
}

export const CANVAS_MIN_ZOOM = 0.1
export const CANVAS_MAX_ZOOM = 2

/** An unsent conversation card living only on this client's board. */
export interface CanvasDraftCard {
  id: string
  target: { folderId: number } | { chat: true }
  agentType: AgentType
  /** Chosen before the card has a row to store it on; carried into that row
   *  when the first message creates it (see `materializeDraft`). Absent or
   *  empty means no colour — the palette clears by re-picking. */
  color?: string
  x: number
  y: number
  width: number
  height: number
}

function readJson(key: string): unknown {
  if (typeof window === "undefined") return null
  try {
    const raw = localStorage.getItem(key)
    return raw ? (JSON.parse(raw) as unknown) : null
  } catch {
    return null
  }
}

function writeJson(key: string, value: unknown): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    /* ignore storage quota/permission failures */
  }
}

function remove(key: string): void {
  if (typeof window === "undefined") return
  try {
    localStorage.removeItem(key)
  } catch {
    /* ignore */
  }
}

function isFinitePosition(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v)
}

function isPositiveSize(v: unknown): v is number {
  return isFinitePosition(v) && v > 0
}

export function loadCanvasViewport(): CanvasViewport | null {
  const parsed = readJson(VIEWPORT_KEY)
  if (!parsed || typeof parsed !== "object") return null
  const obj = parsed as Record<string, unknown>
  if (!isFinitePosition(obj.x) || !isFinitePosition(obj.y)) return null
  if (!isFinitePosition(obj.zoom)) return null
  return {
    x: obj.x,
    y: obj.y,
    zoom: Math.min(Math.max(obj.zoom, CANVAS_MIN_ZOOM), CANVAS_MAX_ZOOM),
  }
}

export function saveCanvasViewport(viewport: CanvasViewport | null): void {
  if (!viewport) {
    remove(VIEWPORT_KEY)
    return
  }
  writeJson(VIEWPORT_KEY, viewport)
}

function loadIds(key: string): number[] {
  const parsed = readJson(key)
  if (!Array.isArray(parsed)) return []
  return parsed.filter((id): id is number => Number.isInteger(id))
}

/** Pinned cards that were expanded into a live conversation. */
export function loadCanvasExpandedCards(): number[] {
  return loadIds(EXPANDED_CARDS_KEY)
}

export function saveCanvasExpandedCards(ids: readonly number[]): void {
  writeJson(EXPANDED_CARDS_KEY, [...ids])
}

/** Regions whose "+N more" expander was open. */
export function loadCanvasExpandedRegions(): number[] {
  return loadIds(EXPANDED_REGIONS_KEY)
}

export function saveCanvasExpandedRegions(ids: readonly number[]): void {
  writeJson(EXPANDED_REGIONS_KEY, [...ids])
}

/**
 * Pinned card id → the ACP connection key its surface must keep using, for the
 * cards minted by a draft's first send: those inherit the DRAFT's key so the
 * send already in flight isn't left on a surface nobody renders (see
 * `materializeDraft`).
 *
 * Remembered for the same reason the expansions are. The key is only the
 * default (`canvas-node-<id>`) for a card that was never a draft, so a card
 * that recomputed it after a route switch would go looking for a connection
 * that lives under the draft's key — and find nothing, while the agent it
 * started keeps running under a key no surface claims any more.
 *
 * Each entry names the card it was written for, and the caller drops any whose
 * node no longer matches. Within one database an id would be enough —
 * `canvas_node.id` is `AUTOINCREMENT`, so ids are never reused, and a node's
 * `conversation_id` is fixed at creation — but this file is deliberately NOT
 * scoped per backend (see the header), and two databases behind one origin
 * hand out the same low ids to entirely different cards. `createdAt` is what
 * survives that: it identifies the node itself rather than its place in some
 * database's counter, so an entry written against another database resolves to
 * nothing, like every other entry here, instead of handing a card a stranger's
 * connection key.
 */
export interface CanvasSurfaceKey {
  /** The conversation the node was bound to when the key was inherited. */
  conversationId: number
  /** The node's `created_at`, as the card identity an id sequence can't give. */
  createdAt: string
  /** The ACP connection key that card must keep using. */
  key: string
}

export function loadCanvasSurfaceKeys(): Map<number, CanvasSurfaceKey> {
  const parsed = readJson(SURFACE_KEYS_KEY)
  const keys = new Map<number, CanvasSurfaceKey>()
  if (!Array.isArray(parsed)) return keys
  for (const raw of parsed) {
    if (!raw || typeof raw !== "object") continue
    const entry = raw as Record<string, unknown>
    if (!Number.isInteger(entry.nodeId)) continue
    if (!Number.isInteger(entry.conversationId)) continue
    if (typeof entry.createdAt !== "string" || entry.createdAt === "") continue
    if (typeof entry.key !== "string" || entry.key.length === 0) continue
    keys.set(entry.nodeId as number, {
      conversationId: entry.conversationId as number,
      createdAt: entry.createdAt,
      key: entry.key,
    })
  }
  return keys
}

export function saveCanvasSurfaceKeys(
  keys: ReadonlyMap<number, CanvasSurfaceKey>
): void {
  if (keys.size === 0) {
    remove(SURFACE_KEYS_KEY)
    return
  }
  writeJson(
    SURFACE_KEYS_KEY,
    [...keys].map(([nodeId, entry]) => ({ nodeId, ...entry }))
  )
}

/**
 * Whether the navigator map is showing above the viewport controls.
 *
 * Absent, corrupt, or anything but a literal `false` means shown: the map is
 * the default because it is how a board bigger than the window stays
 * comprehensible, and a user who has never touched the toggle should not have
 * to find it. Only an explicit dismissal is remembered.
 */
export function loadCanvasMinimapVisible(): boolean {
  return readJson(MINIMAP_KEY) !== false
}

export function saveCanvasMinimapVisible(visible: boolean): void {
  writeJson(MINIMAP_KEY, visible)
}

function parseDraft(value: unknown): CanvasDraftCard | null {
  if (!value || typeof value !== "object") return null
  const obj = value as Record<string, unknown>
  if (typeof obj.id !== "string" || obj.id.length === 0) return null
  // Any non-empty string: custom agents mint their own ids, and an agent that
  // has since been uninstalled is corrected by the card's own AgentSelector
  // fallback rather than by throwing the draft away here.
  if (typeof obj.agentType !== "string" || obj.agentType.trim() === "") {
    return null
  }
  if (!isFinitePosition(obj.x) || !isFinitePosition(obj.y)) return null
  // A card with no area is not a card. Zero or negative sizes are only
  // reachable from hand-edited or foreign storage, and they'd restore a window
  // that renders as an invisible sliver the user can neither read nor discard.
  if (!isPositiveSize(obj.width) || !isPositiveSize(obj.height)) return null
  const rawTarget = obj.target
  if (!rawTarget || typeof rawTarget !== "object") return null
  const target = rawTarget as Record<string, unknown>
  const parsedTarget: CanvasDraftCard["target"] | null =
    target.chat === true
      ? { chat: true }
      : Number.isInteger(target.folderId)
        ? { folderId: target.folderId as number }
        : null
  if (!parsedTarget) return null
  return {
    id: obj.id,
    target: parsedTarget,
    agentType: obj.agentType as AgentType,
    // Read leniently and never fatal: an unrecognised value is dropped by
    // `normalizeFolderThemeColor` at paint time anyway, and losing a whole
    // draft — the card AND the text typed into it — over a decoration would be
    // wildly out of proportion.
    ...(typeof obj.color === "string" ? { color: obj.color } : {}),
    x: obj.x,
    y: obj.y,
    width: obj.width,
    height: obj.height,
  }
}

/**
 * Unsent draft cards. Their ids are load-bearing: the composer's own text is
 * stored under a key derived from the draft id (`canvas-draft:canvas-draft-…`),
 * so restoring the id restores what the user had typed too.
 */
export function loadCanvasDrafts(): CanvasDraftCard[] {
  const parsed = readJson(DRAFTS_KEY)
  if (!Array.isArray(parsed)) return []
  const seen = new Set<string>()
  const drafts: CanvasDraftCard[] = []
  for (const raw of parsed) {
    const draft = parseDraft(raw)
    // Ids are the React keys AND the connection keys. A duplicate would mount
    // two cards claiming the same surface, so the first one wins.
    if (!draft || seen.has(draft.id)) continue
    seen.add(draft.id)
    drafts.push(draft)
  }
  return drafts
}

export function saveCanvasDrafts(drafts: readonly CanvasDraftCard[]): void {
  if (drafts.length === 0) {
    remove(DRAFTS_KEY)
    return
  }
  writeJson(DRAFTS_KEY, [...drafts])
}
