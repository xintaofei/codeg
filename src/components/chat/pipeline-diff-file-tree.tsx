"use client"

import { useId, useMemo, useState } from "react"
import {
  ChevronDown,
  ChevronRight,
  FileCode,
  Folder,
  FolderOpen,
  MessageSquare,
  Search,
} from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { countNotesByFile, type PipelineLineNote } from "@/lib/pipeline-notes"
import type { PipelineDiffFile } from "@/lib/types"
import { cn } from "@/lib/utils"

export interface PipelineDiffFileTreeProps {
  files: PipelineDiffFile[]
  selectedPath?: string | null
  onSelectFile: (path: string) => void
  notes?: PipelineLineNote[]
  className?: string
  searchPlaceholder?: string
}

type TreeFileNode = {
  kind: "file"
  name: string
  path: string
  file: PipelineDiffFile
  notesCount: number
}

type TreeFolderNode = {
  kind: "folder"
  name: string
  path: string
  children: TreeNode[]
  totalAdditions: number
  totalDeletions: number
  totalNotesCount: number
}

type TreeNode = TreeFolderNode | TreeFileNode

function getStatusStyle(status: PipelineDiffFile["status"]): {
  label: string
  badgeClass: string
} {
  switch (status) {
    case "A":
      return {
        label: "A",
        badgeClass:
          "bg-green-500/15 text-green-700 dark:text-green-400 border-green-500/30",
      }
    case "D":
      return {
        label: "D",
        badgeClass:
          "bg-red-500/15 text-red-700 dark:text-red-400 border-red-500/30",
      }
    case "R":
      return {
        label: "R",
        badgeClass:
          "bg-purple-500/15 text-purple-700 dark:text-purple-400 border-purple-500/30",
      }
    case "M":
    default:
      return {
        label: "M",
        badgeClass:
          "bg-blue-500/15 text-blue-700 dark:text-blue-400 border-blue-500/30",
      }
  }
}

function buildTree(
  files: PipelineDiffFile[],
  notesMap: Record<string, number>
): TreeNode[] {
  type DirBuilder = {
    name: string
    path: string
    dirs: Map<string, DirBuilder>
    files: TreeFileNode[]
  }

  const root: DirBuilder = {
    name: "",
    path: "",
    dirs: new Map(),
    files: [],
  }

  for (const file of files) {
    const parts = file.path.split("/").filter(Boolean)
    if (parts.length === 0) continue

    let current = root
    let currentPath = ""

    for (let i = 0; i < parts.length; i += 1) {
      const part = parts[i] ?? ""
      const isLeaf = i === parts.length - 1
      currentPath = currentPath ? `${currentPath}/${part}` : part

      if (isLeaf) {
        current.files.push({
          kind: "file",
          name: part,
          path: file.path,
          file,
          notesCount: notesMap[file.path] ?? 0,
        })
      } else {
        let next = current.dirs.get(part)
        if (!next) {
          next = {
            name: part,
            path: currentPath,
            dirs: new Map(),
            files: [],
          }
          current.dirs.set(part, next)
        }
        current = next
      }
    }
  }

  function convert(builder: DirBuilder): TreeNode[] {
    const folders: TreeFolderNode[] = []
    for (const sub of builder.dirs.values()) {
      const children = convert(sub)
      let totalAdditions = 0
      let totalDeletions = 0
      let totalNotesCount = 0

      for (const child of children) {
        if (child.kind === "file") {
          totalAdditions += child.file.additions
          totalDeletions += child.file.deletions
          totalNotesCount += child.notesCount
        } else {
          totalAdditions += child.totalAdditions
          totalDeletions += child.totalDeletions
          totalNotesCount += child.totalNotesCount
        }
      }

      folders.push({
        kind: "folder",
        name: sub.name,
        path: sub.path,
        children,
        totalAdditions,
        totalDeletions,
        totalNotesCount,
      })
    }

    folders.sort((a, b) => a.name.localeCompare(b.name))
    builder.files.sort((a, b) => a.name.localeCompare(b.name))

    return [...folders, ...builder.files]
  }

  return convert(root)
}

function collectAllFolderPaths(nodes: TreeNode[]): string[] {
  const paths: string[] = []
  for (const node of nodes) {
    if (node.kind === "folder") {
      paths.push(node.path)
      paths.push(...collectAllFolderPaths(node.children))
    }
  }
  return paths
}

