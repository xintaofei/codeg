"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import dynamic from "next/dynamic"
import { ChevronDown, ChevronRight, FileCode2, FileIcon } from "lucide-react"
import type { editor as MonacoEditorNs } from "monaco-editor"
import { useTranslations } from "next-intl"
import { useFolderContext } from "@/contexts/folder-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { ImagePreview } from "@/components/files/image-preview"
import { DiffViewer } from "@/components/diff/diff-viewer"
import { UnifiedDiffPreview } from "@/components/diff/unified-diff-preview"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import { cjk } from "@streamdown/cjk"
import { code } from "@streamdown/code"
import { createMathPlugin } from "@streamdown/math"
import { mermaid } from "@streamdown/mermaid"
import { Streamdown } from "streamdown"
import { readFileBase64 } from "@/lib/api"
import { defineMonacoThemes, useMonacoThemeSync } from "@/lib/monaco-themes"
import "@/lib/monaco-local"

const math = createMathPlugin({ singleDollarTextMath: true })
const previewPlugins = { cjk, code, math, mermaid }

function resolveRelativePath(base: string, relative: string): string {
  // Strip URL fragment (e.g. #gh-light-mode-only) and query string
  const cleaned = relative.replace(/[#?].*$/, "")
  // Preserve leading "/" for absolute paths, filter empty segments
  const isAbsolute = base.startsWith("/")
  const parts = base.split("/").filter(Boolean)
  for (const seg of cleaned.split("/")) {
    if (seg === "..") {
      if (parts.length > 0) parts.pop()
    } else if (seg !== "." && seg !== "") {
      parts.push(seg)
    }
  }
  return (isAbsolute ? "/" : "") + parts.join("/")
}

/**
 * Pre-resolve relative paths in markdown image/link syntax before Streamdown.
 *
 * rehype-harden resolves "../foo" via `new URL("../foo", "http://example.com")`
 * which loses directory context (e.g. "../images/a.png" from "docs/readme/"
 * becomes "/images/a.png" instead of "/docs/images/a.png").
 *
 * This function resolves relative paths against the file's directory BEFORE
 * Streamdown processes them, using "./" prefix so rehype-harden preserves them.
 */
function preprocessMarkdownPaths(
  content: string,
  relativeFileDir: string
): string {
  const resolveUrl = (url: string): string => {
    // Skip absolute URLs, anchors, and already-root-relative paths
    if (/^https?:\/\/|^data:|^blob:|^#|^\//.test(url)) return url
    // Separate fragment/query from path
    const fragIdx = url.search(/[#?]/)
    const pathPart = fragIdx >= 0 ? url.slice(0, fragIdx) : url
    const fragment = fragIdx >= 0 ? url.slice(fragIdx) : ""
    // Resolve relative to file directory within project
    const parts = relativeFileDir.split("/").filter(Boolean)
    for (const seg of pathPart.split("/")) {
      if (seg === "..") {
        if (parts.length > 0) parts.pop()
      } else if (seg !== "." && seg !== "") {
        parts.push(seg)
      }
    }
    // "./" prefix ensures rehype-harden recognizes it as relative
    return "./" + parts.join("/") + fragment
  }

  // Pre-resolve image paths: ![alt](url) or ![alt](url "title")
  let result = content.replace(
    /!\[([^\]]*)\]\(([^)\s"']+)([^)]*)\)/g,
    (match, alt, url, rest) => {
      const resolved = resolveUrl(url)
      if (resolved === url) return match
      return `![${alt}](${resolved}${rest})`
    }
  )

  // Pre-resolve image-wrapped link paths: [![alt](img)](url)
  result = result.replace(
    /\[(!\[[^\]]*\]\([^)]*\))\]\(([^)\s"']+)([^)]*)\)/g,
    (match, imgPart, url, rest) => {
      const resolved = resolveUrl(url)
      if (resolved === url) return match
      return `[${imgPart}](${resolved}${rest})`
    }
  )

  // Pre-resolve link paths: [text](url) — negative lookbehind to skip images
  result = result.replace(
    /(?<!!)\[([^\]]*)\]\(([^)\s"']+)([^)]*)\)/g,
    (match, text, url, rest) => {
      const resolved = resolveUrl(url)
      if (resolved === url) return match
      return `[${text}](${resolved}${rest})`
    }
  )

  // Pre-resolve HTML <a href="..."> and <img src="..."> tags
  result = result.replace(
    /<(a\s[^>]*?href|img\s[^>]*?src)=(["'])([^"']+)\2/gi,
    (match, prefix, quote, url) => {
      const resolved = resolveUrl(url)
      if (resolved === url) return match
      return `<${prefix}=${quote}${resolved}${quote}`
    }
  )

  return result
}

const MIME_BY_EXT: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  svg: "image/svg+xml",
  webp: "image/webp",
  bmp: "image/bmp",
  ico: "image/x-icon",
}

function useLocalImageSrc(
  src: string | undefined,
  fileDir: string | null,
  folderPath: string | null
): string | undefined {
  const [dataUrl, setDataUrl] = useState<string | undefined>(undefined)

  const isLocal = src && fileDir && !/^https?:\/\/|^data:|^blob:/.test(src)

  useEffect(() => {
    if (!isLocal || !src || !fileDir) return
    let cancelled = false
    // rehype-harden resolves "../foo" to "/foo" via new URL(src, "http://example.com")
    // Root-relative paths (starting with "/") should resolve against folderPath
    const absPath =
      src.startsWith("/") && folderPath
        ? resolveRelativePath(folderPath, src)
        : resolveRelativePath(fileDir, src)
    const ext = absPath.split(".").pop()?.toLowerCase() ?? ""
    const mime = MIME_BY_EXT[ext] ?? "image/png"

    readFileBase64(absPath)
      .then((b64) => {
        if (!cancelled) {
          setDataUrl(`data:${mime};base64,${b64}`)
        }
      })
      .catch((err) => {
        console.error(
          `[PreviewImage] readFileBase64 failed for "${absPath}":`,
          typeof err === "object" ? JSON.stringify(err) : err
        )
      })
    return () => {
      cancelled = true
    }
  }, [isLocal, src, fileDir, folderPath])

  if (!isLocal) return src
  return dataUrl
}

function PreviewImage({
  fileDir,
  folderPath,
  ...props
}: React.ComponentProps<"img"> & {
  fileDir: string | null
  folderPath: string | null
}) {
  const src = typeof props.src === "string" ? props.src : undefined
  const resolvedSrc = useLocalImageSrc(src, fileDir, folderPath)

  // eslint-disable-next-line @next/next/no-img-element, jsx-a11y/alt-text
  return <img {...props} src={resolvedSrc} />
}

const AUTO_SAVE_DELAY_MS = 5000

function buildMonacoModelPath(path: string | null, id: string): string {
  if (!path) return `inmemory://model/${encodeURIComponent(id)}`
  const normalized = path.replace(/\\/g, "/")
  const encoded = normalized.split("/").map(encodeURIComponent).join("/")
  return `file:///${encoded}`
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
  const {
    activeFileTab,
    consumePendingFileReveal,
    pendingFileReveal,
    openBranchDiff,
    openCommitDiff,
    openFilePreview,
    openWorkingTreeDiff,
    previewFileTabIds,
    saveActiveFile,
    updateActiveFileContent,
  } = useWorkspaceContext()
  const { folder } = useFolderContext()
  const folderPath = folder?.path ?? null
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
  const fileSaveState = isFileTab ? (activeFileTab.saveState ?? "idle") : "idle"
  const fileIsDirty = isFileTab ? Boolean(activeFileTab.isDirty) : false
  const canEdit = isFileTab && !fileReadonly
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
          className="h-full p-3"
        />
      </div>
    )
  }

  // Preview mode for markdown files
  const isPreviewMode =
    isFileTab &&
    activeFileTab &&
    previewFileTabIds.has(activeFileTab.id) &&
    activeFileTab.language === "markdown"

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
          : (activeFileTab.description ?? diffListContext.path)

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

  // Image preview
  if (isFileTab && activeFileTab && activeFileTab.language === "image") {
    return <ImagePreview key={activeFileTab.id} tab={activeFileTab} />
  }

  if (isPreviewMode && activeFileTab) {
    const absFilePath =
      activeFileTab.path && folderPath
        ? `${folderPath}/${activeFileTab.path}`
        : null
    const fileDir = absFilePath
      ? absFilePath.replace(/\/[^/]*$/, "")
      : folderPath
    // Pre-resolve relative paths before Streamdown/rehype-harden mangles them
    const relativeFileDir = activeFileTab.path?.includes("/")
      ? activeFileTab.path.replace(/\/[^/]*$/, "")
      : ""
    const preprocessedContent = preprocessMarkdownPaths(
      renderedContent,
      relativeFileDir
    )

    return (
      <div className="h-full relative">
        {activeFileTab.loading && (
          <div className="absolute top-2 right-3 z-10 rounded-md bg-background/70 px-2 py-1 text-[11px] text-muted-foreground backdrop-blur-sm">
            {t("loading")}
          </div>
        )}
        <div className="h-full overflow-auto p-6 [&_a_img]:inline">
          <Streamdown
            plugins={previewPlugins}
            components={{
              // eslint-disable-next-line @typescript-eslint/no-unused-vars
              img: ({ node, ...imgProps }) => (
                <PreviewImage
                  {...imgProps}
                  fileDir={fileDir}
                  folderPath={folderPath}
                />
              ),
              // eslint-disable-next-line @typescript-eslint/no-unused-vars
              a: ({ node, href, children, ...aProps }) => {
                const isRelative = href && !/^[a-z][a-z0-9+.-]*:|^#/i.test(href)
                if (isRelative && href) {
                  return (
                    <a
                      {...aProps}
                      href="#"
                      onClick={(e) => {
                        e.preventDefault()
                        // After preprocessing + rehype-harden, paths are
                        // root-relative like "/docs/images/foo.png"
                        const clean = href.replace(/[#?].*$/, "")
                        const target = clean
                          .replace(/^\/+/, "")
                          .replace(/\/\/+/g, "/")
                        openFilePreview(target)
                      }}
                    >
                      {children}
                    </a>
                  )
                }
                return (
                  <a
                    {...aProps}
                    href={href}
                    target="_blank"
                    rel="noopener noreferrer"
                  >
                    {children}
                  </a>
                )
              },
            }}
          >
            {preprocessedContent}
          </Streamdown>
        </div>
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
        <div className="flex-1 min-h-0">
          {activeFileTab.loading ? (
            <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
              {t("loadingEditor")}
            </div>
          ) : (
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
