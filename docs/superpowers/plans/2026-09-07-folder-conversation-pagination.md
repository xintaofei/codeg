# Folder Conversation Pagination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show folder conversations six at a time, reveal six more per click,
and reset a folder to six when it is collapsed.

**Architecture:** Keep all conversation data loaded and sorted as it is today.
Extend the pure sidebar row builder with a folder-scoped footer row and pass it
a session-only visible-count map from `SidebarConversationList`; this preserves
the existing `virtua` row indexes, sticky headers, and folder/worktree nesting.

**Tech Stack:** React 19, TypeScript strict mode, Next.js 16, next-intl,
Vitest, Testing Library, Tailwind CSS v4, Lucide React, virtua.

## Global Constraints

- The folder page size is exactly 6 conversations.
- Each load-more click reveals exactly 6 additional conversations for only the
  clicked folder.
- Collapsing a folder resets only that folder to the first 6 conversations.
- Collapsing all folders clears all folder pagination state.
- Existing folder sort order remains authoritative, including its current
  newest-first behavior.
- Delegation children remain attached to their visible parent and do not consume
  page slots.
- Pinned, Chat, and Recent sections retain their existing behavior.
- Visible counts are session-only and are not written to local storage or the
  backend.
- No dependencies or backend APIs are added.

---

## File Structure

- `src/components/conversations/sidebar-conversation-grouping.ts`: defines the
  page size, folder footer row type, and pure row slicing behavior.
- `src/components/conversations/sidebar-conversation-grouping.test.ts`: verifies
  the pure row model independently of React rendering.
- `src/components/conversations/sidebar-conversation-list.tsx`: owns per-folder
  visible counts, resets them on folder collapse, and renders the footer button.
- `src/components/conversations/sidebar-conversation-list.test.tsx`: verifies
  the complete click, completion, collapse, and reopen flow.
- `src/i18n/messages/{ar,de,en,es,fr,ja,ko,pt,zh-CN,zh-TW}.json`: provides the
  localized folder load-more label.

### Task 1: Implement Folder Conversation Pagination End To End

**Files:**

- Modify: `src/components/conversations/sidebar-conversation-grouping.test.ts`
- Modify: `src/components/conversations/sidebar-conversation-grouping.ts`
- Modify: `src/components/conversations/sidebar-conversation-list.test.tsx`
- Modify: `src/components/conversations/sidebar-conversation-list.tsx`
- Modify: `src/i18n/messages/ar.json`
- Modify: `src/i18n/messages/de.json`
- Modify: `src/i18n/messages/en.json`
- Modify: `src/i18n/messages/es.json`
- Modify: `src/i18n/messages/fr.json`
- Modify: `src/i18n/messages/ja.json`
- Modify: `src/i18n/messages/ko.json`
- Modify: `src/i18n/messages/pt.json`
- Modify: `src/i18n/messages/zh-CN.json`
- Modify: `src/i18n/messages/zh-TW.json`

**Interfaces:**

- Consumes: already sorted `byFolder: Map<number, DbConversationSummary[]>`.
- Produces: `FOLDER_CONVERSATION_PAGE_SIZE = 6`,
  `FolderMoreRow`, and the optional
  `folderConversationLimits?: Readonly<Record<number, number>>` argument on
  `buildRows`.
- `FolderMoreRow` has exact shape
  `{ kind: "folder-more"; folderId: number; depth: number; remaining: number }`.
- Produces the `Folder.sidebar.loadMoreFolderConversations` translation key.

- [ ] **Step 1: Write the failing pure row-model test**

Update `folderRows` in
`src/components/conversations/sidebar-conversation-grouping.test.ts` so its last
parameters and `buildRows` call are:

```typescript
function folderRows(
  orderedFolderIds: number[],
  byFolder: Map<number, DbConversationSummary[]>,
  folderExpanded: Record<number, boolean>,
  folderTotalCounts: Map<number, number>,
  foldersExpanded = true,
  folderConversationLimits: Readonly<Record<number, number>> = {}
): SidebarRow[] {
  const rows = buildRows({
    pinned: [],
    pinnedExpanded: true,
    orderedFolderIds,
    byFolder,
    folderExpanded,
    folderTotalCounts,
    foldersExpanded,
    chatConversations: [],
    chatsExpanded: true,
    folderConversationLimits,
  })
  const chatsIdx = rows.findIndex(
    (r) => r.kind === "section" && r.section === "chats"
  )
  return chatsIdx === -1 ? rows : rows.slice(0, chatsIdx)
}
```

Add this test inside `describe("buildRows")`:

