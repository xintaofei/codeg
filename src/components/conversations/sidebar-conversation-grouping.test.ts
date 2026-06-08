import { describe, expect, it } from "vitest"
import type { DbConversationSummary } from "@/lib/types"
import {
  applyReorder,
  buildOwnerHeaderIndex,
  buildRows,
  computeStickyState,
  flatIndexOfConversation,
  folderHeaderFlatIndices,
  formatRelative,
  groupByFolderWithReuse,
  headerIndexForFolder,
  nextHeaderAfter,
  pointerYToTargetIndex,
  reuseSelected,
  reuseSet,
  type SidebarRow,
} from "./sidebar-conversation-grouping"

const MINUTE = 60_000

function conv(
  id: number,
  folderId: number,
  overrides: Partial<DbConversationSummary> = {}
): DbConversationSummary {
  const createdAt = new Date(1_700_000_000_000 + id * MINUTE).toISOString()
  return {
    id,
    folder_id: folderId,
    title: `conv-${id}`,
    agent_type: "claude_code",
    status: "pending",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 0,
    created_at: createdAt,
    updated_at: createdAt,
    ...overrides,
  }
}

describe("formatRelative", () => {
  const now = 1_700_000_000_000

  it("returns an empty string for an unparseable timestamp", () => {
    expect(formatRelative("", now)).toBe("")
    expect(formatRelative("not-a-date", now)).toBe("")
  })

  it("buckets the elapsed time into compact units", () => {
    expect(formatRelative(new Date(now - 30_000).toISOString(), now)).toBe(
      "now"
    )
    expect(formatRelative(new Date(now - 5 * MINUTE).toISOString(), now)).toBe(
      "5m"
    )
    expect(
      formatRelative(new Date(now - 3 * 60 * MINUTE).toISOString(), now)
    ).toBe("3h")
    expect(
      formatRelative(new Date(now - 2 * 24 * 60 * MINUTE).toISOString(), now)
    ).toBe("2d")
  })

  it("is deterministic for a given `now` regardless of the wall clock", () => {
    const iso = new Date(now - 5 * MINUTE).toISOString()
    // Same inputs → identical string, which is what keeps the card memo hit
    // across re-renders within one minute.
    expect(formatRelative(iso, now)).toBe(formatRelative(iso, now))
  })

  it("ages the label when `now` crosses a unit boundary", () => {
    const iso = new Date(now - 59 * MINUTE).toISOString()
    expect(formatRelative(iso, now)).toBe("59m")
    expect(formatRelative(iso, now + MINUTE)).toBe("1h")
  })
})

