"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useTransition,
  type ComponentProps,
  type ComponentPropsWithoutRef,
  type SyntheticEvent,
} from "react"
import dynamic from "next/dynamic"
import { cjk } from "@streamdown/cjk"
import { code } from "@streamdown/code"
import { math } from "@streamdown/math"
import { mermaid } from "@streamdown/mermaid"
import { convertFileSrc } from "@tauri-apps/api/core"
import {
  ChevronDown,
  ChevronRight,
  Eye,
  FileCode2,
  FileIcon,
  Pencil,
} from "lucide-react"
import type { editor as MonacoEditorNs } from "monaco-editor"
import { useTranslations } from "next-intl"
import { defaultRehypePlugins, Streamdown } from "streamdown"
import type { UrlTransform } from "streamdown"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { useFolderContext } from "@/contexts/folder-context"
import { DiffViewer } from "@/components/diff/diff-viewer"
import { UnifiedDiffPreview } from "@/components/diff/unified-diff-preview"
import { useStreamdownLinkSafety } from "@/components/ai-elements/link-safety"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import { Button } from "@/components/ui/button"
import { ButtonGroup } from "@/components/ui/button-group"
import { defineMonacoThemes, useMonacoThemeSync } from "@/lib/monaco-themes"
import { readFileBase64 } from "@/lib/tauri"

const AUTO_SAVE_DELAY_MS = 5000
const streamdownPlugins = { cjk, code, math, mermaid }

type StreamdownRehypePlugins = NonNullable<
  ComponentProps<typeof Streamdown>["rehypePlugins"]
>
type StreamdownComponents = NonNullable<
  ComponentProps<typeof Streamdown>["components"]
>

function uniqStrings(values: readonly string[]): string[] {
  const seen = new Set<string>()
  const result: string[] = []
  for (const value of values) {
    if (!seen.has(value)) {
      seen.add(value)
      result.push(value)
    }
  }
  return result
}

const URL_SCHEME = /^[a-zA-Z][a-zA-Z\d+\-.]*:/
const WINDOWS_ABSOLUTE_PATH = /^[a-zA-Z]:[\\/]/
const IMAGE_MIME_BY_EXTENSION: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
  bmp: "image/bmp",
  ico: "image/x-icon",
  avif: "image/avif",
}

function buildMonacoModelPath(path: string | null, id: string): string {
  if (!path) return `inmemory://model/${encodeURIComponent(id)}`
  const normalized = path.replace(/\\/g, "/")
  const encoded = normalized.split("/").map(encodeURIComponent).join("/")
  return `file:///${encoded}`
}

function splitUrlSuffix(raw: string): { path: string; suffix: string } {
  const hashIndex = raw.indexOf("#")
  const queryIndex = raw.indexOf("?")
  const cutIndex =
    hashIndex >= 0 && queryIndex >= 0
      ? Math.min(hashIndex, queryIndex)
      : hashIndex >= 0
        ? hashIndex
        : queryIndex

  if (cutIndex < 0) return { path: raw, suffix: "" }
  return { path: raw.slice(0, cutIndex), suffix: raw.slice(cutIndex) }
}

function extensionFromPath(path: string): string {
  const normalized = normalizeSlashPath(path)
  const withoutSuffix = splitUrlSuffix(normalized).path
  const filename = withoutSuffix.split("/").pop() ?? ""
  const idx = filename.lastIndexOf(".")
  if (idx < 0) return ""
  return filename.slice(idx + 1).toLowerCase()
}

function guessImageMimeType(path: string): string {
  const ext = extensionFromPath(path)
  return IMAGE_MIME_BY_EXTENSION[ext] ?? "application/octet-stream"
}

function normalizeSlashPath(path: string): string {
  return path.replace(/\\/g, "/")
}

function dirnameSlashPath(path: string): string {
  const normalized = normalizeSlashPath(path).replace(/\/+$/, "")
  const idx = normalized.lastIndexOf("/")
  if (idx < 0) return ""
  return normalized.slice(0, idx)
}

function resolveSlashPath(baseDir: string, relative: string): string {
  const joined = `${baseDir}/${relative}`.replace(/\/+/g, "/")
  const segments = joined.split("/")
  const stack: string[] = []

  for (const seg of segments) {
    if (!seg || seg === ".") continue
    if (seg === "..") {
      if (stack.length > 0) stack.pop()
      continue
    }
    stack.push(seg)
  }

  return stack.join("/")
}

function isExternalUrl(url: string): boolean {
  if (!url) return false
  if (url.startsWith("//")) return true
  if (WINDOWS_ABSOLUTE_PATH.test(url)) return false
  return URL_SCHEME.test(url)
}

interface DiffOutlineFile {
  key: string
  path: string
  startLine: number
  endLine: number
  additions: number
  deletions: number
  hunks: DiffOutlineHunk[]
}

interface DiffOutlineHunk {
  key: string
  startLine: number
  endLine: number
  header: string
  additions: number
  deletions: number
}

interface DiffOutline {
  files: DiffOutlineFile[]
  totalAdditions: number
  totalDeletions: number
  totalHunks: number
}

type DiffListContext =
  | { kind: "commit"; commitHash: string; commitMessage: string | null }
  | { kind: "working"; path: string }
  | { kind: "branch"; branch: string; path: string }

function decodeDiffTabToken(token: string): string {
  try {
    return decodeURIComponent(token)
  } catch {
    return token
  }
}

function normalizeDiffPath(rawPath: string): string | null {
  const trimmed = rawPath.trim().replace(/^"|"$/g, "")
  if (!trimmed || trimmed === "/dev/null") return null

  if (trimmed.startsWith("a/") || trimmed.startsWith("b/")) {
    return trimmed.slice(2).replace(/\\/g, "/")
  }

  return trimmed.replace(/\\/g, "/")
}

function normalizeWorkspacePath(path: string): string {
  return path.replace(/\\/g, "/")
}

function toWorkspaceRelativePath(folderPath: string, filePath: string): string {
  const folderRoot = normalizeSlashPath(folderPath).replace(/\/+$/, "")
  const normalized = normalizeSlashPath(filePath)
  if (!folderRoot) return normalized

  const isWindows = WINDOWS_ABSOLUTE_PATH.test(folderRoot)
  const rootForCompare = isWindows ? folderRoot.toLowerCase() : folderRoot
  const fileForCompare = isWindows ? normalized.toLowerCase() : normalized

  if (fileForCompare === rootForCompare) return ""
  if (fileForCompare.startsWith(`${rootForCompare}/`)) {
    return normalized.slice(folderRoot.length + 1)
  }

  return normalized.startsWith("/") ? normalized.slice(1) : normalized
}

