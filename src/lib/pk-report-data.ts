import {
  getFileTree,
  getGitBranch,
  gitDiff,
  gitDiffWithBranch,
  gitStatus,
  pkRoundGetReportSnapshot,
  pkRoundSaveReportSnapshot,
  readWorkspaceFileBase64,
} from "@/lib/api"
import { inlineHtmlResources } from "@/lib/html-preview-inline"
import {
  decodePkReportArtifact,
  encodePkReportArtifact,
} from "@/lib/pk-report-artifact"
import {
  pickRunnableHtmlPath,
  reportableArtifactPaths,
  type PkReportArtifact,
} from "@/lib/pk-report"
import type { FileTreeNode } from "@/lib/types"
import type {
  PkContestant,
  PkContestantUsage,
  PkRound,
} from "@/stores/pk-arena-store"

const MAX_HTML_BYTES = 2_000_000
const MAX_RESOURCE_BYTES = 8 * 1024 * 1024
const MAX_INLINE_BYTES = 4 * 1024 * 1024

export interface PkReportSnapshotContestant {
  slot: number
  status?: PkContestant["status"]
  statusDetail?: string | null
  startedAt?: number | null
  endedAt?: number | null
  durationMs?: number | null
  usage?: PkContestantUsage | null
  diff?: string | null
}

export interface PkReportSnapshot {
  version: 1
  roundId: string
  capturedAt: number
  contestants: PkReportSnapshotContestant[]
  artifactsBySlot: Record<string, PkReportArtifact[]>
}

export interface PkReportData {
  round: PkRound
  artifactsBySlot: Record<string, PkReportArtifact[]>
  source: "fresh" | "snapshot" | "empty"
}

export interface PkReportDataDependencies {
  getFileTree: (root: string, maxDepth: number) => Promise<FileTreeNode[]>
  readWorkspaceFileBase64: (
    root: string,
    path: string,
    maxBytes: number
  ) => Promise<string>
  listChangedPaths: (root: string) => Promise<string[]>
  readDiff: (root: string, round: PkRound) => Promise<string | null>
  loadSnapshot: (roundId: string) => Promise<PkReportSnapshot | null>
  saveSnapshot: (snapshot: PkReportSnapshot) => Promise<void>
}

function normalizePath(path: string): string {
  return path.replace(/\\/g, "/").replace(/\/$/, "")
}

function joinPath(root: string, relative: string): string {
  return `${normalizePath(root)}/${relative.replace(/^\.\//, "")}`
}

function dirname(path: string): string {
  const normalized = normalizePath(path)
  const slash = normalized.lastIndexOf("/")
  return slash <= 0 ? normalized : normalized.slice(0, slash)
}

function relativeToRoot(root: string, absolutePath: string): string {
  const normalizedRoot = normalizePath(root)
  const normalizedPath = normalizePath(absolutePath)
  if (normalizedPath === normalizedRoot) return ""
  const prefix = `${normalizedRoot}/`
  if (!normalizedPath.startsWith(prefix)) {
    throw new Error("report resource escaped contestant worktree")
  }
  return normalizedPath.slice(prefix.length)
}

function flattenTree(nodes: readonly FileTreeNode[], prefix = ""): string[] {
  const files: string[] = []
  for (const node of nodes) {
    const relative = prefix ? `${prefix}/${node.name}` : node.name
    if (node.kind === "file") files.push(relative)
    else files.push(...flattenTree(node.children, relative))
  }
  return files
}

/** Files present in a git diff, including changes already committed on a PK
 * branch. Status contributes working-tree and untracked files separately. */
export function changedPathsFromDiff(diff: string | null): string[] {
  if (!diff) return []
  const paths = new Set<string>()
  for (const line of diff.split("\n")) {
    if (!line.startsWith("+++ b/")) continue
    const path = line.slice("+++ b/".length).trim()
    if (path && path !== "/dev/null") paths.add(path)
  }
  return [...paths]
}

function artifactRoots(round: PkRound, contestant: PkContestant): string[] {
  const roots = [
    contestant.worktreePath,
    `${round.workingDir}/.codeg-pk/${round.id}/${contestant.slot}`,
    // Rounds created before slot identity used the agent wire name here.
    `${round.workingDir}/.codeg-pk/${round.id}/${contestant.agentType}`,
  ].filter((root): root is string => Boolean(root))
  return [...new Set(roots.map(normalizePath))]
}

function mergeSnapshot(round: PkRound, snapshot: PkReportSnapshot | null) {
  if (!snapshot || snapshot.roundId !== round.id) return round
  const bySlot = new Map(snapshot.contestants.map((item) => [item.slot, item]))
  return {
    ...round,
    contestants: round.contestants.map((contestant) => {
      const saved = bySlot.get(contestant.slot)
      if (!saved) return contestant
      const patch = Object.fromEntries(
        Object.entries(saved).filter(
          ([key, value]) => key !== "slot" && value !== undefined
        )
      )
      return { ...contestant, ...patch }
    }),
  } as PkRound
}