describe("groupByFolderWithReuse", () => {
  it("groups by folder and sorts each bucket by created-at descending", () => {
    const list = [conv(1, 10), conv(3, 10), conv(2, 20), conv(4, 10)]
    const grouped = groupByFolderWithReuse(list, "created", new Map())

    expect([...grouped.keys()].sort()).toEqual([10, 20])
    expect(grouped.get(10)!.map((c) => c.id)).toEqual([4, 3, 1])
    expect(grouped.get(20)!.map((c) => c.id)).toEqual([2])
  })

  it("sorts by updated-at descending in updated mode", () => {
    const a = conv(1, 10, { updated_at: new Date(1000).toISOString() })
    const b = conv(2, 10, { updated_at: new Date(5000).toISOString() })
    const grouped = groupByFolderWithReuse([a, b], "updated", new Map())
    expect(grouped.get(10)!.map((c) => c.id)).toEqual([2, 1])
  })

  it("reuses the prior bucket array for folders whose membership is unchanged", () => {
    const a1 = conv(1, 10)
    const a2 = conv(2, 10)
    const b1 = conv(3, 20)
    const first = groupByFolderWithReuse([a1, a2, b1], "created", new Map())

    // Simulate a status event on folder 10: one summary is replaced by a new
    // object (slice + spread), every other summary keeps its identity.
    const a2Patched = { ...a2, status: "completed" }
    const second = groupByFolderWithReuse([a1, a2Patched, b1], "created", first)

    // Folder 20 is untouched → same array reference (memo can bail out).
    expect(second.get(20)).toBe(first.get(20))
    // Folder 10 changed → a fresh array reference.
    expect(second.get(10)).not.toBe(first.get(10))
    // …but the untouched summary inside folder 10 keeps its object identity,
    // so its card memo still bails out.
    expect(second.get(10)).toContain(a1)
    expect(second.get(10)).toContain(a2Patched)
    expect(second.get(10)).not.toContain(a2)
  })

  it("reuses every bucket when nothing changed at all", () => {
    const list = [conv(1, 10), conv(2, 20)]
    const first = groupByFolderWithReuse(list, "created", new Map())
    const second = groupByFolderWithReuse(list, "created", first)
    expect(second.get(10)).toBe(first.get(10))
    expect(second.get(20)).toBe(first.get(20))
  })

  it("merges worktree child folders into their parent bucket", () => {
    // folder 11 + 12 are worktrees of root folder 10.
    const childToParent = new Map<number, number>([
      [11, 10],
      [12, 10],
    ])
    const list = [conv(1, 10), conv(2, 11), conv(3, 12), conv(4, 20)]
    const grouped = groupByFolderWithReuse(
      list,
      "created",
      new Map(),
      childToParent
    )

    // No child folder gets its own bucket; everything lands under the root (10).
    expect([...grouped.keys()].sort((a, b) => a - b)).toEqual([10, 20])
    expect(
      grouped
        .get(10)!
        .map((c) => c.id)
        .sort()
    ).toEqual([1, 2, 3])
    // The merge never rewrites folder_id — each conversation keeps its own.
    const merged = grouped.get(10)!
    expect(merged.find((c) => c.id === 2)!.folder_id).toBe(11)
    expect(merged.find((c) => c.id === 3)!.folder_id).toBe(12)
  })

  it("sorts the merged parent+worktree bucket as one list", () => {
    const childToParent = new Map<number, number>([[11, 10]])
    // ids encode created-at order (higher id = newer), interleaved across folders.
    const list = [conv(1, 10), conv(4, 11), conv(2, 11), conv(3, 10)]
    const grouped = groupByFolderWithReuse(
      list,
      "created",
      new Map(),
      childToParent
    )
    expect(grouped.get(10)!.map((c) => c.id)).toEqual([4, 3, 2, 1])
  })

  it("leaves grouping unchanged when childToParent is empty/omitted", () => {
    const list = [conv(1, 10), conv(2, 11)]
    const withEmpty = groupByFolderWithReuse(
      list,
      "created",
      new Map(),
      new Map()
    )
    expect([...withEmpty.keys()].sort((a, b) => a - b)).toEqual([10, 11])
  })
})

describe("reuseSet", () => {
  it("returns the previous set when membership is unchanged", () => {
    const prev = new Set(["a:1", "b:2"])
    const next = new Set(["b:2", "a:1"])
    expect(reuseSet(prev, next)).toBe(prev)
  })

  it("returns the next set when membership differs", () => {
    const prev = new Set(["a:1"])
    expect(reuseSet(prev, new Set(["a:1", "b:2"]))).not.toBe(prev)
    expect(reuseSet(new Set(["a:1", "b:2"]), new Set(["a:1"]))).toEqual(
      new Set(["a:1"])
    )
    expect(reuseSet(new Set(["a:1"]), new Set(["b:2"]))).toEqual(
      new Set(["b:2"])
    )
  })
})

describe("reuseSelected", () => {
  it("returns the previous ref when it denotes the same conversation", () => {
    const prev = { id: 1, agentType: "claude_code" }
    expect(reuseSelected(prev, { id: 1, agentType: "claude_code" })).toBe(prev)
  })

  it("returns the next value when the selection changed or cleared", () => {
    const prev = { id: 1, agentType: "claude_code" }
    expect(reuseSelected(prev, { id: 2, agentType: "claude_code" })).toEqual({
      id: 2,
      agentType: "claude_code",
    })
    expect(reuseSelected(prev, { id: 1, agentType: "codex" })).toEqual({
      id: 1,
      agentType: "codex",
    })
    expect(reuseSelected(prev, null)).toBeNull()
    expect(reuseSelected(null, prev)).toBe(prev)
  })
})

