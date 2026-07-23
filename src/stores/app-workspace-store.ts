import { create } from "zustand"
import { registerBackendScopedStoreReset } from "@/stores/backend-scoped-store-reset"
import {
  getFolder as apiGetFolder,
  listAllConversations,
  listAllFolderDetails,
  listOpenFolderDetails,
  openFolder as apiOpenFolder,
  openFolderById as apiOpenFolderById,
  openWorktreeFolder as apiOpenWorktreeFolder,
  removeFolderFromWorkspace as apiRemoveFolderFromWorkspace,
  reorderFolders as apiReorderFolders,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type {
  AgentStats,
  AgentType,
  DbConversationSummary,
  FolderDetail,
  GitHeadInfo,
} from "@/lib/types"

/**
 * Workspace-level shared state (folders, conversations, branches) as a Zustand
 * store. Components subscribe to the narrowest slice they render via
 * `useAppWorkspaceStore(selector)`; event bridges and callbacks read fresh
 * state through `useAppWorkspaceStore.getState()` instead of ref mirrors.
 *
 * Event wiring (side-channel subscriptions, branch polling, initial fetches)
 * stays in `AppWorkspaceProvider` — the store itself is transport-agnostic.
 */
export interface AppWorkspaceStoreState {
  folders: FolderDetail[]
  allFolders: FolderDetail[]
  foldersHydrated: boolean
  foldersLoading: boolean

  conversations: DbConversationSummary[]
  conversationsLoading: boolean
  conversationsError: string | null

  /**
   * Display branch name per folder (null when detached or non-repo).
   */
  branches: Map<number, string | null>

  /**
   * Full HEAD state per folder (repo-ness, detached, short sha). The poll keeps
   * this in sync alongside `branches`; consumers that only need the display
   * branch name keep reading `branches`. `BranchDropdown` reads this to tell a
   * detached HEAD apart from a non-git folder (issue #279).
   */
  gitHeads: Map<number, GitHeadInfo | null>

  /**
   * Derived from `conversations` on every write so subscribers get a stable
   * reference between conversation changes.
   */
  stats: AgentStats | null

  /**
   * Currently-active folder id as driven by the active tab.
   * TabProvider sets this; `useActiveFolder` / other consumers read it.
   */
  activeFolderId: number | null

  fetchFolders: () => Promise<void>
  refreshConversations: () => Promise<void>
  /**
   * Non-reactive by-id lookup for callbacks/effects. Render-time reads must
   * use a selector (`useAppWorkspaceStore((s) => s.allFolders.find(...))`)
   * instead, or they won't update when the folder changes.
   */
  getFolder: (id: number) => FolderDetail | undefined
  updateConversationLocal: (
    id: number,
    patch: Partial<
      Pick<DbConversationSummary, "status" | "title" | "pinned_at">
    >
  ) => void
  applyConversationUpsert: (summary: DbConversationSummary) => void
  applyConversationRemove: (id: number) => void
  getBranch: (folderId: number) => string | null | undefined
  setBranch: (folderId: number, branch: string | null) => void
  /** Equality-guarded merge of one folder's polled HEAD into branches/gitHeads. */
  applyGitHead: (folderId: number, head: GitHeadInfo) => void
  /**
   * Insert/replace a folder in local state, mirroring the backend's list
   * split: a `kind === "chat"` folder goes into `allFolders` only (matching
   * `list_open_folder_details`, which excludes chat folders from the
   * user-facing list), every other kind into both lists.
   */
  upsertFolder: (detail: FolderDetail) => void
  openFolder: (path: string) => Promise<FolderDetail>
  openWorktreeFolder: (
    path: string,
    sourceFolderId: number
  ) => Promise<FolderDetail>
  addFolderToWorkspaceById: (folderId: number) => Promise<FolderDetail>
  removeFolderFromWorkspace: (folderId: number) => Promise<void>
  reorderFolders: (ids: number[]) => Promise<void>
  refreshFolder: (id: number) => Promise<void>
  setActiveFolderId: (id: number | null) => void
}

function computeStats(conversations: DbConversationSummary[]): AgentStats {
  const byAgent = new Map<AgentType, number>()
  let totalMessages = 0

  for (const s of conversations) {
    byAgent.set(s.agent_type, (byAgent.get(s.agent_type) ?? 0) + 1)
    totalMessages += s.message_count
  }

  return {
    total_conversations: conversations.length,
    total_messages: totalMessages,
    by_agent: Array.from(byAgent.entries()).map(([agent_type, count]) => ({
      agent_type,
      conversation_count: count,
    })),
  }
}

/** Keep `stats` in lockstep with every `conversations` write. */
function withConversations(conversations: DbConversationSummary[]) {
  return {
    conversations,
    stats: conversations.length > 0 ? computeStats(conversations) : null,
  }
}

// Bound on the soft-delete tombstone set (see `deletedIds`). The eviction
// window — 512 deletions — far exceeds any realistic late/out-of-order event
// delay, so a row can never be resurrected in practice while memory stays
// bounded across a long-lived session.
const DELETED_TOMBSTONE_CAP = 512

// Tombstones for soft-deleted ids: a stale/out-of-order `upsert` that lands
// after a `deleted` (e.g. a concurrent rename racing a delete from another
// client) must not resurrect the row. Ids are DB autoincrement and never
// reused, so the tombstone is permanent; the set is FIFO-bounded.
const deletedIds = new Set<number>()

export const useAppWorkspaceStore = create<AppWorkspaceStoreState>()(
  (set, get) => ({
    folders: [],
    allFolders: [],
    foldersHydrated: false,
    foldersLoading: true,

    conversations: [],
    conversationsLoading: true,
    conversationsError: null,

    branches: new Map(),
    gitHeads: new Map(),
    stats: null,
    activeFolderId: null,

    fetchFolders: async () => {
      set({ foldersLoading: true })
      try {
        const [openList, allList] = await Promise.all([
          listOpenFolderDetails(),
          listAllFolderDetails(),
        ])
        const branches = new Map(get().branches)
        for (const f of allList) {
          if (!branches.has(f.id)) {
            branches.set(f.id, f.git_branch ?? null)
          }
        }
        set({ folders: openList, allFolders: allList, branches })
      } catch (err) {
        console.error("[AppWorkspace] fetchFolders failed:", err)
      } finally {
        set({ foldersLoading: false, foldersHydrated: true })
      }
    },

    refreshConversations: async () => {
      set({ conversationsLoading: true })
      try {
        const list = await listAllConversations()
        set({ ...withConversations(list), conversationsError: null })
      } catch (err) {
        set({ conversationsError: toErrorMessage(err) })
      } finally {
        set({ conversationsLoading: false })
      }
    },

    getFolder: (id) => get().allFolders.find((f) => f.id === id),

    updateConversationLocal: (id, patch) => {
      const prev = get().conversations
      const idx = prev.findIndex((c) => c.id === id)
      // Unknown id (e.g. a delegation-child status event reaching the global
      // channel) → leave state untouched so `stats` and sidebar consumers
      // don't re-render on a logical no-op.
      if (idx < 0) return
      const next = prev.slice()
      // A pin toggle is a view preference, not activity — mirror the backend
      // (`update_pin`) and leave `updated_at` untouched so an updated-sorted
      // folder doesn't briefly float the row. Status/title patches still bump.
      const bumpUpdatedAt = !("pinned_at" in patch)
      next[idx] = {
        ...next[idx],
        ...patch,
        ...(bumpUpdatedAt ? { updated_at: new Date().toISOString() } : {}),
      }
      // `stats` (computeStats) depends ONLY on the conversation count and each
      // row's agent_type/message_count. This path replaces a row IN PLACE (count
      // never changes), and the patch type is restricted to status/title/pinned_at
      // — none of which is a stat input — so a patch here can never move a stat.
      // Reuse the existing `stats` reference instead of recomputing O(n) and
      // minting a fresh object: otherwise every turn-boundary
      // `conversation_status_changed` tick (one per turn start/stop, per running
      // agent) would re-render every `stats` subscriber for a no-op. The
      // `statsAffecting` guard keeps this self-correcting if the patch type is
      // ever widened to include a stat input (it recomputes then); today it is
      // always false, i.e. always reuse.
      const statsAffecting = "message_count" in patch || "agent_type" in patch
      set(
        statsAffecting
          ? withConversations(next)
          : { conversations: next, stats: get().stats }
      )
    },

    // Insert-or-replace a conversation by id (create + field updates). Root-only:
    // delegation children (parent_id set) are not sidebar rows. New rows prepend
    // (most-recent-first); existing rows replace in place to keep their position.
    applyConversationUpsert: (summary) => {
      if (summary.parent_id != null) return
      if (deletedIds.has(summary.id)) return
      const prev = get().conversations
      const idx = prev.findIndex((c) => c.id === summary.id)
      if (idx < 0) {
        set(withConversations([summary, ...prev]))
        return
      }
      const next = prev.slice()
      next[idx] = summary
      set(withConversations(next))
    },

    // Remove a conversation by id. Idempotent: an unknown id leaves state
    // untouched (no re-render; keeps `stats` stable).
    applyConversationRemove: (id) => {
      deletedIds.add(id)
      if (deletedIds.size > DELETED_TOMBSTONE_CAP) {
        // FIFO eviction — Set preserves insertion order.
        const oldest = deletedIds.values().next().value
        if (oldest !== undefined) deletedIds.delete(oldest)
      }
      const prev = get().conversations
      const idx = prev.findIndex((c) => c.id === id)
      if (idx < 0) return
      const next = prev.slice()
      next.splice(idx, 1)
      set(withConversations(next))
    },

    getBranch: (folderId) => get().branches.get(folderId),

    setBranch: (folderId, branch) => {
      const next = new Map(get().branches)
      next.set(folderId, branch)
      set({ branches: next })
    },

    applyGitHead: (folderId, head) => {
      const { branches, gitHeads } = get()
      const patch: Partial<AppWorkspaceStoreState> = {}
      // `branches` stays the display branch name (null when detached or
      // non-repo) — unchanged contract for tab-bar/context-bar consumers.
      if (branches.get(folderId) !== head.branch) {
        const next = new Map(branches)
        next.set(folderId, head.branch)
        patch.branches = next
      }
      const existing = gitHeads.get(folderId)
      if (
        !existing ||
        existing.is_repo !== head.is_repo ||
        existing.branch !== head.branch ||
        existing.detached !== head.detached ||
        existing.short_sha !== head.short_sha
      ) {
        const next = new Map(gitHeads)
        next.set(folderId, head)
        patch.gitHeads = next
      }
      if (Object.keys(patch).length > 0) set(patch)
    },

    upsertFolder: (detail) => {
      const upsert = (prev: FolderDetail[]) => {
        const idx = prev.findIndex((f) => f.id === detail.id)
        if (idx >= 0) {
          const updated = [...prev]
          updated[idx] = detail
          return updated
        }
        return [...prev, detail]
      }
      const { folders, allFolders } = get()
      // Mirror the backend's list split: hidden chat folders are excluded from
      // `list_open_folder_details` (the user-facing `folders` list) but kept in
      // `list_all_folder_details` (`allFolders`, for by-id cwd / active-folder
      // lookups). Seeding a chat folder into `folders` would render a "Chat"
      // header row in the sidebar until the next refetch.
      set({
        ...(detail.kind !== "chat" ? { folders: upsert(folders) } : {}),
        allFolders: upsert(allFolders),
      })
    },

    openFolder: async (path) => {
      const detail = await apiOpenFolder(path)
      const { upsertFolder, setBranch, refreshConversations } = get()
      upsertFolder(detail)
      setBranch(detail.id, detail.git_branch ?? null)
      void refreshConversations()
      return detail
    },

    openWorktreeFolder: async (path, sourceFolderId) => {
      const detail = await apiOpenWorktreeFolder(path, sourceFolderId)
      const { upsertFolder, setBranch, refreshConversations } = get()
      upsertFolder(detail)
      setBranch(detail.id, detail.git_branch ?? null)
      void refreshConversations()
      return detail
    },

    addFolderToWorkspaceById: async (folderId) => {
      const detail = await apiOpenFolderById(folderId)
      const { upsertFolder, setBranch, refreshConversations } = get()
      upsertFolder(detail)
      setBranch(detail.id, detail.git_branch ?? null)
      void refreshConversations()
      return detail
    },

    removeFolderFromWorkspace: async (folderId) => {
      await apiRemoveFolderFromWorkspace(folderId)
      const { folders, branches, refreshConversations } = get()
      const patch: Partial<AppWorkspaceStoreState> = {
        folders: folders.filter((f) => f.id !== folderId),
      }
      if (branches.has(folderId)) {
        const next = new Map(branches)
        next.delete(folderId)
        patch.branches = next
      }
      set(patch)
      void refreshConversations()
    },

    reorderFolders: async (ids) => {
      const { folders: prevFolders, allFolders: prevAllFolders } = get()

      const reorderByIds = (prev: FolderDetail[]) => {
        const byId = new Map(prev.map((f) => [f.id, f]))
        const next: FolderDetail[] = []
        ids.forEach((id, idx) => {
          const folder = byId.get(id)
          if (folder) {
            next.push({ ...folder, sort_order: idx + 1 })
            byId.delete(id)
          }
        })
        // Keep folders not included in `ids` at the end, preserving relative order.
        for (const f of prev) {
          if (byId.has(f.id)) next.push(f)
        }
        return next
      }

      set({
        folders: reorderByIds(prevFolders),
        allFolders: reorderByIds(prevAllFolders),
      })

      try {
        await apiReorderFolders(ids)
      } catch (err) {
        set({ folders: prevFolders, allFolders: prevAllFolders })
        throw err
      }
    },

    refreshFolder: async (id) => {
      try {
        const detail = await apiGetFolder(id)
        const patchList = (prev: FolderDetail[]) => {
          const idx = prev.findIndex((f) => f.id === id)
          if (idx < 0) return prev
          const updated = [...prev]
          updated[idx] = detail
          return updated
        }
        const { folders, allFolders, branches } = get()
        const patch: Partial<AppWorkspaceStoreState> = {
          folders: patchList(folders),
          allFolders: patchList(allFolders),
        }
        // Only adopt the DB branch when the row actually carries one. A folder's
        // `git_branch` column is always null today — branch state is resolved by
        // git-head polling (`applyGitHead`) — so unconditionally writing it here
        // would clobber the polled branch name with null and make the selector
        // flash "no branch" until the next poll (up to 10s). Mirrors the same
        // null-guard the `folder://changed` handler already applies.
        if (detail.git_branch != null) {
          const nextBranches = new Map(branches)
          nextBranches.set(id, detail.git_branch)
          patch.branches = nextBranches
        }
        set(patch)
      } catch (err) {
        console.error("[AppWorkspace] refreshFolder failed:", err)
      }
    },

    setActiveFolderId: (id) => {
      if (get().activeFolderId === id) return
      set({ activeFolderId: id })
    },
  })
)

/**
 * Restore the pristine initial state (including tombstones). Used by tests, and
 * by the backend-scoped reset registry if a realm's backend identity ever
 * changes (an invariant-violating transition that does not occur today — see
 * `RemoteConnectionGate`). In normal operation the store lives for the window's
 * lifetime and is never reset.
 */
export function resetAppWorkspaceStore() {
  // NOTE: this clears state only; `fetchFolders` / `refreshConversations` have no
  // backend epoch, so a pre-reset in-flight fetch could re-commit stale data. Moot
  // today (the backend-identity guard never fires); a real in-place backend switch
  // would need per-store fetch epochs. See `RemoteConnectionGate`.
  deletedIds.clear()
  useAppWorkspaceStore.setState(useAppWorkspaceStore.getInitialState(), true)
}

// Reset this backend-scoped store on any (currently-unreachable) in-realm
// backend switch. See `backend-scoped-store-reset.ts`.
registerBackendScopedStoreReset(resetAppWorkspaceStore)