export function PipelineDiffFileTree({
  files,
  selectedPath,
  onSelectFile,
  notes = [],
  className,
  searchPlaceholder = "Filter files...",
}: PipelineDiffFileTreeProps) {
  const [searchQuery, setSearchQuery] = useState("")
  const searchInputId = useId()

  const notesMap = useMemo(() => countNotesByFile(notes), [notes])

  const filteredFiles = useMemo(() => {
    const q = searchQuery.trim().toLowerCase()
    if (!q) return files
    return files.filter((f) => f.path.toLowerCase().includes(q))
  }, [files, searchQuery])

  const treeNodes = useMemo(
    () => buildTree(filteredFiles, notesMap),
    [filteredFiles, notesMap]
  )

  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(() => {
    // Expand all folders by default so all files are quickly visible
    return new Set(collectAllFolderPaths(buildTree(files, notesMap)))
  })

  const toggleFolder = (folderPath: string) => {
    setExpandedFolders((prev) => {
      const next = new Set(prev)
      if (next.has(folderPath)) {
        next.delete(folderPath)
      } else {
        next.add(folderPath)
      }
      return next
    })
  }

  const totalAdditions = useMemo(
    () => files.reduce((acc, f) => acc + (f.additions || 0), 0),
    [files]
  )
  const totalDeletions = useMemo(
    () => files.reduce((acc, f) => acc + (f.deletions || 0), 0),
    [files]
  )
  const totalNotes = useMemo(
    () => Object.values(notesMap).reduce((acc, c) => acc + c, 0),
    [notesMap]
  )

  const renderNodes = (nodes: TreeNode[], depth = 0) => {
    return nodes.map((node) => {
      if (node.kind === "folder") {
        const isExpanded = expandedFolders.has(node.path)
        return (
          <div key={node.path} className="flex flex-col select-none">
            <button
              type="button"
              onClick={() => toggleFolder(node.path)}
              style={{ paddingLeft: `${depth * 0.75 + 0.5}rem` }}
              className="flex items-center gap-1.5 py-1 pr-2 text-xs font-medium text-muted-foreground hover:bg-muted/50 rounded-sm transition-colors text-left w-full"
              aria-expanded={isExpanded}
            >
              {isExpanded ? (
                <ChevronDown className="h-3.5 w-3.5 shrink-0" />
              ) : (
                <ChevronRight className="h-3.5 w-3.5 shrink-0" />
              )}
              {isExpanded ? (
                <FolderOpen className="h-3.5 w-3.5 shrink-0 text-amber-500/80" />
              ) : (
                <Folder className="h-3.5 w-3.5 shrink-0 text-amber-500/80" />
              )}
              <span className="truncate flex-1 font-mono">{node.name}</span>
              {node.totalNotesCount > 0 && (
                <span
                  title={`${node.totalNotesCount} notes in folder`}
                  className="inline-flex items-center gap-0.5 px-1.5 py-0.2 bg-amber-500/20 text-amber-700 dark:text-amber-300 rounded text-2xs font-sans"
                >
                  <MessageSquare className="h-2.5 w-2.5" />
                  {node.totalNotesCount}
                </span>
              )}
            </button>
            {isExpanded && (
              <div className="flex flex-col">
                {renderNodes(node.children, depth + 1)}
              </div>
            )}
          </div>
        )
      }

      const isSelected = selectedPath === node.path
      const statusStyle = getStatusStyle(node.file.status)

      return (
        <button
          key={node.path}
          type="button"
          onClick={() => onSelectFile(node.path)}
          style={{ paddingLeft: `${depth * 0.75 + 0.5}rem` }}
          className={cn(
            "flex items-center gap-1.5 py-1.5 pr-2 text-xs text-left w-full rounded-sm transition-colors group select-none",
            isSelected
              ? "bg-accent text-accent-foreground font-medium"
              : "text-foreground hover:bg-muted/60"
          )}
          aria-pressed={isSelected}
          data-testid={`file-tree-item-${node.path}`}
        >
          <FileCode className="h-3.5 w-3.5 shrink-0 text-muted-foreground group-hover:text-foreground" />
          <span className="truncate flex-1 font-mono text-2xs">
            {node.name}
          </span>

          <div className="flex items-center gap-1.5 shrink-0">
            {node.notesCount > 0 && (
              <span
                data-testid={`file-note-badge-${node.path}`}
                title={`${node.notesCount} notes for coder`}
                className="inline-flex items-center gap-0.5 px-1 py-0.2 bg-amber-500/20 text-amber-700 dark:text-amber-300 rounded text-2xs font-sans"
              >
                <MessageSquare className="h-2.5 w-2.5" />
                {node.notesCount}
              </span>
            )}

            <span
              dir="ltr"
              className="inline-flex items-center gap-1 font-mono text-2xs"
            >
              {node.file.additions > 0 && (
                <span className="text-green-600 dark:text-green-400">
                  +{node.file.additions}
                </span>
              )}
              {node.file.deletions > 0 && (
                <span className="text-red-600 dark:text-red-400">
                  -{node.file.deletions}
                </span>
              )}
            </span>

            <Badge
              variant="outline"
              className={cn(
                "h-4 min-w-4 px-1 text-2xs font-mono font-bold leading-none uppercase shrink-0 border",
                statusStyle.badgeClass
              )}
            >
              {statusStyle.label}
            </Badge>
          </div>
        </button>
      )
    })
  }

  return (
    <div
      className={cn(
        "flex flex-col h-full border-r border-border bg-card/50 text-card-foreground",
        className
      )}
    >
      {/* Header with stats and search */}
      <div className="p-2 space-y-2 border-b border-border/60 shrink-0">
        <div className="flex items-center justify-between text-2xs text-muted-foreground font-mono">
          <span>{files.length} files</span>
          <div className="flex items-center gap-2">
            {totalNotes > 0 && (
              <span className="inline-flex items-center gap-1 text-amber-600 dark:text-amber-400 font-sans">
                <MessageSquare className="h-3 w-3" />
                {totalNotes}
              </span>
            )}
            <span className="text-green-600 dark:text-green-400">
              +{totalAdditions}
            </span>
            <span className="text-red-600 dark:text-red-400">
              -{totalDeletions}
            </span>
          </div>
        </div>

        {files.length > 5 && (
          <div className="relative">
            <Search className="absolute left-2 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground pointer-events-none" />
            <Input
              id={searchInputId}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder={searchPlaceholder}
              aria-label={searchPlaceholder}
              className="h-7 text-2xs pl-7 py-1 bg-background/80"
            />
          </div>
        )}
      </div>

      {/* Tree scroll view */}
      <ScrollArea className="flex-1 min-h-0" x="scroll">
        <div className="p-1 space-y-0.5">
          {treeNodes.length === 0 ? (
            <div className="p-3 text-center text-xs text-muted-foreground">
              No matching files
            </div>
          ) : (
            renderNodes(treeNodes)
          )}
        </div>
      </ScrollArea>
    </div>
  )
}
