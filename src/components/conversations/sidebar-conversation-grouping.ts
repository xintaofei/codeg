import type { DbConversationSummary } from "@/lib/types"
import type {
  SidebarSortMode,
  SidebarSectionOrder,
} from "@/lib/sidebar-view-mode-storage"

export function parseTimestamp(value: string): number {
  const timestamp = Date.parse(value)
  return Number.isNaN(timestamp) ? 0 : timestamp
}

export function compareByUpdatedAtDesc(
  left: DbConversationSummary,
  right: DbConversationSummary
): number {
  const updatedDiff =
    parseTimestamp(right.updated_at) - parseTimestamp(left.updated_at)
  if (updatedDiff !== 0) return updatedDiff

  const createdDiff =
    parseTimestamp(right.created_at) - parseTimestamp(left.created_at)
  if (createdDiff !== 0) return createdDiff

  return right.id - left.id
}

export function compareByCreatedAtDesc(
  left: DbConversationSummary,
  right: DbConversationSummary
): number {
  const createdDiff =
    parseTimestamp(right.created_at) - parseTimestamp(left.created_at)
  if (createdDiff !== 0) return createdDiff

  const updatedDiff =
    parseTimestamp(right.updated_at) - parseTimestamp(left.updated_at)
  if (updatedDiff !== 0) return updatedDiff

  return right.id - left.id
}

/**
 * Newest-created first, id as a stable tie-break — matching the backend
 * `list_children` ORDER BY created_at DESC, id DESC so a merged/inserted child
 * lands where a refetch would put it. Sub-sessions render newest-on-top like the
 * root list, so a freshly-spawned sub-agent surfaces right under its parent.
 * Deliberately created_at + id only (no `updated_at` middle key like
 * {@link compareByCreatedAtDesc}) to mirror the SQL order the raw fetch snapshot
 * is trusted to already be in. Parity holds at millisecond + id resolution:
 * `parseTimestamp` (Date.parse) truncates to ms, so two children created in the
 * same millisecond fall to the id tie-break here while the backend orders them
 * by full-precision `created_at` — harmless because ids increase with creation
 * time, so both still agree on newest-first.
 */
export function compareByChildCreatedAtDesc(
  left: DbConversationSummary,
  right: DbConversationSummary
): number {
  const createdDiff =
    parseTimestamp(right.created_at) - parseTimestamp(left.created_at)
  if (createdDiff !== 0) return createdDiff
  return right.id - left.id
}

/**
 * Most-recently-pinned first. Only ever applied to rows with a non-null
 * `pinned_at` (the pinned bucket), so the empty-string fallback is just a guard.
 */
export function compareByPinnedAtDesc(
  left: DbConversationSummary,
  right: DbConversationSummary
): number {
  const diff =
    parseTimestamp(right.pinned_at ?? "") - parseTimestamp(left.pinned_at ?? "")
  if (diff !== 0) return diff
  return right.id - left.id
}

/**
 * Relative time label (e.g. "5m", "3h", "2d"). `now` is passed in rather than
 * read from `Date.now()` so a whole render tick shares one value: every
 * unchanged row then produces an identical label string and the card `memo`
 * stays hit. The list refreshes `now` once a minute (see
 * `SidebarConversationList`), bounding label staleness without making a single
 * status event re-render every card.
 */