describe("buildRows", () => {
  it("emits a single header row for a collapsed folder", () => {
    const byFolder = new Map([[10, [conv(1, 10), conv(2, 10)]]])
    const rows = buildRows([10], byFolder, { 10: false }, new Map([[10, 2]]))
    expect(rows).toEqual([{ kind: "folder", folderId: 10 }])
  })

  it("defaults to expanded when folderExpanded has no entry", () => {
    const byFolder = new Map([[10, [conv(1, 10)]]])
    const rows = buildRows([10], byFolder, {}, new Map([[10, 1]]))
    expect(rows.map((r) => r.kind)).toEqual(["folder", "conversation"])
  })

  it("emits header + empty-hint row for an expanded folder with no visible rows", () => {
    const rows = buildRows([10], new Map(), { 10: true }, new Map([[10, 3]]))
    expect(rows).toEqual([
      { kind: "folder", folderId: 10 },
      { kind: "empty", folderId: 10, totalConversationCount: 3 },
    ])
  })

  it("carries the unfiltered total count on the empty-hint row", () => {
    // byFolder is empty (all filtered out) but the folder has 5 conversations
    // total → renderer shows "no unfinished conversations", not "empty folder".
    const rows = buildRows([10], new Map(), { 10: true }, new Map([[10, 5]]))
    const empty = rows.find((r) => r.kind === "empty")
    expect(empty).toMatchObject({ totalConversationCount: 5 })
  })

  it("emits header + each conversation row, passing summary references through", () => {
    const a = conv(1, 10)
    const b = conv(2, 10)
    const byFolder = new Map([[10, [a, b]]])
    const rows = buildRows([10], byFolder, { 10: true }, new Map([[10, 2]]))
    expect(rows).toHaveLength(3)
    expect(rows[0]).toEqual({ kind: "folder", folderId: 10 })
    // The exact summary object references survive (identity, not a copy) — this
    // is what keeps the card memo alive through the flat row model.
    expect(rows[1]).toEqual({ kind: "conversation", conversation: a })
    expect(
      (rows[1] as { conversation: DbConversationSummary }).conversation
    ).toBe(a)
    expect(
      (rows[2] as { conversation: DbConversationSummary }).conversation
    ).toBe(b)
  })

  it("follows orderedFolderIds order across multiple folders", () => {
    const byFolder = new Map([
      [10, [conv(1, 10)]],
      [20, [conv(2, 20)]],
    ])
    const expanded = { 10: true, 20: false }
    const counts = new Map([
      [10, 1],
      [20, 1],
    ])
    // Folder 20 first (collapsed → header only), then 10 (expanded).
    const rows = buildRows([20, 10], byFolder, expanded, counts)
    expect(rows).toEqual([
      { kind: "folder", folderId: 20 },
      { kind: "folder", folderId: 10 },
      { kind: "conversation", conversation: byFolder.get(10)![0] },
    ])
  })
})

describe("flatIndexOfConversation", () => {
  const rows: SidebarRow[] = [
    { kind: "folder", folderId: 10 },
    { kind: "conversation", conversation: conv(1, 10) },
    {
      kind: "conversation",
      conversation: conv(2, 10, { agent_type: "codex" }),
    },
    { kind: "folder", folderId: 20 },
    { kind: "empty", folderId: 20, totalConversationCount: 0 },
  ]

  it("returns the flat index of the matching conversation row", () => {
    expect(flatIndexOfConversation(rows, 1, "claude_code")).toBe(1)
    expect(flatIndexOfConversation(rows, 2, "codex")).toBe(2)
  })

  it("requires both id and agent_type to match", () => {
    expect(flatIndexOfConversation(rows, 2, "claude_code")).toBe(-1)
    expect(flatIndexOfConversation(rows, 99, "claude_code")).toBe(-1)
  })
})

describe("pointerYToTargetIndex", () => {
  it("maps a pointer offset to the row under it", () => {
    // surfaceTop=100, scrollTop=0, rowHeight=32 → y=148 lands in row 1 (132..164)
    expect(pointerYToTargetIndex(148, 100, 0, 32, 5)).toBe(1)
    expect(pointerYToTargetIndex(100, 100, 0, 32, 5)).toBe(0)
  })

  it("accounts for scroll offset", () => {
    // Scrolled down 64px → the same screen Y points two rows lower.
    expect(pointerYToTargetIndex(100, 100, 64, 32, 5)).toBe(2)
  })

  it("clamps above and below the surface", () => {
    expect(pointerYToTargetIndex(0, 100, 0, 32, 5)).toBe(0)
    expect(pointerYToTargetIndex(9999, 100, 0, 32, 5)).toBe(4)
  })

  it("is safe for degenerate inputs", () => {
    expect(pointerYToTargetIndex(150, 100, 0, 32, 0)).toBe(0)
    expect(pointerYToTargetIndex(150, 100, 0, 0, 5)).toBe(0)
  })
})