function snapshotOf(
  round: PkRound,
  artifactsBySlot: Record<string, PkReportArtifact[]>
): PkReportSnapshot {
  return {
    version: 1,
    roundId: round.id,
    capturedAt: Date.now(),
    contestants: round.contestants.map((contestant) => ({
      slot: contestant.slot,
      status: contestant.status,
      statusDetail: contestant.statusDetail,
      startedAt: contestant.startedAt,
      endedAt: contestant.endedAt,
      durationMs: contestant.durationMs,
      usage: contestant.usage,
      diff: contestant.diff,
    })),
    artifactsBySlot,
  }
}

function parseSnapshot(raw: string | null, roundId: string) {
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as PkReportSnapshot
    if (
      parsed.version !== 1 ||
      parsed.roundId !== roundId ||
      !Array.isArray(parsed.contestants) ||
      !parsed.artifactsBySlot
    ) {
      return null
    }
    return parsed
  } catch {
    return null
  }
}

const defaultDependencies: PkReportDataDependencies = {
  getFileTree,
  readWorkspaceFileBase64,
  listChangedPaths: async (root) => {
    const entries = await gitStatus(root, true)
    return entries.map((entry) => {
      // Porcelain v1 renders renames as `old -> new`; only the destination can
      // exist in the current worktree and participate in a runnable report.
      const separator = entry.file.lastIndexOf(" -> ")
      return separator >= 0 ? entry.file.slice(separator + 4) : entry.file
    })
  },
  readDiff: async (root, round) => {
    const base = round.baseCommit ?? (await getGitBranch(round.workingDir))
    const diff = base
      ? await gitDiffWithBranch(root, base)
      : await gitDiff(root)
    return diff
  },
  loadSnapshot: async (roundId) =>
    parseSnapshot(await pkRoundGetReportSnapshot(Number(roundId)), roundId),
  saveSnapshot: async (snapshot) =>
    pkRoundSaveReportSnapshot(
      Number(snapshot.roundId),
      JSON.stringify(snapshot)
    ),
}

async function collectContestant(
  root: string,
  round: PkRound,
  contestant: PkContestant,
  dependencies: PkReportDataDependencies
): Promise<{ artifacts: PkReportArtifact[]; diff: string | null }> {
  const tree = await dependencies.getFileTree(root, 8)
  const allPaths = reportableArtifactPaths(flattenTree(tree))
  const diff = contestant.diff ?? (await dependencies.readDiff(root, round))
  const changed = new Set([
    ...(await dependencies.listChangedPaths(root)),
    ...changedPathsFromDiff(diff),
  ])
  const artifactPaths = allPaths.filter((path) => changed.has(path))
  const runnablePath = pickRunnableHtmlPath(artifactPaths)
  let runnableBase64: string | undefined

  if (runnablePath) {
    const encoded = await dependencies.readWorkspaceFileBase64(
      root,
      runnablePath,
      MAX_HTML_BYTES
    )
    const html = decodePkReportArtifact(encoded)
    const inlined = await inlineHtmlResources(html, {
      fileDir: dirname(joinPath(root, runnablePath)),
      folderPath: root,
      maxInlineBytes: MAX_INLINE_BYTES,
      readFileBase64: (absolutePath) =>
        dependencies.readWorkspaceFileBase64(
          root,
          relativeToRoot(root, absolutePath),
          MAX_RESOURCE_BYTES
        ),
    })
    runnableBase64 = encodePkReportArtifact(inlined)
  }

  return {
    diff,
    artifacts: artifactPaths.map((path) => ({
      path,
      ...(path === runnablePath && runnableBase64
        ? { contentBase64: runnableBase64 }
        : {}),
    })),
  }
}

export async function preparePkReportData(
  inputRound: PkRound,
  dependencies: PkReportDataDependencies = defaultDependencies
): Promise<PkReportData> {
  const saved = await dependencies.loadSnapshot(inputRound.id).catch(() => null)
  let round = mergeSnapshot(inputRound, saved)
  const artifactsBySlot: Record<string, PkReportArtifact[]> = {
    ...(saved?.artifactsBySlot ?? {}),
  }
  let fresh = false

  for (const contestant of round.contestants) {
    for (const root of artifactRoots(round, contestant)) {
      try {
        const collected = await collectContestant(
          root,
          round,
          contestant,
          dependencies
        )
        artifactsBySlot[String(contestant.slot)] = collected.artifacts
        round = {
          ...round,
          contestants: round.contestants.map((item) =>
            item.slot === contestant.slot
              ? { ...item, diff: collected.diff }
              : item
          ),
        }
        fresh = true
        break
      } catch {
        // Try the slot path, then the legacy agent-named path, then the saved
        // snapshot. One missing contestant must not block the whole report.
      }
    }
  }

  if (fresh) {
    await dependencies.saveSnapshot(snapshotOf(round, artifactsBySlot))
    return { round, artifactsBySlot, source: "fresh" }
  }
  if (saved) return { round, artifactsBySlot, source: "snapshot" }
  return { round, artifactsBySlot, source: "empty" }
}