function resolveMarkdownImageNativePath(params: {
  folderPath: string
  activeFilePath: string
  rawUrl: string
}): string | null {
  const trimmed = params.rawUrl.trim()
  if (!trimmed) return null
  if (trimmed.startsWith("data:") || trimmed.startsWith("blob:")) return null
  if (isExternalUrl(trimmed)) return null

  const { path: urlPath } = splitUrlSuffix(trimmed)
  if (!urlPath) return null
  if (WINDOWS_ABSOLUTE_PATH.test(urlPath)) return null

  const workspaceActivePath = toWorkspaceRelativePath(
    params.folderPath,
    params.activeFilePath
  )

  const relativeWorkspacePath = urlPath.startsWith("/")
    ? urlPath.slice(1)
    : resolveSlashPath(dirnameSlashPath(workspaceActivePath), urlPath)
  if (!relativeWorkspacePath) return null

  const root = normalizeSlashPath(params.folderPath).replace(/\/+$/, "")
  const absoluteSlash = `${root}/${relativeWorkspacePath}`.replace(/\/+/g, "/")
  return WINDOWS_ABSOLUTE_PATH.test(params.folderPath)
    ? absoluteSlash.replace(/\//g, "\\")
    : absoluteSlash
}

function parsePathFromDiffGitLine(line: string): string | null {
  if (!line.startsWith("diff --git ")) return null
  const match = line.match(/^diff --git\s+(.+?)\s+(.+)$/)
  if (!match) return null

  return normalizeDiffPath(match[2]) ?? normalizeDiffPath(match[1])
}

function parsePathFromFileHeader(
  line: string,
  prefix: "--- " | "+++ "
): string | null {
  if (!line.startsWith(prefix)) return null
  return normalizeDiffPath(line.slice(prefix.length))
}

function parsePathFromApplyPatchLine(line: string): string | null {
  const prefixes = ["*** Update File: ", "*** Add File: ", "*** Delete File: "]

  for (const prefix of prefixes) {
    if (line.startsWith(prefix)) {
      return normalizeDiffPath(line.slice(prefix.length))
    }
  }

  return null
}

function parseMovedPathFromApplyPatchLine(line: string): string | null {
  if (!line.startsWith("*** Move to: ")) return null
  return normalizeDiffPath(line.slice(13))
}

function buildDiffOutline(content: string): DiffOutline | null {
  if (!content.trim()) return null

  const lines = content.split("\n")
  const files: DiffOutlineFile[] = []

  let current: DiffOutlineFile | null = null
  let currentHunk: {
    startLine: number
    header: string
    additions: number
    deletions: number
  } | null = null
  let fileIndex = 1
  let hunkIndex = 1

  const createFile = (
    lineNumber: number,
    path: string | null
  ): DiffOutlineFile => {
    const entry: DiffOutlineFile = {
      key: `${lineNumber}-${fileIndex}`,
      path: path ?? `Diff #${fileIndex}`,
      startLine: lineNumber,
      endLine: lineNumber,
      additions: 0,
      deletions: 0,
      hunks: [],
    }
    fileIndex += 1
    return entry
  }

  const flushHunk = (endLine: number) => {
    if (!current || !currentHunk) return

    const normalizedEnd = Math.max(currentHunk.startLine, endLine)
    current.hunks.push({
      key: `${current.key}:hunk-${hunkIndex}`,
      startLine: currentHunk.startLine,
      endLine: normalizedEnd,
      header: currentHunk.header,
      additions: currentHunk.additions,
      deletions: currentHunk.deletions,
    })
    hunkIndex += 1
    currentHunk = null
  }

  const flushFile = () => {
    if (!current) return

    if (currentHunk) {
      flushHunk(current.endLine)
    }

    if (
      current.hunks.length === 0 &&
      (current.additions > 0 || current.deletions > 0)
    ) {
      current.hunks.push({
        key: `${current.key}:hunk-${hunkIndex}`,
        startLine: current.startLine,
        endLine: current.endLine,
        header: "Change",
        additions: current.additions,
        deletions: current.deletions,
      })
      hunkIndex += 1
    }

    files.push(current)
    current = null
  }

  for (const [index, line] of lines.entries()) {
    const lineNumber = index + 1

    const diffGitPath = parsePathFromDiffGitLine(line)
    if (diffGitPath) {
      flushFile()
      current = createFile(lineNumber, diffGitPath)
      continue
    }

    const applyPatchPath = parsePathFromApplyPatchLine(line)
    if (applyPatchPath) {
      flushFile()
      current = createFile(lineNumber, applyPatchPath)
      continue
    }

    if (line.startsWith("--- ") && currentHunk) {
      flushHunk(lineNumber - 1)
    }

    const movedPath = parseMovedPathFromApplyPatchLine(line)
    if (movedPath && current) {
      current.path = movedPath
    }

    if (!current) {
      const minusPath = parsePathFromFileHeader(line, "--- ")
      if (minusPath) {
        current = createFile(lineNumber, minusPath)
      } else {
        const plusPath = parsePathFromFileHeader(line, "+++ ")
        if (plusPath) current = createFile(lineNumber, plusPath)
      }
    } else {
      const plusPath = parsePathFromFileHeader(line, "+++ ")
      if (plusPath) current.path = plusPath
    }

    if (!current) continue
    current.endLine = lineNumber

    if (line.startsWith("@@ ")) {
      if (currentHunk) {
        flushHunk(lineNumber - 1)
      }
      currentHunk = {
        startLine: lineNumber,
        header: line,
        additions: 0,
        deletions: 0,
      }
      continue
    }

    if (line.startsWith("+") && !line.startsWith("+++")) {
      current.additions += 1
      if (currentHunk) currentHunk.additions += 1
    }
    if (line.startsWith("-") && !line.startsWith("---")) {
      current.deletions += 1
      if (currentHunk) currentHunk.deletions += 1
    }
  }

  flushFile()

  if (files.length === 0) return null

  const totalAdditions = files.reduce((sum, file) => sum + file.additions, 0)
  const totalDeletions = files.reduce((sum, file) => sum + file.deletions, 0)
  const totalHunks = files.reduce((sum, file) => sum + file.hunks.length, 0)

  return {
    files,
    totalAdditions,
    totalDeletions,
    totalHunks,
  }
}

function setEditorHiddenAreas(
  editor: MonacoEditorNs.IStandaloneCodeEditor,
  ranges: {
    startLineNumber: number
    startColumn: number
    endLineNumber: number
    endColumn: number
  }[]
) {
  const hiddenAreaEditor = editor as unknown as {
    setHiddenAreas?: (
      hiddenRanges: {
        startLineNumber: number
        startColumn: number
        endLineNumber: number
        endColumn: number
      }[]
    ) => void
  }

  hiddenAreaEditor.setHiddenAreas?.(ranges)
}

const MonacoEditor = dynamic(async () => import("@monaco-editor/react"), {
  ssr: false,
})

function normalizeLineEndings(text: string): string {
  return text.replace(/\r\n/g, "\n")
}

function splitDiffLines(text: string): string[] {
  if (!text) return []
  return normalizeLineEndings(text).split("\n")
}

function lowerBound(values: number[], target: number): number {
  let lo = 0
  let hi = values.length

  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (values[mid] < target) {
      lo = mid + 1
    } else {
      hi = mid
    }
  }

  return lo
}