```typescript
it("pages each folder independently in batches of six", () => {
  const conversations = Array.from({ length: 13 }, (_, i) => conv(i + 1, 10))
  const byFolder = new Map([[10, conversations]])
  const counts = new Map([[10, conversations.length]])

  const initial = folderRows([10], byFolder, { 10: true }, counts)
  expect(initial.filter((row) => row.kind === "conversation")).toHaveLength(6)
  expect(initial.at(-1)).toEqual({
    kind: "folder-more",
    folderId: 10,
    depth: 0,
    remaining: 7,
  })

  const nextPage = folderRows(
    [10],
    byFolder,
    { 10: true },
    counts,
    true,
    { 10: 12 }
  )
  expect(nextPage.filter((row) => row.kind === "conversation")).toHaveLength(
    12
  )
  expect(nextPage.at(-1)).toEqual({
    kind: "folder-more",
    folderId: 10,
    depth: 0,
    remaining: 1,
  })
})
```

- [ ] **Step 2: Write the failing component interaction test**

Add this describe block to
`src/components/conversations/sidebar-conversation-list.test.tsx`:

```typescript
describe("SidebarConversationList - folder conversation paging", () => {
  const folderConversationCount = () =>
    document.querySelectorAll("[data-conversation-id]").length
  const loadMoreButton = () =>
    Array.from(document.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("Load more")
    )
  const folderToggle = () =>
    document.querySelector<HTMLButtonElement>(
      '[data-folder-id="1"][aria-expanded]'
    )

  beforeEach(() => {
    localStorage.clear()
    const folders = [folder(1, "Repo")]
    useAppWorkspaceStore.setState({
      folders,
      allFolders: folders,
      conversations: Array.from({ length: 13 }, (_, i) => conv(i + 1, 1)),
    })
  })

  it("reveals six at a time and resets after the folder is collapsed", () => {
    render(tree())

    expect(folderConversationCount()).toBe(6)
    expect(loadMoreButton()?.textContent).toContain("7")

    act(() => fireEvent.click(loadMoreButton()!))
    expect(folderConversationCount()).toBe(12)
    expect(loadMoreButton()?.textContent).toContain("1")

    act(() => fireEvent.click(loadMoreButton()!))
    expect(folderConversationCount()).toBe(13)
    expect(loadMoreButton()).toBeUndefined()

    act(() => fireEvent.click(folderToggle()!))
    expect(folderConversationCount()).toBe(0)

    act(() => fireEvent.click(folderToggle()!))
    expect(folderConversationCount()).toBe(6)
    expect(loadMoreButton()?.textContent).toContain("7")
  })
})
```

- [ ] **Step 3: Run both focused tests and verify RED before production edits**

Run:

```bash
pnpm test src/components/conversations/sidebar-conversation-grouping.test.ts src/components/conversations/sidebar-conversation-list.test.tsx
```

Expected: both new tests FAIL because all 13 folder conversations are emitted;
the initial visible count is 13 instead of 6.

- [ ] **Step 4: Add folder pagination to the pure row model**

Near `RECENT_PAGE_SIZE` in
`src/components/conversations/sidebar-conversation-grouping.ts`, add:

```typescript
export const FOLDER_CONVERSATION_PAGE_SIZE = 6

const EMPTY_FOLDER_CONVERSATION_LIMITS: Readonly<Record<number, number>> = {}
```

Near the other folder row interfaces, add:

```typescript
export interface FolderMoreRow {
  kind: "folder-more"
  folderId: number
  depth: number
  remaining: number
}
```

Add `FolderMoreRow` to the `SidebarRow` union. Add this optional `buildRows`
input after `folderTotalCounts`:

```typescript
folderConversationLimits?: Readonly<Record<number, number>>
```

Destructure it with the shared empty default:

```typescript
folderConversationLimits = EMPTY_FOLDER_CONVERSATION_LIMITS,
```

Replace the conversation loop in `pushFolderBody` with:

```typescript
const limit = Math.max(
  0,
  folderConversationLimits[folderId] ?? FOLDER_CONVERSATION_PAGE_SIZE
)
const shown = convs.slice(0, limit)
for (const conv of shown) {
  pushConversationRow(
    rows,
    conv,
    baseDepth,
    conversationExpanded,
    childrenByParent,
    childrenLoading
  )
}
const remaining = convs.length - shown.length
if (remaining > 0) {
  rows.push({
    kind: "folder-more",
    folderId,
    depth: baseDepth,
    remaining,
  })
}
```

The slice occurs before `pushConversationRow`, so expanded delegation children
stay attached to a visible parent without consuming a top-level page slot.

- [ ] **Step 5: Add per-folder state, increment, and reset actions**

Import `FOLDER_CONVERSATION_PAGE_SIZE` from
`./sidebar-conversation-grouping`. Near the existing `recentLimit` state in
`src/components/conversations/sidebar-conversation-list.tsx`, add:

