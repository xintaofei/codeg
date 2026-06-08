import type { FolderDetail, WorktreeResolution } from "@/lib/types"

/**
 * Outcome of deciding what selecting branch B should do, given where B is
 * checked out (`WorktreeResolution`) relative to the active folder.
 *
 * - `noop` — B is already checked out in the active folder.
 * - `navigateRegistered` — B is checked out at a path owned by an already-
 *   registered folder (a sibling worktree, or the root/main working tree);
 *   switch the working directory there, no checkout.
 * - `navigateExternal` — B is checked out in a worktree that isn't a registered
 *   folder yet; register it under `rootId`, then switch there.
 * - `checkoutInRoot` — B isn't checked out anywhere; switch to the root folder
 *   (if not already there) and `git checkout` B in the main working tree.
 */
export type BranchSwitchPlan =
  | { kind: "noop" }
  | { kind: "navigateRegistered"; folderId: number }
  | { kind: "navigateExternal"; path: string; rootId: number }
  | { kind: "checkoutInRoot"; rootFolder: FolderDetail }

/**
 * The root (main repo) folder for `activeFolder`: the folder its `parent_id`
 * points at, or itself when it is already a top-level folder. Falls back to the
 * active folder if the parent isn't in `allFolders`.
 */
export function resolveRootFolder(
  activeFolder: FolderDetail,
  allFolders: readonly FolderDetail[]
): FolderDetail {
  if (activeFolder.parent_id == null) return activeFolder
  return allFolders.find((f) => f.id === activeFolder.parent_id) ?? activeFolder
}

export function planBranchSwitch(args: {
  activeFolder: FolderDetail
  /** `null` for a remote selection (not resolved against local worktrees). */
  resolution: WorktreeResolution | null
  allFolders: readonly FolderDetail[]
  /** Remote selections always check out (track) in the root working tree —
   * never navigate to a same-short-name local worktree. */
  isRemote: boolean
}): BranchSwitchPlan {
  const { activeFolder, resolution, allFolders, isRemote } = args
  const root = resolveRootFolder(activeFolder, allFolders)

  // Remote selection, or a branch not checked out in any worktree → check it
  // out in the root working tree.
  if (isRemote || resolution == null || resolution.path == null) {
    return { kind: "checkoutInRoot", rootFolder: root }
  }

  // Already checked out somewhere.
  if (resolution.folder_id === activeFolder.id) {
    // …at the active folder itself — nothing to switch.
    return { kind: "noop" }
  }
  if (resolution.folder_id != null) {
    return { kind: "navigateRegistered", folderId: resolution.folder_id }
  }
  // …in a worktree directory that isn't a registered folder yet.
  return { kind: "navigateExternal", path: resolution.path, rootId: root.id }
}