function computeLcsMatches(
  baseLines: string[],
  currentLines: string[]
): Array<{ baseIndex: number; currentIndex: number }> | null {
  const MAX_MATCHES_PER_LINE = 256
  const MAX_TOKENS = 200_000
  const basePositions = new Map<string, number[]>()

  for (const [index, line] of baseLines.entries()) {
    const positions = basePositions.get(line)
    if (positions) {
      positions.push(index)
    } else {
      basePositions.set(line, [index])
    }
  }

  const tokens: Array<{ baseIndex: number; currentIndex: number }> = []
  for (const [currentIndex, line] of currentLines.entries()) {
    const positions = basePositions.get(line)
    if (!positions) continue
    if (positions.length > MAX_MATCHES_PER_LINE) continue
    for (let pos = positions.length - 1; pos >= 0; pos -= 1) {
      tokens.push({ baseIndex: positions[pos], currentIndex })
      if (tokens.length > MAX_TOKENS) return null
    }
  }

  if (tokens.length === 0) return []

  const tails: number[] = []
  const tailsTokenIndex: number[] = []
  const prevTokenIndex = Array<number>(tokens.length).fill(-1)

  for (const [tokenIndex, token] of tokens.entries()) {
    const len = lowerBound(tails, token.baseIndex)
    tails[len] = token.baseIndex
    tailsTokenIndex[len] = tokenIndex
    if (len > 0) {
      prevTokenIndex[tokenIndex] = tailsTokenIndex[len - 1]
    }
  }

  const matches: Array<{ baseIndex: number; currentIndex: number }> = []
  let cursor = tailsTokenIndex[tails.length - 1]
  while (cursor >= 0) {
    const token = tokens[cursor]
    matches.push({
      baseIndex: token.baseIndex,
      currentIndex: token.currentIndex,
    })
    cursor = prevTokenIndex[cursor]
  }

  matches.reverse()
  return matches
}

interface GitGutterLineMarkers {
  added: number[]
  modified: number[]
  deleted: number[]
}

const EMPTY_GIT_GUTTER_LINE_MARKERS: GitGutterLineMarkers = {
  added: [],
  modified: [],
  deleted: [],
}

function toSortedUniqueLineNumbers(lineNumbers: number[]): number[] {
  if (lineNumbers.length <= 1) return lineNumbers
  return [...new Set(lineNumbers)].sort((a, b) => a - b)
}

function appendLineRange(
  target: number[],
  startIndexInclusive: number,
  endIndexExclusive: number
) {
  for (let index = startIndexInclusive; index < endIndexExclusive; index += 1) {
    target.push(index + 1)
  }
}

function computeGitGutterLineMarkers(
  baseContent: string,
  currentContent: string
): GitGutterLineMarkers {
  const MAX_TOTAL_LINES = 20_000
  if (baseContent === currentContent) {
    return EMPTY_GIT_GUTTER_LINE_MARKERS
  }

  const baseLines = splitDiffLines(baseContent)
  const currentLines = splitDiffLines(currentContent)
  if (baseLines.length + currentLines.length > MAX_TOTAL_LINES) {
    return EMPTY_GIT_GUTTER_LINE_MARKERS
  }

  if (
    normalizeLineEndings(baseContent) === normalizeLineEndings(currentContent)
  ) {
    return EMPTY_GIT_GUTTER_LINE_MARKERS
  }
  if (baseLines.length === 0) {
    return {
      added: currentLines.map((_, index) => index + 1),
      modified: [],
      deleted: [],
    }
  }
  if (currentLines.length === 0) {
    return {
      added: [],
      modified: [],
      deleted: [1],
    }
  }

  const matches = computeLcsMatches(baseLines, currentLines)
  if (matches === null) {
    return EMPTY_GIT_GUTTER_LINE_MARKERS
  }
  const added: number[] = []
  const modified: number[] = []
  const deleted: number[] = []

  let previousBase = -1
  let previousCurrent = -1
  const sentinels = [
    ...matches,
    { baseIndex: baseLines.length, currentIndex: currentLines.length },
  ]

  for (const match of sentinels) {
    const baseGap = match.baseIndex - previousBase - 1
    const currentGap = match.currentIndex - previousCurrent - 1

    if (baseGap === 0 && currentGap > 0) {
      appendLineRange(added, previousCurrent + 1, match.currentIndex)
    } else if (baseGap > 0 && currentGap === 0) {
      const anchorLine = Math.max(
        1,
        Math.min(currentLines.length, match.currentIndex + 1)
      )
      deleted.push(anchorLine)
    } else if (baseGap > 0 && currentGap > 0) {
      appendLineRange(modified, previousCurrent + 1, match.currentIndex)
    }

    previousBase = match.baseIndex
    previousCurrent = match.currentIndex
  }

  return {
    added: toSortedUniqueLineNumbers(added),
    modified: toSortedUniqueLineNumbers(modified),
    deleted: toSortedUniqueLineNumbers(deleted),
  }
}

function DiffFileList({
  diffOutline,
  badge,
  description,
  onOpenDiff,
  openFilePreview,
}: {
  diffOutline: DiffOutline
  badge?: string | null
  description?: string | null
  onOpenDiff: (path: string) => Promise<void>
  openFilePreview: (path: string) => Promise<void>
}) {
  const t = useTranslations("Folder.fileWorkspacePanel")
  return (
    <div className="h-full flex flex-col min-h-0">
      <div className="border-b border-border bg-muted/25 px-3 py-2 space-y-1">
        <div className="text-[11px] text-muted-foreground flex items-center gap-3">
          {badge && (
            <span className="font-medium text-foreground/80 font-mono">
              {badge}
            </span>
          )}
          <span>{t("fileCount", { count: diffOutline.files.length })}</span>
          <span className="font-mono text-green-600 dark:text-green-400">
            +{diffOutline.totalAdditions}
          </span>
          <span className="font-mono text-red-600 dark:text-red-400">
            -{diffOutline.totalDeletions}
          </span>
        </div>
        {description && (
          <p className="text-xs text-foreground/70 line-clamp-2 leading-snug">
            {description}
          </p>
        )}
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto">
        <div className="py-1">
          {diffOutline.files.map((file) => (
            <ContextMenu key={file.key}>
              <ContextMenuTrigger asChild>
                <button
                  type="button"
                  className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-muted/50 transition-colors group"
                  onClick={() => {
                    void onOpenDiff(file.path)
                  }}
                  title={file.path}
                >
                  <FileIcon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span className="text-xs truncate flex-1 min-w-0 font-mono">
                    {file.path}
                  </span>
                  <span className="shrink-0 flex items-center gap-2 text-[10px] font-mono">
                    {file.additions > 0 && (
                      <span className="text-green-600 dark:text-green-400">
                        +{file.additions}
                      </span>
                    )}
                    {file.deletions > 0 && (
                      <span className="text-red-600 dark:text-red-400">
                        -{file.deletions}
                      </span>
                    )}
                  </span>
                </button>
              </ContextMenuTrigger>
              <ContextMenuContent>
                <ContextMenuItem
                  onSelect={() => {
                    void onOpenDiff(file.path)
                  }}
                >
                  {t("viewDiff")}
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() => {
                    void openFilePreview(file.path)
                  }}
                >
                  {t("openFile")}
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          ))}
        </div>
      </div>
    </div>
  )
}

