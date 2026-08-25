import { describe, expect, it, vi } from "vitest"
import { decodePkReportArtifact } from "@/lib/pk-report-artifact"
import {
  preparePkReportData,
  type PkReportDataDependencies,
  type PkReportSnapshot,
} from "@/lib/pk-report-data"
import type { PkRound } from "@/stores/pk-arena-store"
import type { FileTreeNode } from "@/lib/types"

const round = {
  id: "7",
  task: "build a game",
  workingDir: "/repo",
  contestants: [
    {
      slot: 0,
      agentType: "qoder",
      worktreePath: null,
      status: "done",
      diff: null,
    },
  ],
} as PkRound

function dependencies(
  overrides: Partial<PkReportDataDependencies> = {}
): PkReportDataDependencies {
  return {
    getFileTree: vi.fn(async () => {
      throw new Error("missing")
    }),
    readWorkspaceFileBase64: vi.fn(async () => {
      throw new Error("missing")
    }),
    listChangedPaths: vi.fn(async () => []),
    readDiff: vi.fn(async () => null),
    loadSnapshot: vi.fn(async () => null),
    saveSnapshot: vi.fn(async () => undefined),
    ...overrides,
  }
}

describe("preparePkReportData", () => {
  it("recovers a legacy agent-named worktree and inlines its web dependencies", async () => {
    const files: Record<string, string> = {
      "index.html": '<script src="assets/game.js"></script><h1>Game</h1>',
      "assets/game.js": 'document.querySelector("h1").textContent = "Ready"',
    }
    const deps = dependencies({
      getFileTree: vi.fn(async (root): Promise<FileTreeNode[]> => {
        if (root !== "/repo/.codeg-pk/7/qoder") throw new Error("missing")
        return [
          { kind: "file", name: "index.html", path: `${root}/index.html` },
          {
            kind: "dir",
            name: "assets",
            path: `${root}/assets`,
            children: [
              {
                kind: "file",
                name: "game.js",
                path: `${root}/assets/game.js`,
              },
            ],
          },
        ]
      }),
      readWorkspaceFileBase64: vi.fn(async (_root, path) => btoa(files[path])),
      listChangedPaths: vi.fn(async () => ["index.html", "assets/game.js"]),
      readDiff: vi.fn(
        async () =>
          "diff --git a/index.html b/index.html\n+++ b/index.html\ndiff --git a/assets/game.js b/assets/game.js\n+++ b/assets/game.js"
      ),
    })

    const result = await preparePkReportData(round, deps)
    const artifact = result.artifactsBySlot["0"][0]

    expect(result.source).toBe("fresh")
    expect(artifact.path).toBe("index.html")
    expect(decodePkReportArtifact(artifact.contentBase64 ?? "")).toContain(
      'document.querySelector("h1")'
    )
    expect(deps.saveSnapshot).toHaveBeenCalledOnce()
  })

  it("exports a historical round from its saved snapshot after worktrees are gone", async () => {
    const snapshot: PkReportSnapshot = {
      version: 1,
      roundId: "7",
      capturedAt: 1,
      contestants: [{ slot: 0, diff: "+historical", usage: null }],
      artifactsBySlot: {
        "0": [{ path: "index.html", contentBase64: btoa("<h1>Saved</h1>") }],
      },
    }
    const deps = dependencies({ loadSnapshot: vi.fn(async () => snapshot) })

    const result = await preparePkReportData(round, deps)

    expect(result.source).toBe("snapshot")
    expect(result.round.contestants[0].diff).toBe("+historical")
    expect(result.artifactsBySlot["0"][0].path).toBe("index.html")
  })

  it("does not present an unchanged repository page as contestant output", async () => {
    const root = "/repo/.codeg-pk/7/0"
    const deps = dependencies({
      getFileTree: vi.fn(
        async (): Promise<FileTreeNode[]> => [
          { kind: "file", name: "index.html", path: `${root}/index.html` },
          { kind: "file", name: "README.md", path: `${root}/README.md` },
        ]
      ),
      readDiff: vi.fn(
        async () =>
          "diff --git a/README.md b/README.md\n+++ b/README.md\n+updated"
      ),
      listChangedPaths: vi.fn(async () => ["README.md"]),
    })

    const result = await preparePkReportData(round, deps)

    expect(result.artifactsBySlot["0"]).toEqual([{ path: "README.md" }])
  })

  it("recognizes a newly created untracked HTML entry when git diff is empty", async () => {
    const root = "/repo/.codeg-pk/7/0"
    const deps = dependencies({
      getFileTree: vi.fn(
        async (): Promise<FileTreeNode[]> => [
          { kind: "file", name: "index.html", path: `${root}/index.html` },
          { kind: "file", name: "README.md", path: `${root}/README.md` },
        ]
      ),
      listChangedPaths: vi.fn(async () => ["index.html"]),
      readDiff: vi.fn(async () => ""),
      readWorkspaceFileBase64: vi.fn(async () => btoa("<h1>New game</h1>")),
    })

    const result = await preparePkReportData(round, deps)

    expect(result.artifactsBySlot["0"]).toEqual([
      expect.objectContaining({
        path: "index.html",
        contentBase64: expect.any(String),
      }),
    ])
  })

  it("recognizes an HTML entry already committed on the contestant branch", async () => {
    const root = "/repo/.codeg-pk/7/0"
    const deps = dependencies({
      getFileTree: vi.fn(
        async (): Promise<FileTreeNode[]> => [
          { kind: "file", name: "index.html", path: `${root}/index.html` },
        ]
      ),
      listChangedPaths: vi.fn(async () => []),
      readDiff: vi.fn(
        async () =>
          "diff --git a/index.html b/index.html\n+++ b/index.html\n+committed"
      ),
      readWorkspaceFileBase64: vi.fn(async () =>
        btoa("<h1>Committed game</h1>")
      ),
    })

    const result = await preparePkReportData(round, deps)

    expect(result.artifactsBySlot["0"][0]).toEqual(
      expect.objectContaining({ path: "index.html" })
    )
  })
})