```typescript
const [folderConversationLimits, setFolderConversationLimits] = useState<
  Record<number, number>
>({})
const revealMoreFolderConversations = useCallback((folderId: number) => {
  setFolderConversationLimits((prev) => ({
    ...prev,
    [folderId]:
      (prev[folderId] ?? FOLDER_CONVERSATION_PAGE_SIZE) +
      FOLDER_CONVERSATION_PAGE_SIZE,
  }))
}, [])
const resetFolderConversationLimit = useCallback((folderId: number) => {
  setFolderConversationLimits((prev) => {
    if (prev[folderId] == null) return prev
    const next = { ...prev }
    delete next[folderId]
    return next
  })
}, [])
```

Call `resetFolderConversationLimit(folderId)` at the start of both
`toggleFolder` and `toggleRootGroup`, and add the callback to their dependency
arrays. In `collapseAll`, clear every folder limit with:

```typescript
setFolderConversationLimits({})
```

Pass `folderConversationLimits` into `buildRows` and add it to the row
`useMemo` dependency list.

- [ ] **Step 6: Render the folder footer and add its unique key**

Add this branch in `renderRow` before the Recent footer branch:

```tsx
if (row.kind === "folder-more") {
  const railLeft = `calc(var(--conv-rail-axis, 0.875rem) + ${row.depth} * ${CONV_RAIL_DEPTH_STEP})`
  return (
    <div className="relative h-[2rem]">
      <button
        type="button"
        onClick={() => revealMoreFolderConversations(row.folderId)}
        className="relative flex h-[1.9375rem] w-full items-center rounded-full pr-[0.25rem] text-left text-[0.75rem] text-muted-foreground/80 outline-none transition-colors duration-[120ms] hover:bg-[color-mix(in_oklab,var(--sidebar-accent),var(--sidebar-foreground)_2%)] hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
        style={{
          paddingLeft: `calc(var(--conv-rail-axis, 0.875rem) + ${row.depth} * ${CONV_RAIL_DEPTH_STEP} + 0.875rem)`,
        }}
      >
        <span
          aria-hidden
          className="pointer-events-none absolute top-1/2 flex h-[0.875rem] w-[0.875rem] -translate-x-1/2 -translate-y-1/2 items-center justify-center"
          style={{ left: railLeft }}
        >
          <ChevronDown className="h-[0.75rem] w-[0.75rem]" />
        </span>
        <span className="truncate">
          {t("loadMoreFolderConversations", { count: row.remaining })}
        </span>
      </button>
    </div>
  )
}
```

Add this branch to `rowKey`:

```typescript
if (row.kind === "folder-more") return `foldermore-${row.folderId}`
```

- [ ] **Step 7: Add the localized label to all ten message files**

Add `loadMoreFolderConversations` beside `showMoreRecent` under
`Folder.sidebar` with these exact values:

```text
ar.json:    "تحميل المزيد ({count})"
de.json:    "Mehr laden ({count})"
en.json:    "Load more ({count})"
es.json:    "Cargar más ({count})"
fr.json:    "Charger plus ({count})"
ja.json:    "さらに読み込む（{count}）"
ko.json:    "더 불러오기 ({count})"
pt.json:    "Carregar mais ({count})"
zh-CN.json: "加载更多（{count}）"
zh-TW.json: "載入更多（{count}）"
```

- [ ] **Step 8: Run the focused suites and verify GREEN**

Run:

```bash
pnpm test src/components/conversations/sidebar-conversation-grouping.test.ts src/components/conversations/sidebar-conversation-list.test.tsx
```

Expected: PASS with both files green.

- [ ] **Step 9: Run frontend regression verification**

Run each command separately:

```bash
pnpm eslint src/components/conversations/sidebar-conversation-grouping.ts src/components/conversations/sidebar-conversation-grouping.test.ts src/components/conversations/sidebar-conversation-list.tsx src/components/conversations/sidebar-conversation-list.test.tsx
pnpm test
pnpm build
```

Expected: ESLint exits 0, all Vitest suites pass, and Next.js completes the
static export build.

- [ ] **Step 10: Review and commit the completed feature**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors and only the planned source, test, and locale
files are modified.

Commit:

```bash
git add src/components/conversations/sidebar-conversation-grouping.ts src/components/conversations/sidebar-conversation-grouping.test.ts src/components/conversations/sidebar-conversation-list.tsx src/components/conversations/sidebar-conversation-list.test.tsx src/i18n/messages/ar.json src/i18n/messages/de.json src/i18n/messages/en.json src/i18n/messages/es.json src/i18n/messages/fr.json src/i18n/messages/ja.json src/i18n/messages/ko.json src/i18n/messages/pt.json src/i18n/messages/zh-CN.json src/i18n/messages/zh-TW.json
git commit -m "feat(sidebar): load folder conversations in batches"
```