export function FileWorkspacePanel() {
  const t = useTranslations("Folder.fileWorkspacePanel")
  const { folder } = useFolderContext()
  const folderPath = folder?.path ?? null
  const {
    activeFileTab,
    consumePendingFileReveal,
    pendingFileReveal,
    openBranchDiff,
    openCommitDiff,
    openFilePreview,
    openWorkingTreeDiff,
    saveActiveFile,
    updateActiveFileContent,
  } = useWorkspaceContext()
  const activeScope = activeFileTab?.id ?? "__default__"
  const editorRef = useRef<MonacoEditorNs.IStandaloneCodeEditor | null>(null)
  const cursorListenerRef = useRef<{ dispose: () => void } | null>(null)
  const gitChangeDecorationsRef = useRef<string[]>([])
  const editorTheme = useMonacoThemeSync()
  const [editorMountVersion, setEditorMountVersion] = useState(0)
  const [cursorLine, setCursorLine] = useState(1)
  const [collapsedFiles, setCollapsedFiles] = useState<Record<string, boolean>>(
    {}
  )
  const [collapsedHunks, setCollapsedHunks] = useState<Record<string, boolean>>(
    {}
  )
  const renderedContent = activeFileTab?.content ?? ""
  const isFileTab = activeFileTab?.kind === "file"
  const fileReadonly = isFileTab ? Boolean(activeFileTab.readonly) : true
  const fileTruncated = isFileTab ? Boolean(activeFileTab.truncated) : false
  const fileSaveState = isFileTab ? (activeFileTab.saveState ?? "idle") : "idle"
  const fileIsDirty = isFileTab ? Boolean(activeFileTab.isDirty) : false
  const canEdit = isFileTab && !fileReadonly && !fileTruncated
  const isMarkdownFile = isFileTab && activeFileTab?.language === "markdown"
  const activeFilePath = isFileTab ? activeFileTab?.path : null
  const streamdownLinkSafety = useStreamdownLinkSafety()
  const [, startMarkdownPreviewTransition] = useTransition()
  const markdownImageDataUrlCacheRef = useRef<Map<string, string>>(new Map())
  const markdownImageInflightRef = useRef<Map<string, Promise<string>>>(
    new Map()
  )
  const markdownImageNativePathByResolvedSrcRef = useRef<Map<string, string>>(
    new Map()
  )
  const autoSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const autoSaveGuardRef = useRef({
    canEdit: false,
    fileIsDirty: false,
    fileSaveState: "idle" as "idle" | "saving" | "error",
  })
  const diffListContext = useMemo<DiffListContext | null>(() => {
    if (!activeFileTab) return null
    if (activeFileTab.kind !== "diff") return null

    const commitMatch = activeFileTab.id.match(/^diff:commit:([^:]+):all$/)
    if (commitMatch) {
      return {
        kind: "commit",
        commitHash: commitMatch[1],
        commitMessage: activeFileTab.description,
      }
    }

    const workingOverviewMatch = activeFileTab.id.match(
      /^diff:working-overview:(.+)$/
    )
    if (workingOverviewMatch) {
      return {
        kind: "working",
        path: decodeDiffTabToken(workingOverviewMatch[1]),
      }
    }

    const branchOverviewMatch = activeFileTab.id.match(
      /^diff:branch-overview:([^:]+):(.+)$/
    )
    if (branchOverviewMatch) {
      return {
        kind: "branch",
        branch: decodeDiffTabToken(branchOverviewMatch[1]),
        path: decodeDiffTabToken(branchOverviewMatch[2]),
      }
    }

    return null
  }, [activeFileTab])
  const diffOutline = useMemo(() => {
    if (!activeFileTab || activeFileTab.kind !== "diff") return null
    return buildDiffOutline(activeFileTab.content)
  }, [activeFileTab])
  const allHunks = useMemo(
    () =>
      diffListContext
        ? []
        : (diffOutline?.files.flatMap((file) => file.hunks) ?? []),
    [diffListContext, diffOutline]
  )
  const activeHunkIndex = useMemo(() => {
    if (allHunks.length === 0) return -1

    const containingIndex = allHunks.findIndex(
      (hunk) => cursorLine >= hunk.startLine && cursorLine <= hunk.endLine
    )
    if (containingIndex >= 0) return containingIndex

    const firstAfterIndex = allHunks.findIndex(
      (hunk) => hunk.startLine > cursorLine
    )
    if (firstAfterIndex < 0) return allHunks.length - 1
    return firstAfterIndex - 1
  }, [allHunks, cursorLine])

  const lineNumbersMinChars = useMemo(() => {
    const lineCount = renderedContent.split("\n").length
    const digits = String(Math.max(1, lineCount)).length

    // Keep a one-character buffer so numbers don't visually hug the gutter edge.
    return Math.max(3, digits + 1)
  }, [renderedContent])

  const hasGitBaseSnapshot =
    isFileTab && activeFileTab?.gitBaseContent !== undefined
  const gitBaseContent = hasGitBaseSnapshot
    ? (activeFileTab?.gitBaseContent ?? "")
    : ""
  const gitGutterLineMarkers = useMemo(() => {
    if (!hasGitBaseSnapshot) return EMPTY_GIT_GUTTER_LINE_MARKERS
    return computeGitGutterLineMarkers(gitBaseContent, renderedContent)
  }, [gitBaseContent, hasGitBaseSnapshot, renderedContent])

  const applyGitChangeDecorations = useCallback(() => {
    const editorInstance = editorRef.current
    if (!editorInstance) return

    const { added, modified, deleted } = gitGutterLineMarkers

    if (
      !isFileTab ||
      (added.length === 0 && modified.length === 0 && deleted.length === 0)
    ) {
      gitChangeDecorationsRef.current = editorInstance.deltaDecorations(
        gitChangeDecorationsRef.current,
        []
      )
      return
    }

    const model = editorInstance.getModel()
    if (!model) return

    const maxLine = model.getLineCount()
    const toRange = (lineNumber: number) => ({
      startLineNumber: lineNumber,
      startColumn: 1,
      endLineNumber: lineNumber,
      endColumn: 1,
    })
    const addedDecorations = added
      .filter((lineNumber) => lineNumber >= 1 && lineNumber <= maxLine)
      .map((lineNumber) => ({
        range: toRange(lineNumber),
        options: {
          isWholeLine: true,
          linesDecorationsClassName:
            "codeg-dirty-diff-glyph codeg-dirty-diff-added",
        },
      }))
    const modifiedDecorations = modified
      .filter((lineNumber) => lineNumber >= 1 && lineNumber <= maxLine)
      .map((lineNumber) => ({
        range: toRange(lineNumber),
        options: {
          isWholeLine: true,
          linesDecorationsClassName:
            "codeg-dirty-diff-glyph codeg-dirty-diff-modified",
        },
      }))
    const deletedDecorations = deleted
      .filter((lineNumber) => lineNumber >= 1 && lineNumber <= maxLine)
      .map((lineNumber) => ({
        range: {
          startLineNumber: lineNumber,
          startColumn: Number.MAX_VALUE,
          endLineNumber: lineNumber,
          endColumn: Number.MAX_VALUE,
        },
        options: {
          isWholeLine: false,
          linesDecorationsClassName:
            "codeg-dirty-diff-glyph codeg-dirty-diff-deleted",
        },
      }))
    const decorations = [
      ...addedDecorations,
      ...modifiedDecorations,
      ...deletedDecorations,
    ]

    gitChangeDecorationsRef.current = editorInstance.deltaDecorations(
      gitChangeDecorationsRef.current,
      decorations
    )
  }, [gitGutterLineMarkers, isFileTab])

  const applyHiddenAreas = useCallback(() => {
    const editorInstance = editorRef.current
    if (!editorInstance) return

    if (!diffOutline || diffListContext) {
      setEditorHiddenAreas(editorInstance, [])
      return
    }

    const model = editorInstance.getModel()
    if (!model) return

    const maxLine = model.getLineCount()
    const ranges: {
      startLineNumber: number
      startColumn: number
      endLineNumber: number
      endColumn: number
    }[] = []

    const addRange = (startLine: number, endLine: number) => {
      const safeStart = Math.max(1, startLine)
      const safeEnd = Math.min(maxLine, endLine)
      if (safeStart > safeEnd) return

      ranges.push({
        startLineNumber: safeStart,
        startColumn: 1,
        endLineNumber: safeEnd,
        endColumn: 1,
      })
    }

    for (const file of diffOutline.files) {
      const fileCollapsed = Boolean(
        collapsedFiles[`${activeScope}:${file.key}`]
      )
      if (fileCollapsed) {
        addRange(file.startLine + 1, file.endLine)
        continue
      }

      for (const hunk of file.hunks) {
        if (!collapsedHunks[`${activeScope}:${hunk.key}`]) continue
        addRange(hunk.startLine + 1, hunk.endLine)
      }
    }

    setEditorHiddenAreas(editorInstance, ranges)
  }, [
    activeScope,
    collapsedFiles,
    collapsedHunks,
    diffListContext,
    diffOutline,
  ])

  const handleEditorMount = useCallback(
    (editorInstance: MonacoEditorNs.IStandaloneCodeEditor) => {
      editorRef.current = editorInstance
      cursorListenerRef.current?.dispose()
      cursorListenerRef.current = editorInstance.onDidChangeCursorPosition(
        (event) => {
          setCursorLine(event.position.lineNumber)
        }
      )
      setEditorMountVersion((prev) => prev + 1)
      setCursorLine(editorInstance.getPosition()?.lineNumber ?? 1)
      applyHiddenAreas()
      applyGitChangeDecorations()
    },
    [applyGitChangeDecorations, applyHiddenAreas]
  )

  const jumpToLine = useCallback((lineNumber: number) => {
    const editorInstance = editorRef.current
    if (!editorInstance) return false

    const model = editorInstance.getModel()
    if (!model) return false
    const maxLine = model.getLineCount()
    const targetLine = Math.min(Math.max(1, lineNumber), maxLine)

    editorInstance.revealLineInCenter(targetLine)
    editorInstance.setPosition({ lineNumber: targetLine, column: 1 })
    editorInstance.focus()
    return true
  }, [])

  const jumpToHunk = useCallback(
    (hunkIndex: number) => {
      const hunk = allHunks[hunkIndex]
      if (!hunk) return
      jumpToLine(hunk.startLine)
    },
    [allHunks, jumpToLine]
  )

  const handlePrevHunk = useCallback(() => {
    if (allHunks.length === 0) return
    if (activeHunkIndex <= 0) return
    jumpToHunk(activeHunkIndex - 1)
  }, [activeHunkIndex, allHunks.length, jumpToHunk])

  const handleNextHunk = useCallback(() => {
    if (allHunks.length === 0) return
    if (activeHunkIndex >= allHunks.length - 1) return
    jumpToHunk(activeHunkIndex + 1)
  }, [activeHunkIndex, allHunks.length, jumpToHunk])

  const toggleFileCollapsed = useCallback(
    (fileKey: string) => {
      setCollapsedFiles((prev) => {
        const scopedKey = `${activeScope}:${fileKey}`
        return {
          ...prev,
          [scopedKey]: !prev[scopedKey],
        }
      })
    },
    [activeScope]
  )

  const toggleHunkCollapsed = useCallback(
    (hunkKey: string) => {
      setCollapsedHunks((prev) => {
        const scopedKey = `${activeScope}:${hunkKey}`
        return {
          ...prev,
          [scopedKey]: !prev[scopedKey],
        }
      })
    },
    [activeScope]
  )

  useEffect(() => {
    applyHiddenAreas()
  }, [applyHiddenAreas])

  useEffect(() => {
    applyGitChangeDecorations()
  }, [activeFileTab?.id, applyGitChangeDecorations])

  useEffect(() => {
    if (!pendingFileReveal) return
    if (!isFileTab || !activeFileTab || activeFileTab.loading) return
    if (!activeFileTab.path) return
    if (
      normalizeWorkspacePath(activeFileTab.path) !==
      normalizeWorkspacePath(pendingFileReveal.path)
    ) {
      return
    }

    const jumped = jumpToLine(pendingFileReveal.line)
    if (!jumped) return

    consumePendingFileReveal(pendingFileReveal.requestId)
  }, [
    activeFileTab,
    consumePendingFileReveal,
    editorMountVersion,
    isFileTab,
    jumpToLine,
    pendingFileReveal,
  ])

  useEffect(() => {
    autoSaveGuardRef.current = {
      canEdit,
      fileIsDirty,
      fileSaveState,
    }
  }, [canEdit, fileIsDirty, fileSaveState])

  useEffect(() => {
    if (autoSaveTimerRef.current) {
      clearTimeout(autoSaveTimerRef.current)
      autoSaveTimerRef.current = null
    }

    if (!canEdit || !fileIsDirty || fileSaveState !== "idle") return

    autoSaveTimerRef.current = setTimeout(() => {
      const guard = autoSaveGuardRef.current
      if (
        !guard.canEdit ||
        !guard.fileIsDirty ||
        guard.fileSaveState !== "idle"
      ) {
        return
      }
      void saveActiveFile()
    }, AUTO_SAVE_DELAY_MS)

    return () => {
      if (autoSaveTimerRef.current) {
        clearTimeout(autoSaveTimerRef.current)
        autoSaveTimerRef.current = null
      }
    }
  }, [canEdit, fileIsDirty, fileSaveState, saveActiveFile, renderedContent])

  useEffect(() => {
    if (!isFileTab) return

    const saveOnDeactivation = () => {
      const guard = autoSaveGuardRef.current
      if (
        !guard.canEdit ||
        !guard.fileIsDirty ||
        guard.fileSaveState === "saving"
      ) {
        return
      }
      void saveActiveFile()
    }

    const onWindowBlur = () => {
      saveOnDeactivation()
    }

    const onVisibilityChange = () => {
      if (document.visibilityState !== "hidden") return
      saveOnDeactivation()
    }

    window.addEventListener("blur", onWindowBlur)
    document.addEventListener("visibilitychange", onVisibilityChange)
    return () => {
      window.removeEventListener("blur", onWindowBlur)
      document.removeEventListener("visibilitychange", onVisibilityChange)
    }
  }, [isFileTab, saveActiveFile])

  useEffect(() => {
    if (!isFileTab) return

    const onKeyDown = (event: KeyboardEvent) => {
      const isSaveShortcut =
        (event.metaKey || event.ctrlKey) && event.key === "s"
      if (!isSaveShortcut) return
      event.preventDefault()
      if (!canEdit) return
      void saveActiveFile()
    }

    window.addEventListener("keydown", onKeyDown)
    return () => {
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [canEdit, isFileTab, saveActiveFile])

  useEffect(
    () => () => {
      if (editorRef.current) {
        editorRef.current.deltaDecorations(gitChangeDecorationsRef.current, [])
      }
      gitChangeDecorationsRef.current = []
      cursorListenerRef.current?.dispose()
      cursorListenerRef.current = null
    },
    []
  )

  const [markdownModeByTab, setMarkdownModeByTab] = useState<
    Record<string, "edit" | "preview">
  >({})
  const [markdownPreviewContentByTab, setMarkdownPreviewContentByTab] =
    useState<Record<string, string>>({})

  const markdownMode = isMarkdownFile
    ? canEdit
      ? (markdownModeByTab[activeScope] ?? "preview")
      : "preview"
    : "edit"

  useEffect(() => {
    if (!isMarkdownFile) return
    setMarkdownPreviewContentByTab((prev) => {
      if (prev[activeScope] !== undefined) return prev
      return { ...prev, [activeScope]: renderedContent }
    })
  }, [activeScope, isMarkdownFile, renderedContent])

  useEffect(() => {
    if (!isMarkdownFile) return
    if (markdownMode !== "edit") return

    const timer = window.setTimeout(() => {
      startMarkdownPreviewTransition(() => {
        setMarkdownPreviewContentByTab((prev) => {
          if (prev[activeScope] === renderedContent) return prev
          return { ...prev, [activeScope]: renderedContent }
        })
      })
    }, 400)

    return () => {
      window.clearTimeout(timer)
    }
  }, [
    activeScope,
    isMarkdownFile,
    markdownMode,
    renderedContent,
    startMarkdownPreviewTransition,
  ])

  useEffect(() => {
    if (!isMarkdownFile) return
    if (markdownMode !== "preview") return

    startMarkdownPreviewTransition(() => {
      setMarkdownPreviewContentByTab((prev) => {
        if (prev[activeScope] === renderedContent) return prev
        return { ...prev, [activeScope]: renderedContent }
      })
    })
  }, [
    activeScope,
    isMarkdownFile,
    markdownMode,
    renderedContent,
    startMarkdownPreviewTransition,
  ])

  const getMarkdownImageDataUrl = useCallback(async (nativePath: string) => {
    const cached = markdownImageDataUrlCacheRef.current.get(nativePath)
    if (cached) return cached

    const inflight = markdownImageInflightRef.current.get(nativePath)
    if (inflight) return inflight

    const promise = (async () => {
      const mime = guessImageMimeType(nativePath)
      const base64 = await readFileBase64(nativePath, 10_000_000)
      const dataUrl = `data:${mime};base64,${base64}`
      markdownImageDataUrlCacheRef.current.set(nativePath, dataUrl)
      markdownImageInflightRef.current.delete(nativePath)
      return dataUrl
    })().catch((error) => {
      markdownImageInflightRef.current.delete(nativePath)
      throw error
    })

    markdownImageInflightRef.current.set(nativePath, promise)
    return promise
  }, [])

  const markdownRehypePlugins = useMemo<StreamdownRehypePlugins>(() => {
    const raw = defaultRehypePlugins.raw
    const hardenBase = defaultRehypePlugins.harden
    const sanitize = defaultRehypePlugins.sanitize

    const harden = Array.isArray(hardenBase)
      ? ((): unknown => {
          const [hardenPlugin, hardenOptions] = hardenBase
          if (!hardenOptions || typeof hardenOptions !== "object") {
            return hardenBase
          }

          const options = hardenOptions as {
            defaultOrigin?: string | undefined
            allowedProtocols?: string[] | undefined
            allowedImagePrefixes?: string[] | undefined
          }

          const nextOptions = {
            ...options,
            defaultOrigin: options.defaultOrigin ?? "http://localhost",
            allowedProtocols: uniqStrings([
              ...(options.allowedProtocols ?? []),
              "asset:",
              "tauri:",
            ]),
            allowedImagePrefixes: uniqStrings([
              ...(options.allowedImagePrefixes ?? []),
              "asset://localhost/",
              "tauri://localhost/",
            ]),
          }

          return [hardenPlugin, nextOptions]
        })()
      : hardenBase

    if (!Array.isArray(sanitize)) {
      return [raw, sanitize, harden] as unknown as StreamdownRehypePlugins
    }

    const [sanitizePlugin, sanitizeSchema] = sanitize
    if (!sanitizeSchema || typeof sanitizeSchema !== "object") {
      return [raw, sanitize, harden] as unknown as StreamdownRehypePlugins
    }

    const schema = sanitizeSchema as {
      protocols?: Record<string, string[] | undefined>
      attributes?: Record<string, string[] | undefined>
    }

    const nextSchema = {
      ...schema,
      protocols: {
        ...schema.protocols,
        src: uniqStrings([
          ...(schema.protocols?.src ?? []),
          "data",
          "asset",
          "tauri",
        ]),
      },
      attributes: {
        ...schema.attributes,
        img: uniqStrings([
          ...(schema.attributes?.img ?? []),
          "alt",
          "title",
          "width",
          "height",
        ]),
      },
    }

    return [
      raw,
      [sanitizePlugin, nextSchema],
      harden,
    ] as unknown as StreamdownRehypePlugins
  }, [])

  const markdownUrlTransform = useCallback<UrlTransform>(
    (rawUrl, key, node) => {
      if (!folderPath) return rawUrl
      if (!isMarkdownFile) return rawUrl
      if (!activeFilePath) return rawUrl
      if (key !== "src") return rawUrl
      if (node.tagName.toLowerCase() !== "img") return rawUrl

      const url = rawUrl.trim()
      if (!url) return rawUrl
      if (url.startsWith("data:") || url.startsWith("blob:")) return rawUrl
      if (url.startsWith("//")) return rawUrl
      if (URL_SCHEME.test(url) && !WINDOWS_ABSOLUTE_PATH.test(url)) {
        return rawUrl
      }

      const absoluteNativePath = resolveMarkdownImageNativePath({
        folderPath,
        activeFilePath,
        rawUrl: url,
      })
      if (!absoluteNativePath) return rawUrl

      try {
        const resolvedSrc = convertFileSrc(absoluteNativePath)
        markdownImageNativePathByResolvedSrcRef.current.set(
          resolvedSrc,
          absoluteNativePath
        )
        return resolvedSrc
      } catch {
        return rawUrl
      }
    },
    [activeFilePath, folderPath, isMarkdownFile]
  )

  const markdownComponents = useMemo<StreamdownComponents>(() => {
    const MarkdownImg = (
      props: ComponentPropsWithoutRef<"img"> & { node?: unknown }
    ) => {
      const rawSrc = typeof props.src === "string" ? props.src : ""
      const [src, setSrc] = useState(rawSrc)
      const loadedRef = useRef(false)
      const fallbackStartedRef = useRef(false)

      useEffect(() => {
        setSrc(rawSrc)
        loadedRef.current = false
        fallbackStartedRef.current = false
      }, [rawSrc])

      const startFallback = useCallback(() => {
        if (!rawSrc) return
        if (fallbackStartedRef.current) return
        fallbackStartedRef.current = true

        const nativePath =
          markdownImageNativePathByResolvedSrcRef.current.get(rawSrc)
        if (!nativePath) return

        void getMarkdownImageDataUrl(nativePath)
          .then((dataUrl) => {
            setSrc(dataUrl)
          })
          .catch(() => {
            fallbackStartedRef.current = false
          })
      }, [rawSrc])

      useEffect(() => {
        if (!rawSrc.startsWith("asset:") && !rawSrc.startsWith("tauri:")) {
          return
        }

        const timer = window.setTimeout(() => {
          if (loadedRef.current) return
          startFallback()
        }, 300)

        return () => {
          window.clearTimeout(timer)
        }
      }, [rawSrc, startFallback])

      const handleError = useCallback(
        (event: SyntheticEvent<HTMLImageElement>) => {
          props.onError?.(event)
          startFallback()
        },
        [props, startFallback]
      )

      const handleLoad = useCallback(
        (event: SyntheticEvent<HTMLImageElement>) => {
          loadedRef.current = true
          props.onLoad?.(event)
        },
        [props]
      )

      return (
        // eslint-disable-next-line @next/next/no-img-element
        <img
          {...props}
          alt={typeof props.alt === "string" ? props.alt : ""}
          src={src}
          onError={handleError}
          onLoad={handleLoad}
        />
      )
    }

    return { img: MarkdownImg }
  }, [getMarkdownImageDataUrl])

  if (!activeFileTab) {
    return (
      <div className="h-full flex flex-col items-center justify-center text-center px-6">
        <FileCode2 className="h-8 w-8 text-muted-foreground/60 mb-3" />
        <p className="text-sm text-muted-foreground">{t("openFileOrDiff")}</p>
      </div>
    )
  }

  if (activeFileTab.kind === "rich-diff") {
    const isCommitDiff = activeFileTab.id.startsWith("diff:commit:")
    const isExternalConflictDiff = activeFileTab.id.startsWith(
      "diff:external-conflict:"
    )
    const commitHash = isCommitDiff
      ? (activeFileTab.id.split(":")[2]?.slice(0, 7) ?? "")
      : ""
    const origLabel = isCommitDiff
      ? `${commitHash}~1`
      : isExternalConflictDiff
        ? t("disk")
        : t("head")
    const modLabel = isCommitDiff
      ? commitHash
      : isExternalConflictDiff
        ? t("unsaved")
        : t("workingTree")

    return (
      <div className="h-full relative">
        {activeFileTab.loading && (
          <div className="absolute top-2 right-3 z-10 rounded-md bg-background/70 px-2 py-1 text-[11px] text-muted-foreground backdrop-blur-sm">
            {t("loading")}
          </div>
        )}
        {!activeFileTab.loading && (
          <DiffViewer
            key={activeFileTab.id}
            original={activeFileTab.originalContent ?? ""}
            modified={activeFileTab.modifiedContent ?? ""}
            originalLabel={origLabel}
            modifiedLabel={modLabel}
            language={activeFileTab.language}
            className="h-full"
          />
        )}
      </div>
    )
  }

  if (
    activeFileTab.kind === "diff" &&
    activeFileTab.id.startsWith("diff:session:")
  ) {
    return (
      <div className="h-full relative">
        {activeFileTab.loading && (
          <div className="absolute top-2 right-3 z-10 rounded-md bg-background/70 px-2 py-1 text-[11px] text-muted-foreground backdrop-blur-sm">
            {t("loading")}
          </div>
        )}
        <UnifiedDiffPreview
          diffText={activeFileTab.content}
          modelId={activeFileTab.id}
          className="h-full"
        />
      </div>
    )
  }

  // Diff overview list view (commit / directory)
  if (diffListContext && diffOutline) {
    const badge =
      diffListContext.kind === "commit"
        ? diffListContext.commitHash.slice(0, 7)
        : diffListContext.kind === "branch"
          ? diffListContext.branch
          : t("workingTree")

    const description =
      diffListContext.kind === "commit"
        ? diffListContext.commitMessage
        : diffListContext.kind === "branch"
          ? t("compareWithBranch", {
              path: diffListContext.path,
              branch: diffListContext.branch,
            })
          : diffListContext.path

    const handleOpenDiff = async (path: string) => {
      if (diffListContext.kind === "commit") {
        await openCommitDiff(diffListContext.commitHash, path)
        return
      }

      if (diffListContext.kind === "branch") {
        await openBranchDiff(diffListContext.branch, path)
        return
      }

      await openWorkingTreeDiff(path)
    }

    return (
      <div className="h-full relative">
        {activeFileTab.loading && (
          <div className="absolute top-2 right-3 z-10 rounded-md bg-background/70 px-2 py-1 text-[11px] text-muted-foreground backdrop-blur-sm">
            {t("loading")}
          </div>
        )}
        {!activeFileTab.loading && (
          <DiffFileList
            diffOutline={diffOutline}
            badge={badge}
            description={description}
            onOpenDiff={handleOpenDiff}
            openFilePreview={openFilePreview}
          />
        )}
      </div>
    )
  }

  return (
    <div className="h-full relative">
      {activeFileTab.loading && (
        <div className="absolute top-2 right-3 z-10 rounded-md bg-background/70 px-2 py-1 text-[11px] text-muted-foreground backdrop-blur-sm">
          {t("loading")}
        </div>
      )}
      {isMarkdownFile && !activeFileTab.loading && (
        <div className="absolute top-2 right-3 z-10">
          <ButtonGroup className="rounded-4xl">
            <Button
              size="xs"
              variant={markdownMode === "edit" ? "secondary" : "outline"}
              onClick={() => {
                setMarkdownModeByTab((prev) => ({
                  ...prev,
                  [activeScope]: "edit",
                }))
              }}
              disabled={!canEdit}
              title="Edit"
              aria-label="Edit"
            >
              <Pencil className="h-3 w-3" />
              Edit
            </Button>
            <Button
              size="xs"
              variant={markdownMode === "preview" ? "secondary" : "outline"}
              onClick={() => {
                setMarkdownModeByTab((prev) => ({
                  ...prev,
                  [activeScope]: "preview",
                }))
                startMarkdownPreviewTransition(() => {
                  setMarkdownPreviewContentByTab((prev) => ({
                    ...prev,
                    [activeScope]: renderedContent,
                  }))
                })
              }}
              title="Preview"
              aria-label="Preview"
            >
              <Eye className="h-3 w-3" />
              Preview
            </Button>
          </ButtonGroup>
        </div>
      )}
      <div className="h-full flex flex-col min-h-0">
        {diffOutline && (
          <div className="border-b border-border bg-muted/25">
            <div className="px-3 py-1.5 text-[11px] text-muted-foreground flex items-center gap-3">
              <span>{t("fileCount", { count: diffOutline.files.length })}</span>
              <span className="font-mono text-green-600 dark:text-green-400">
                +{diffOutline.totalAdditions}
              </span>
              <span className="font-mono text-red-600 dark:text-red-400">
                -{diffOutline.totalDeletions}
              </span>
              {diffOutline.totalHunks > 0 && (
                <span>{t("hunkCount", { count: diffOutline.totalHunks })}</span>
              )}
              {allHunks.length > 0 && (
                <div className="ml-auto flex items-center gap-1">
                  <button
                    type="button"
                    onClick={handlePrevHunk}
                    disabled={activeHunkIndex <= 0}
                    className="rounded border border-border bg-background px-2 py-0.5 text-[10px] disabled:opacity-40 hover:bg-muted transition-colors inline-flex items-center gap-1"
                  >
                    <ChevronRight className="h-3 w-3 rotate-180" />
                    {t("prev")}
                  </button>
                  <button
                    type="button"
                    onClick={handleNextHunk}
                    disabled={
                      activeHunkIndex < 0 ||
                      activeHunkIndex >= allHunks.length - 1
                    }
                    className="rounded border border-border bg-background px-2 py-0.5 text-[10px] disabled:opacity-40 hover:bg-muted transition-colors inline-flex items-center gap-1"
                  >
                    {t("next")}
                    <ChevronRight className="h-3 w-3" />
                  </button>
                </div>
              )}
            </div>
            <div className="px-2 pb-2 space-y-1 max-h-52 overflow-y-auto">
              {diffOutline.files.map((file) => {
                const fileCollapsed = Boolean(
                  collapsedFiles[`${activeScope}:${file.key}`]
                )
                return (
                  <div
                    key={file.key}
                    className="rounded-md border border-border/80 bg-background/80"
                  >
                    <button
                      type="button"
                      onClick={() => toggleFileCollapsed(file.key)}
                      className="w-full px-2 py-1.5 text-[11px] flex items-center gap-1 hover:bg-muted/60 transition-colors"
                    >
                      <ChevronRight
                        className={`h-3 w-3 shrink-0 transition-transform ${
                          fileCollapsed ? "" : "rotate-90"
                        }`}
                      />
                      <span
                        className="font-mono text-left truncate"
                        title={file.path}
                        onClick={(event) => {
                          event.stopPropagation()
                          jumpToLine(file.startLine)
                        }}
                      >
                        {file.path}
                      </span>
                      <span className="ml-auto shrink-0 flex items-center gap-2 text-[10px]">
                        <span className="font-mono text-green-600 dark:text-green-400">
                          +{file.additions}
                        </span>
                        <span className="font-mono text-red-600 dark:text-red-400">
                          -{file.deletions}
                        </span>
                        <span>{file.hunks.length}h</span>
                      </span>
                    </button>
                    {!fileCollapsed && file.hunks.length > 0 && (
                      <div className="px-2 pb-2 space-y-1">
                        {file.hunks.map((hunk) => {
                          const hunkCollapsed = Boolean(
                            collapsedHunks[`${activeScope}:${hunk.key}`]
                          )
                          const isActive =
                            activeHunkIndex >= 0 &&
                            allHunks[activeHunkIndex]?.key === hunk.key

                          return (
                            <div
                              key={hunk.key}
                              className={`flex items-center gap-1 rounded border px-1.5 py-1 text-[10px] ${
                                isActive
                                  ? "border-primary/50 bg-primary/10"
                                  : "border-border/70 bg-muted/30"
                              }`}
                            >
                              <button
                                type="button"
                                onClick={() => toggleHunkCollapsed(hunk.key)}
                                className="inline-flex items-center gap-1 min-w-0 flex-1 text-left hover:opacity-80"
                                title={hunk.header}
                              >
                                <ChevronDown
                                  className={`h-3 w-3 shrink-0 transition-transform ${
                                    hunkCollapsed ? "-rotate-90" : ""
                                  }`}
                                />
                                <span className="font-mono truncate">
                                  {hunk.header}
                                </span>
                              </button>
                              <button
                                type="button"
                                onClick={() => jumpToLine(hunk.startLine)}
                                className="shrink-0 rounded border border-border bg-background px-1.5 py-0.5 hover:bg-muted transition-colors"
                                title={t("jumpToLine", {
                                  line: hunk.startLine,
                                })}
                              >
                                L{hunk.startLine}
                              </button>
                            </div>
                          )
                        })}
                      </div>
                    )}
                  </div>
                )
              })}
              {diffOutline.files.length === 0 && (
                <div className="text-[11px] text-muted-foreground px-1 py-0.5">
                  {t("noParsedDiffSections")}
                </div>
              )}
            </div>
          </div>
        )}
        <div className="flex-1 min-h-0 relative">
          {isMarkdownFile && (
            <div
              className={`absolute inset-0 h-full overflow-auto px-4 py-3 ${
                markdownMode === "preview" ? "" : "hidden"
              }`}
            >
              <div className="break-words text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
                {activeFileTab.loading &&
                !(
                  markdownPreviewContentByTab[activeScope] ?? renderedContent
                ).trim() ? (
                  <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
                    {t("loading")}
                  </div>
                ) : (
                  <Streamdown
                    plugins={streamdownPlugins}
                    components={markdownComponents}
                    linkSafety={streamdownLinkSafety}
                    urlTransform={markdownUrlTransform}
                    rehypePlugins={markdownRehypePlugins}
                  >
                    {markdownPreviewContentByTab[activeScope] ??
                      renderedContent}
                  </Streamdown>
                )}
              </div>
            </div>
          )}
          {(!isMarkdownFile || markdownMode !== "preview") && (
            <MonacoEditor
              beforeMount={defineMonacoThemes}
              onMount={handleEditorMount}
              path={buildMonacoModelPath(activeFileTab.path, activeFileTab.id)}
              value={renderedContent}
              onChange={(value) => {
                if (!isFileTab) return
                updateActiveFileContent(value ?? "")
              }}
              language={activeFileTab.language}
              theme={editorTheme}
              loading={
                <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
                  {t("loadingEditor")}
                </div>
              }
              options={{
                readOnly: !canEdit,
                minimap: { enabled: false },
                automaticLayout: true,
                fontSize: 13,
                lineNumbersMinChars,
                lineDecorationsWidth: 10,
                wordWrap: "off",
                scrollBeyondLastLine: false,
                scrollBeyondLastColumn: 8,
                renderLineHighlight: "line",
                scrollbar: {
                  horizontal: "auto",
                },
              }}
            />
          )}
        </div>
      </div>
    </div>
  )
}