describe("sticky overlay helpers", () => {
  // F10 expanded (2 convs), F20 collapsed, F30 expanded (empty hint).
  const rows: SidebarRow[] = [
    { kind: "folder", folderId: 10 }, // 0
    { kind: "conversation", conversation: conv(1, 10) }, // 1
    { kind: "conversation", conversation: conv(2, 10) }, // 2
    { kind: "folder", folderId: 20 }, // 3
    { kind: "folder", folderId: 30 }, // 4
    { kind: "empty", folderId: 30, totalConversationCount: 0 }, // 5
  ]

  describe("buildOwnerHeaderIndex", () => {
    it("maps every row to the flat index of its owning folder header", () => {
      expect(Array.from(buildOwnerHeaderIndex(rows))).toEqual([
        0, 0, 0, 3, 4, 4,
      ])
    })

    it("returns an empty array for no rows", () => {
      expect(Array.from(buildOwnerHeaderIndex([]))).toEqual([])
    })
  })

  describe("folderHeaderFlatIndices", () => {
    it("lists folder header indices in ascending order", () => {
      expect(folderHeaderFlatIndices(rows)).toEqual([0, 3, 4])
    })
  })

  describe("nextHeaderAfter", () => {
    it("returns the next header index strictly after the active one", () => {
      const headers = [0, 3, 4]
      expect(nextHeaderAfter(headers, 0)).toBe(3)
      expect(nextHeaderAfter(headers, 3)).toBe(4)
    })

    it("returns null for the last folder", () => {
      expect(nextHeaderAfter([0, 3, 4], 4)).toBeNull()
      expect(nextHeaderAfter([], 0)).toBeNull()
    })
  })

  describe("headerIndexForFolder", () => {
    it("finds the header row index for a folder id", () => {
      expect(headerIndexForFolder(rows, 10)).toBe(0)
      expect(headerIndexForFolder(rows, 30)).toBe(4)
    })

    it("returns -1 when the folder has no header row", () => {
      expect(headerIndexForFolder(rows, 999)).toBe(-1)
    })
  })

  describe("computeStickyState", () => {
    const base = {
      activeHeaderOffset: 0,
      nextHeaderOffset: 96,
      headerHeight: 32,
    }

    it("hides the overlay when the real header is at the top", () => {
      expect(computeStickyState({ ...base, scrollOffset: 0 })).toEqual({
        visible: false,
        translateY: 0,
      })
    })

    it("shows the overlay with no offset mid-folder", () => {
      expect(computeStickyState({ ...base, scrollOffset: 40 })).toEqual({
        visible: true,
        translateY: 0,
      })
    })

    it("pushes the overlay up as the next header enters the handoff window", () => {
      // next header at 96, scrolled to 80 → d=16 (<32) → translateY 16-32 = -16
      expect(computeStickyState({ ...base, scrollOffset: 80 })).toEqual({
        visible: true,
        translateY: -16,
      })
    })

    it("does not push while the next header is a full header height away", () => {
      // d === headerHeight is the exclusive boundary → no push yet.
      expect(computeStickyState({ ...base, scrollOffset: 64 })).toEqual({
        visible: true,
        translateY: 0,
      })
    })

    it("never pushes for the last folder (no next header)", () => {
      expect(
        computeStickyState({
          scrollOffset: 1000,
          activeHeaderOffset: 320,
          nextHeaderOffset: null,
          headerHeight: 32,
        })
      ).toEqual({ visible: true, translateY: 0 })
    })

    it("rounds the handoff offset to whole pixels", () => {
      // d = 95.4 - 80 = 15.4 → round(15.4 - 32) = round(-16.6) = -17
      expect(
        computeStickyState({
          scrollOffset: 80,
          activeHeaderOffset: 0,
          nextHeaderOffset: 95.4,
          headerHeight: 32,
        }).translateY
      ).toBe(-17)
    })
  })
})

describe("applyReorder", () => {
  it("moves an item forward", () => {
    expect(applyReorder([1, 2, 3, 4], 0, 2)).toEqual([2, 3, 1, 4])
  })

  it("moves an item backward", () => {
    expect(applyReorder([1, 2, 3, 4], 3, 1)).toEqual([1, 4, 2, 3])
  })

  it("returns a fresh copy on a no-op move", () => {
    const order = [1, 2, 3]
    const result = applyReorder(order, 1, 1)
    expect(result).toEqual([1, 2, 3])
    expect(result).not.toBe(order)
  })

  it("clamps the destination and ignores an out-of-range source", () => {
    expect(applyReorder([1, 2, 3], 0, 99)).toEqual([2, 3, 1])
    expect(applyReorder([1, 2, 3], 5, 0)).toEqual([1, 2, 3])
  })
})