export function formatRelative(iso: string, now: number): string {
  const ts = parseTimestamp(iso)
  if (!ts) return ""
  const diff = Math.max(0, now - ts)
  const m = Math.floor(diff / 60000)
  if (m < 1) return "now"
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h`
  const d = Math.floor(h / 24)
  if (d < 30) return `${d}d`
  const mo = Math.floor(d / 30)
  if (mo < 12) return `${mo}mo`
  const y = Math.floor(mo / 12)
  return `${y}y`
}

function arraysShallowEqual<T>(a: readonly T[], b: readonly T[]): boolean {
  if (a === b) return true
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false
  }
  return true
}

/**
 * Return `prev` when `next` has identical string membership, else `next`.
 *
 * `tabs` is rebuilt (new array) on every `conversations` change (tab-context
 * re-derives titles/status), so `openTabKeys` recomputes every status event.
 * Without this reuse the freshly-built Set would be a new reference each time
 * and would defeat the `FolderGroupItem` memo for *every* folder. Content
 * equality keeps the reference stable when the open-tab set is actually
 * unchanged.
 */
export function reuseSet(prev: Set<string>, next: Set<string>): Set<string> {
  if (prev === next) return prev
  if (prev.size !== next.size) return next
  for (const key of next) {
    if (!prev.has(key)) return next
  }
  return prev
}

export interface SelectedConversationRef {
  id: number
  agentType: string
}

/**
 * Return `prev` when it denotes the same conversation as `next`, else `next`.
 * Same motivation as {@link reuseSet}: keeps `selectedConversation` reference
 * stable across the `tabs` churn so unaffected folders stay memoized.
 */
export function reuseSelected(
  prev: SelectedConversationRef | null,
  next: SelectedConversationRef | null
): SelectedConversationRef | null {
  if (
    prev &&
    next &&
    prev.id === next.id &&
    prev.agentType === next.agentType
  ) {
    return prev
  }
  return next
}

/**
 * Group conversations by folder, sorting each bucket, while reusing the
 * previous render's bucket array whenever a folder's sorted membership is
 * referentially unchanged.
 *
 * Reference stability is the whole point: a single `conversation_status_changed`
 * event replaces exactly one summary object (slice + spread in
 * `updateConversationLocal`), so only the touched folder's bucket fails the
 * shallow-equality check and gets a fresh array. Every sibling folder keeps its
 * old array reference, letting a memoized `FolderGroupItem` bail out — and
 * inside the one folder that did change, every unchanged summary keeps its
 * object identity so the card `memo` still bails out for all but the one
 * affected row.
 *
 * `prev` is the map returned by the last call (the caller threads it via a ref).
 *
 * `childToParent` (optional) merges worktree child folders into their parent: a
 * conversation whose `folder_id` is a key is bucketed under the mapped parent
 * id instead, so the parent group renders the main repo's and all its worktrees'
 * conversations together (sorted as one bucket). The conversation objects
 * themselves are untouched — only the grouping key is redirected, never
 * `folder_id` — so per-conversation cwd resolution stays correct.
 */
export function groupByFolderWithReuse(
  filtered: readonly DbConversationSummary[],
  sortMode: SidebarSortMode,
  prev: Map<number, DbConversationSummary[]>,
  childToParent?: ReadonlyMap<number, number>
): Map<number, DbConversationSummary[]> {
  const next = new Map<number, DbConversationSummary[]>()
  for (const conv of filtered) {
    const groupId = childToParent?.get(conv.folder_id) ?? conv.folder_id
    const list = next.get(groupId)
    if (list) list.push(conv)
    else next.set(groupId, [conv])
  }

  const comparator =
    sortMode === "updated" ? compareByUpdatedAtDesc : compareByCreatedAtDesc
  for (const [folderId, list] of next) {
    list.sort(comparator)
    const prevList = prev.get(folderId)
    // Replacing an existing key's value mid-iteration is safe (we never add or
    // remove keys here).
    if (prevList && arraysShallowEqual(prevList, list)) {
      next.set(folderId, prevList)
    }
  }
  return next
}

/**
 * Select the pinned conversations (those with a non-null `pinned_at`), sorted
 * most-recently-pinned first, reusing the previous array reference when the
 * sorted membership is referentially unchanged.
 *
 * Same reference-stability motivation as {@link groupByFolderWithReuse}: a
 * single status event replaces exactly one summary object, so this would
 * otherwise build a fresh array each tick and defeat the Pinned section's memo.
 * Built from the FULL `conversations` list (never the completed-filtered one): a
 * pinned conversation stays in the Pinned section even when "Show completed" is
 * off — pinning is an explicit "keep this handy" override of that filter.
 *
 * `prev` is the array returned by the last call (the caller threads it via a
 * ref).
 */
export function selectPinnedWithReuse(
  conversations: readonly DbConversationSummary[],
  prev: DbConversationSummary[]
): DbConversationSummary[] {
  const next: DbConversationSummary[] = []
  for (const conv of conversations) {
    if (conv.pinned_at != null) next.push(conv)
  }
  next.sort(compareByPinnedAtDesc)
  return arraysShallowEqual(prev, next) ? prev : next
}

/**
 * Select the folderless "chat mode" conversations (`kind === "chat"`) for the
 * flat "Chat" sidebar section. Sorted most-recently-updated first, with
 * reference reuse (same motivation as {@link selectPinnedWithReuse}).
 *
 * Excludes pinned conversations (they surface in the Pinned section, an explicit
 * override) and — unless `showCompleted` — completed ones, matching how
 * `folderConversations` is filtered for the folders section.
 *
 * `prev` is the array returned last call (threaded via a ref by the caller).
 */
export function selectChatConversationsWithReuse(
  conversations: readonly DbConversationSummary[],
  showCompleted: boolean,
  prev: DbConversationSummary[]
): DbConversationSummary[] {
  const next: DbConversationSummary[] = []
  for (const conv of conversations) {
    if (conv.pinned_at != null) continue
    if (conv.kind !== "chat") continue
    if (!showCompleted && conv.status === "completed") continue
    next.push(conv)
  }
  next.sort(compareByUpdatedAtDesc)
  return arraysShallowEqual(prev, next) ? prev : next
}

// ── Flat row model (Phase 2 virtualization) ─────────────────────────────────
// The sidebar tree (folders → their conversation rows) is flattened into a
// single linear array so it can be windowed by `virtua`. Each visible folder
// contributes one header row, and — when expanded — either one empty-hint row
// or its sorted conversation rows.

export interface FolderHeaderRow {
  kind: "folder"
  folderId: number
}

export interface ConversationRow {
  kind: "conversation"
  /**
   * The summary object reference is passed through untouched (never copied), so
   * a status event that replaces exactly one summary keeps every other row's
   * `conversation` identity — the linchpin that lets the card `memo` bail out
   * through the virtualized render. See {@link groupByFolderWithReuse}.
   */
  conversation: DbConversationSummary
  /**
   * Nesting depth in the delegation tree: 0 for a root / pinned / chat
   * conversation, 1 for its direct delegation children, 2 for grandchildren,
   * etc. Drives the card's per-level indent (a pure function of this number).
   */
  depth: number
}

export interface EmptyHintRow {
  kind: "empty"
  folderId: number
  /**
   * Total (unfiltered, pinned-excluded) conversation count for this folder, used
   * by the renderer to pick between the "empty folder" and "no unfinished
   * conversations" hints.
   */
  totalConversationCount: number
}

/**
 * The single empty-state hint shown under an expanded but empty "Chat" section
 * ("No chats yet"). Unlike {@link EmptyHintRow} it is folderless — chat
 * conversations are a flat list — so it carries no folder id and renders with a
 * flat (non-rail) indent.
 */
export interface ChatsEmptyRow {
  kind: "chats-empty"
}

/**
 * The single empty-state hint shown under an expanded but empty "Folders"
 * section ("No folders open"). Like {@link ChatsEmptyRow} it is folderless — it
 * stands in for the whole (empty) folder list rather than one folder — so it
 * carries no folder id and renders with a flat (non-rail) indent. Distinct from
 * {@link EmptyHintRow}, which is the per-folder "this folder is empty" hint.
 */
export interface FoldersEmptyRow {
  kind: "folders-empty"
}

/**
 * A collapsible section heading. Three exist: "pinned" (above the folders, shown
 * only when there are pinned conversations), "folders" (wraps the whole folder
 * list), and "chats" (below the folders, a flat list of folderless chat-mode
 * conversations, shown only when there are any). All live in the same flat row
 * array so the single Virtualizer windows them like any other row — there is no
 * separate, un-virtualized list.
 */
export interface SectionHeaderRow {
  kind: "section"
  section: "pinned" | "folders" | "chats"
  expanded: boolean
  /** Pinned count, folder count, or chat-conversation count — shown beside the title. */
  count: number
}

/**
 * A transient placeholder at the child indent, shown while a conversation's
 * delegation children are being lazily fetched (between expand and the
 * `listChildConversations` response). Replaced by the real child rows once
 * loaded, or by nothing if the parent turns out to have no (live) children.
 */
export interface SubsessionLoadingRow {
  kind: "subsession-loading"
  parentId: number
  depth: number
}

export type SidebarRow =
  | SectionHeaderRow
  | FolderHeaderRow
  | ConversationRow
  | EmptyHintRow
  | ChatsEmptyRow
  | FoldersEmptyRow
  | SubsessionLoadingRow

const MAX_RENDER_DEPTH = 32

// Shared empty defaults so callers (and existing tests) that don't track
// sub-session expansion can omit the two params without allocating per call —
// the row output is then identical to the pre-subtree flat model.
const EMPTY_EXPANDED: ReadonlySet<number> = new Set()
const EMPTY_CHILDREN: ReadonlyMap<number, readonly DbConversationSummary[]> =
  new Map()

/**
 * Merge a freshly-fetched children snapshot with child summaries already applied
 * from live events (buffered into the lazy-load placeholder while the fetch was
 * in flight). Keyed by id with the live event winning, so a child created or
 * updated after the fetch's DB query is never lost — closing the lazy-load
 * lost-update race. Sorted created_at-descending (newest first) to match
 * `list_children`.
 */
export function mergeChildrenById(
  snapshot: readonly DbConversationSummary[],
  buffered: readonly DbConversationSummary[]
): DbConversationSummary[] {
  const byId = new Map<number, DbConversationSummary>()
  for (const c of snapshot) byId.set(c.id, c)
  for (const b of buffered) byId.set(b.id, b)
  return [...byId.values()].sort(compareByChildCreatedAtDesc)
}

/**
 * Push a conversation row and — when it is expanded and its delegation children
 * are cached — recursively push its subtree (depth+1 per level). Bounded by
 * `conversationExpanded` (a finite, user-controlled set) and by what is actually
 * in `childrenByParent` (only fetched parents descend), so it always terminates;
 * `MAX_RENDER_DEPTH` is defense-in-depth against pathological data. The
 * `parent_id` chain is a tree by construction (set once at insert, never
 * updated), so cycles cannot occur.
 *
 * Child summaries are pushed by reference (never copied), exactly like root
 * rows, so a status event replacing one child keeps every sibling's identity and
 * the card `memo` still bails out through the virtualized render.
 */
function pushConversationRow(
  rows: SidebarRow[],
  conversation: DbConversationSummary,
  depth: number,
  conversationExpanded: ReadonlySet<number>,
  childrenByParent: ReadonlyMap<number, readonly DbConversationSummary[]>,
  childrenLoading: ReadonlySet<number>
): void {
  rows.push({ kind: "conversation", conversation, depth })
  if (
    depth >= MAX_RENDER_DEPTH ||
    conversation.child_count <= 0 ||
    !conversationExpanded.has(conversation.id)
  ) {
    return
  }
  const kids = childrenByParent.get(conversation.id)
  // Spinner while: not fetched yet (undefined), OR an in-flight placeholder
  // (empty array still loading — events may not have buffered any child yet).
  if (
    kids === undefined ||
    (kids.length === 0 && childrenLoading.has(conversation.id))
  ) {
    rows.push({
      kind: "subsession-loading",
      parentId: conversation.id,
      depth: depth + 1,
    })
    return
  }
  // Loaded (possibly merged with mid-flight events). An empty array that is NOT
  // loading means a stale child_count → the `for` renders nothing, self-healing.
  for (const kid of kids) {
    pushConversationRow(
      rows,
      kid,
      depth + 1,
      conversationExpanded,
      childrenByParent,
      childrenLoading
    )
  }
}

/**
 * Flatten the (optional) pinned section and the folders section into a single
 * linear row list for windowing by the one Virtualizer — pinned conversations
 * are ordinary conversation rows in the SAME array, never a separate list.
 *
 * Pure and deliberately **does not take `now`**: the per-minute `now` tick that
 * refreshes relative time labels must not rebuild this array (that would defeat
 * the Phase 1 memo chain). `timeLabel` stays computed at the row renderer from
 * the shared `now` against the row's `conversation`.
 *
 * Structure (top to bottom): the "Pinned" section (when present) is always
 * first; the "Folders" and "Chat" sections follow in the order set by
 * `sectionOrder` (default `folders-first` = Folders then Chat; `chats-first`
 * swaps them). Each section's own presence/expansion rules are unchanged by
 * that order:
 * - The "Pinned" section header + its conversations appear only when `pinned`
 *   is non-empty, and its rows only when `pinnedExpanded`.
 * - The "Folders" section header ALWAYS appears (like "Chat"), so the section
 *   stays a permanent entry point — its Open-folder / Clone / Import actions stay
 *   reachable even with nothing open. Its rows appear only when `foldersExpanded`:
 *   when expanded with no open folders it contributes a single `folders-empty`
 *   hint row; otherwise its folder rows follow `orderedFolderIds`: a collapsed
 *   folder contributes only its header; an expanded empty folder contributes
 *   header + one (per-folder) empty-hint row; an expanded non-empty folder
 *   contributes header + its (already sorted) bucket. `byFolder` /
 *   `folderTotalCounts` exclude pinned conversations (they live in the Pinned
 *   section), so a folder whose only conversations are pinned reads as empty. The
 *   fully-empty initial workspace (no folders AND no conversations) never reaches
 *   buildRows — the list renders its dedicated open-folder call-to-action there.
 * - The "Chat" section header ALWAYS appears (even with zero chat
 *   conversations), so the section is a permanent entry point — its New-chat
 *   affordance and an empty hint stay reachable. When expanded and empty it
 *   contributes a single `chats-empty` hint row; otherwise its (flat, folderless)
 *   conversation rows. Pinned chat conversations live in the Pinned section, so
 *   they are excluded from `chatConversations`.
 */
export function buildRows(args: {
  pinned: readonly DbConversationSummary[]
  pinnedExpanded: boolean
  orderedFolderIds: readonly number[]
  byFolder: Map<number, DbConversationSummary[]>
  folderExpanded: Record<number, boolean>
  folderTotalCounts: Map<number, number>
  foldersExpanded: boolean
  chatConversations: readonly DbConversationSummary[]
  chatsExpanded: boolean
  /** Vertical order of the Folders and Chat sections. The Pinned section (when
   *  present) always stays on top regardless. Optional — omitted (e.g. in
   *  tests) defaults to `folders-first`, the historical layout. */
  sectionOrder?: SidebarSectionOrder
  /** Ids whose delegation subtree is open. A conversation row with
   *  `child_count > 0` and id in this set recurses into its cached children.
   *  Optional — omitted (e.g. in tests) means nothing is expanded. */
  conversationExpanded?: ReadonlySet<number>
  /** Lazily-fetched direct children keyed by parent id. Absent key = not yet
   *  fetched (renders a loading row when expanded); empty array = no live
   *  children (renders nothing — self-heals stale `child_count`). Optional. */
  childrenByParent?: ReadonlyMap<number, readonly DbConversationSummary[]>
  /** Parent ids whose children are currently being fetched. With an empty
   *  placeholder array in `childrenByParent`, membership here renders the loading
   *  spinner; an empty array WITHOUT membership is a settled-empty subtree
   *  (renders nothing). Optional. */
  childrenLoading?: ReadonlySet<number>
}): SidebarRow[] {
  const {
    pinned,
    pinnedExpanded,
    orderedFolderIds,
    byFolder,
    folderExpanded,
    folderTotalCounts,
    foldersExpanded,
    chatConversations,
    chatsExpanded,
    sectionOrder = "folders-first",
    conversationExpanded = EMPTY_EXPANDED,
    childrenByParent = EMPTY_CHILDREN,
    childrenLoading = EMPTY_EXPANDED,
  } = args
  const rows: SidebarRow[] = []

  if (pinned.length > 0) {
    rows.push({
      kind: "section",
      section: "pinned",
      expanded: pinnedExpanded,
      count: pinned.length,
    })
    if (pinnedExpanded) {
      for (const conv of pinned) {
        pushConversationRow(
          rows,
          conv,
          0,
          conversationExpanded,
          childrenByParent,
          childrenLoading
        )
      }
    }
  }

  // The Folders and Chat sections sit below the (always-top) Pinned section in
  // an order the user controls via `sectionOrder`. Each is its own closure so
  // the order they emit into `rows` is a one-line swap below — the row logic
  // inside each (both headers always present, each with its own empty hint)
  // stays intact regardless of position.
  const pushFolders = () => {
    // The Folders section header is always present (a permanent entry point),
    // mirroring the Chat section — so a workspace with chats but no open folders
    // still shows the "Folders" heading and its "add a folder" actions. The
    // fully-empty initial workspace (no folders AND no conversations) never
    // reaches buildRows; the list renders a dedicated open-folder CTA there.
    rows.push({
      kind: "section",
      section: "folders",
      expanded: foldersExpanded,
      count: orderedFolderIds.length,
    })
    if (!foldersExpanded) return
    if (orderedFolderIds.length === 0) {
      rows.push({ kind: "folders-empty" })
      return
    }
    for (const folderId of orderedFolderIds) {
      rows.push({ kind: "folder", folderId })
      const expanded = folderExpanded[folderId] ?? true
      if (!expanded) continue
      const convs = byFolder.get(folderId)
      if (!convs || convs.length === 0) {
        rows.push({
          kind: "empty",
          folderId,
          totalConversationCount: folderTotalCounts.get(folderId) ?? 0,
        })
        continue
      }
      for (const conv of convs) {
        pushConversationRow(
          rows,
          conv,
          0,
          conversationExpanded,
          childrenByParent,
          childrenLoading
        )
      }
    }
  }

  const pushChats = () => {
    // The Chat section header is always present (a permanent entry point),
    // unlike the conditional Pinned/Folders headers.
    rows.push({
      kind: "section",
      section: "chats",
      expanded: chatsExpanded,
      count: chatConversations.length,
    })
    if (chatsExpanded) {
      if (chatConversations.length === 0) {
        rows.push({ kind: "chats-empty" })
      } else {
        for (const conv of chatConversations) {
          pushConversationRow(
            rows,
            conv,
            0,
            conversationExpanded,
            childrenByParent,
            childrenLoading
          )
        }
      }
    }
  }

  if (sectionOrder === "chats-first") {
    pushChats()
    pushFolders()
  } else {
    pushFolders()
    pushChats()
  }

  return rows
}

/**
 * Flat index of the conversation row for `(id, agentType)`, or -1 if absent
 * (folder collapsed, filtered out, or unknown). Used by `scrollToActive` to
 * drive `VirtualizerHandle.scrollToIndex` — off-screen virtualized rows are not
 * in the DOM, so a querySelector-based lookup no longer works.
 */
export function flatIndexOfConversation(
  rows: readonly SidebarRow[],
  id: number,
  agentType: string
): number {
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]
    if (
      row.kind === "conversation" &&
      row.conversation.id === id &&
      row.conversation.agent_type === agentType
    ) {
      return i
    }
  }
  return -1
}

// ── Folder drag index math (Phase 2 custom pointer reorder) ──────────────────

/**
 * Map a pointer's Y position over the (fixed row height) collapsed drag surface
 * to a target folder slot, clamped to `[0, count - 1]`.
 *
 * @param pointerY   `clientY` of the pointer
 * @param surfaceTop `getBoundingClientRect().top` of the scroll viewport
 * @param scrollTop  current scroll offset of the viewport
 * @param rowHeight  height of one folder header row in px (fixed, 32)
 * @param count      number of folder rows on the surface
 */
export function pointerYToTargetIndex(
  pointerY: number,
  surfaceTop: number,
  scrollTop: number,
  rowHeight: number,
  count: number
): number {
  if (count <= 0) return 0
  if (rowHeight <= 0) return 0
  const raw = Math.floor((pointerY - surfaceTop + scrollTop) / rowHeight)
  return Math.max(0, Math.min(count - 1, raw))
}

/**
 * Move the item at `from` to `to`, returning a new array. Out-of-range indices
 * are clamped; a no-op move still returns a fresh array copy.
 */
export function applyReorder<T>(
  order: readonly T[],
  from: number,
  to: number
): T[] {
  const next = order.slice()
  if (from < 0 || from >= next.length) return next
  const clampedTo = Math.max(0, Math.min(next.length - 1, to))
  if (from === clampedTo) return next
  const [moved] = next.splice(from, 1)
  next.splice(clampedTo, 0, moved)
  return next
}

// ── Sticky folder header (floating overlay) ─────────────────────────────────
// virtua renders every row as `position:absolute; top:<offset>` inside a
// `contain:strict` container and unmounts off-screen rows, so CSS
// `position:sticky` cannot pin a folder header. Instead a single floating
// overlay stands in for the folder currently scrolled through. These pure
// helpers resolve "which folder" and the iOS-style handoff offset from the
// virtua handle's measured pixel offsets — see the wiring in
// `SidebarConversationList`.

/**
 * For every flat row, the index of the folder header that owns it: a folder
 * header owns itself; a conversation/empty row owns the nearest folder header
 * above it (or -1 if none precedes it, which `buildRows` never produces). Lets
 * the scroll handler resolve the active folder in O(1) from the topmost visible
 * row index, instead of an O(folder span) backward scan that would jank in very
 * large folders.
 */
export function buildOwnerHeaderIndex(rows: readonly SidebarRow[]): Int32Array {
  const out = new Int32Array(rows.length)
  let current = -1
  for (let i = 0; i < rows.length; i++) {
    if (rows[i].kind === "folder") current = i
    out[i] = current
  }
  return out
}

/** Flat indices of every folder header row, in ascending order. */
export function folderHeaderFlatIndices(rows: readonly SidebarRow[]): number[] {
  const indices: number[] = []
  for (let i = 0; i < rows.length; i++) {
    if (rows[i].kind === "folder") indices.push(i)
  }
  return indices
}

/**
 * The next folder header flat index strictly after `activeHeaderIndex`, or
 * `null` when `activeHeaderIndex` is the last folder. `headerIndices` must be
 * ascending (as produced by {@link folderHeaderFlatIndices}).
 */
export function nextHeaderAfter(
  headerIndices: readonly number[],
  activeHeaderIndex: number
): number | null {
  for (let i = 0; i < headerIndices.length; i++) {
    if (headerIndices[i] > activeHeaderIndex) return headerIndices[i]
  }
  return null
}

/**
 * Flat index of the folder header row for `folderId`, or -1 if absent. Used
 * after a collapse-from-overlay toggle to scroll that header to the top.
 */
export function headerIndexForFolder(
  rows: readonly SidebarRow[],
  folderId: number
): number {
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]
    if (row.kind === "folder" && row.folderId === folderId) return i
  }
  return -1
}

/**
 * Pure geometry for the floating sticky folder header. All inputs are measured
 * pixel offsets from the virtua handle; no DOM access.
 *
 * - `visible`: the active folder's own header has scrolled above the viewport
 *   top, so the overlay should stand in for it. (At the very top, where
 *   `scrollOffset === activeHeaderOffset`, the real header is shown instead.)
 * - `translateY`: iOS-style handoff — once the next folder's header is within
 *   one header height of the top it pushes the overlay up so the incoming header
 *   displaces it. Rounded to whole pixels to avoid sub-pixel shimmer against the
 *   real (still-mounted within the buffer) header underneath.
 */
export function computeStickyState(args: {
  scrollOffset: number
  activeHeaderOffset: number
  nextHeaderOffset: number | null
  headerHeight: number
}): { visible: boolean; translateY: number } {
  const { scrollOffset, activeHeaderOffset, nextHeaderOffset, headerHeight } =
    args
  const visible = scrollOffset > activeHeaderOffset
  let translateY = 0
  if (visible && nextHeaderOffset != null) {
    const d = nextHeaderOffset - scrollOffset
    if (d >= 0 && d < headerHeight) {
      translateY = Math.round(d - headerHeight)
    }
  }
  return { visible, translateY }
}
