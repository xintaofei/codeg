"use client"

import {
  useState,
  useEffect,
  useLayoutEffect,
  useCallback,
  useRef,
} from "react"
import { useTranslations } from "next-intl"
import {
  ChevronRight,
  ChevronUp,
  FolderIcon,
  FolderOpenIcon,
  Home,
  Loader2,
} from "lucide-react"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"
import { getHomeDirectory, listDirectoryEntries } from "@/lib/api"
import { parentFsPath } from "@/lib/path-utils"
import type { DirectoryEntry } from "@/lib/types"

interface DirectoryBrowserDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSelect: (path: string) => void
  title?: string
  initialPath?: string
}

/**
 * Strip trailing separators (POSIX `/` or Windows `\`) so otherwise-equivalent
 * paths compare equal for the row highlight; an all-separator root is left
 * intact rather than collapsed away.
 */
const normalizePath = (path: string) => path.replace(/[/\\]+$/, "") || path

// Synchronous layout effect on the client (so the session/selection guards see
// the latest committed values before any pending async work resolves), but a
// passive effect during the static-export prerender to avoid the SSR warning.
const useIsomorphicLayoutEffect =
  typeof window !== "undefined" ? useLayoutEffect : useEffect

export function DirectoryBrowserDialog({
  open,
  onOpenChange,
  onSelect,
  title,
  initialPath,
}: DirectoryBrowserDialogProps) {
  const t = useTranslations("DirectoryBrowser")

  const [rootPath, setRootPath] = useState("")
  const [pathInput, setPathInput] = useState("")
  const [entries, setEntries] = useState<Map<string, DirectoryEntry[]>>(
    new Map()
  )
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(new Set())
  const [loading, setLoading] = useState<Set<string>>(new Set())
  const [error, setError] = useState<string | null>(null)
  const [confirming, setConfirming] = useState(false)

  const initialized = useRef(false)
  // Monotonic session id, bumped synchronously on every real open/close
  // transition. Async flows capture it before awaiting and discard their writes
  // when it changes, so a slow request from a previous open can't clobber — or
  // expose — state in a newer one. Guarding on the previous `open` keeps the
  // bump idempotent under StrictMode's mount-time effect replay (which would
  // otherwise bump again and strand the first init's in-flight writes).
  const sessionGen = useRef(0)
  const prevOpen = useRef(open)
  useIsomorphicLayoutEffect(() => {
    if (prevOpen.current !== open) {
      prevOpen.current = open
      sessionGen.current += 1
    }
  }, [open])
  // Monotonic navigation id. Each navigateTo() bumps it and the open-time init()
  // captures it, so within a single session a slower earlier navigation (or a
  // late init) can't overwrite the destination of a newer one — the latest user
  // intent always wins.
  const navSeq = useRef(0)
  // Latest committed pathInput, mirrored synchronously so a confirm validation
  // can tell the selection moved (within the session) before its check resolved.
  const pathInputRef = useRef(pathInput)
  useIsomorphicLayoutEffect(() => {
    pathInputRef.current = pathInput
  }, [pathInput])

  const loadEntries = useCallback(
    async (path: string): Promise<DirectoryEntry[] | null> => {
      // Already cached
      if (entries.has(path)) return entries.get(path)!

      const gen = sessionGen.current
      setLoading((prev) => new Set(prev).add(path))
      setError(null)
      try {
        const result = await listDirectoryEntries(path)
        // Skip writes if a close/reopen happened mid-flight — they belong to a
        // session that no longer exists and would surface stale data.
        if (gen === sessionGen.current) {
          setEntries((prev) => new Map(prev).set(path, result))
        }
        return result
      } catch {
        if (gen === sessionGen.current) setError(t("errorLoadingDir"))
        return null
      } finally {
        if (gen === sessionGen.current) {
          setLoading((prev) => {
            const next = new Set(prev)
            next.delete(path)
            return next
          })
        }
      }
    },
    [entries, t]
  )

  const navigateTo = useCallback(
    async (path: string) => {
      const gen = sessionGen.current
      const seq = (navSeq.current += 1)
      const result = await loadEntries(path)
      // Discard if a newer navigation started, or the load outlived its session
      // (close/reopen), so the most recent navigation wins.
      if (gen !== sessionGen.current || seq !== navSeq.current) return
      if (result !== null) {
        setRootPath(path)
        setPathInput(path)
        setExpandedPaths(new Set())
      }
    },
    [loadEntries]
  )

  // Initialize on open
  useEffect(() => {
    if (!open) {
      initialized.current = false
      return
    }
    if (initialized.current) return
    initialized.current = true

    const gen = sessionGen.current
    const seq = navSeq.current

    // Reset synchronously so a reopened dialog never shows — or lets the user
    // confirm — the previous session's path while the start dir is loading.
    setRootPath("")
    setPathInput(initialPath ?? "")
    setExpandedPaths(new Set())
    setEntries(new Map())
    setError(null)
    setLoading(new Set())
    setConfirming(false)

    const init = async () => {
      try {
        const startPath = initialPath || (await getHomeDirectory())
        // Drop these writes if a close/reopen superseded this init, or the user
        // already navigated somewhere else while the start dir was loading.
        if (gen !== sessionGen.current || seq !== navSeq.current) return
        setRootPath(startPath)
        setPathInput(startPath)
        setLoading(new Set([startPath]))

        const result = await listDirectoryEntries(startPath)
        if (gen !== sessionGen.current || seq !== navSeq.current) return
        setEntries(new Map([[startPath, result]]))
        setLoading(new Set())
      } catch {
        if (gen !== sessionGen.current || seq !== navSeq.current) return
        setError(t("errorLoadingDir"))
        setLoading(new Set())
      }
    }
    init()
  }, [open, initialPath, t])

  const handleToggleExpand = useCallback(
    async (path: string) => {
      if (expandedPaths.has(path)) {
        setExpandedPaths((prev) => {
          const next = new Set(prev)
          next.delete(path)
          return next
        })
        return
      }
      const gen = sessionGen.current
      await loadEntries(path)
      if (gen !== sessionGen.current) return
      // Functional update so two folders expanded concurrently compose instead
      // of overwriting each other with a stale snapshot.
      setExpandedPaths((prev) => new Set(prev).add(path))
    },
    [expandedPaths, loadEntries]
  )

  const handleSelect = useCallback((path: string) => {
    setPathInput(path)
  }, [])

  const handleConfirm = useCallback(async () => {
    const path = pathInput.trim()
    if (!path || confirming) return
    // Validate the path is a real, readable directory before committing.
    // Visited dirs (clicked rows / navigated roots) are served from the
    // entries cache, so this is instant in the common case; a typed path is
    // verified here and keeps the dialog open with an error on failure.
    const gen = sessionGen.current
    setConfirming(true)
    const result = await loadEntries(path)
    // A stale confirm from a previous open must not touch the new session's
    // state — not even its spinner — so bail before clearing `confirming`.
    if (gen !== sessionGen.current) return
    setConfirming(false)
    // Within the session, still bail if the selection moved while validating
    // (e.g. the user picked another directory after pressing Select).
    if (pathInputRef.current.trim() !== path) return
    if (result !== null) {
      onSelect(path)
      onOpenChange(false)
    }
  }, [pathInput, confirming, loadEntries, onSelect, onOpenChange])

  const handleNavigateUp = useCallback(() => {
    const parent = parentFsPath(pathInput.trim() || rootPath)
    if (!parent) return
    navigateTo(parent)
  }, [pathInput, rootPath, navigateTo])

  const handleGoHome = useCallback(async () => {
    const gen = sessionGen.current
    try {
      const home = await getHomeDirectory()
      if (gen !== sessionGen.current) return
      navigateTo(home)
    } catch {
      if (gen !== sessionGen.current) return
      setError(t("errorLoadingDir"))
    }
  }, [navigateTo, t])

  const handlePathInputKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && pathInput.trim()) {
        navigateTo(pathInput.trim())
      }
    },
    [pathInput, navigateTo]
  )

  const handleDoubleClick = useCallback(
    (path: string) => {
      onSelect(path)
      onOpenChange(false)
    },
    [onSelect, onOpenChange]
  )

  const renderEntries = (parentPath: string, depth: number) => {
    const children = entries.get(parentPath)
    const isLoading = loading.has(parentPath)

    if (isLoading) {
      return (
        <div
          className="flex items-center gap-2 py-2 text-sm text-muted-foreground"
          style={{ paddingLeft: `${depth * 20 + 8}px` }}
        >
          <Loader2 className="size-3.5 animate-spin" />
          <span>{t("loading")}</span>
        </div>
      )
    }

    if (!children) return null

    if (children.length === 0) {
      return (
        <div
          className="py-2 text-sm text-muted-foreground"
          style={{ paddingLeft: `${depth * 20 + 28}px` }}
        >
          {t("emptyDirectory")}
        </div>
      )
    }

    return children.map((entry) => {
      const isExpanded = expandedPaths.has(entry.path)
      const isSelected = normalizePath(entry.path) === normalizePath(pathInput)

      return (
        <div key={entry.path}>
          <button
            className={cn(
              "flex w-full items-center gap-1 rounded px-2 py-1 text-left text-sm transition-colors hover:bg-muted/50",
              isSelected && "bg-accent text-accent-foreground"
            )}
            style={{ paddingLeft: `${depth * 20 + 8}px` }}
            onClick={() => handleSelect(entry.path)}
            onDoubleClick={() => handleDoubleClick(entry.path)}
            type="button"
          >
            <span
              className="shrink-0 p-0.5"
              onClick={(e) => {
                e.stopPropagation()
                if (entry.hasChildren) {
                  handleToggleExpand(entry.path)
                }
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.stopPropagation()
                  if (entry.hasChildren) {
                    handleToggleExpand(entry.path)
                  }
                }
              }}
              role="button"
              tabIndex={0}
            >
              <ChevronRight
                className={cn(
                  "size-3.5 text-muted-foreground transition-transform",
                  isExpanded && "rotate-90",
                  !entry.hasChildren && "invisible"
                )}
              />
            </span>
            {isExpanded ? (
              <FolderOpenIcon className="size-4 shrink-0 text-blue-500" />
            ) : (
              <FolderIcon className="size-4 shrink-0 text-blue-500" />
            )}
            <span className="truncate">{entry.name}</span>
          </button>
          {isExpanded && renderEntries(entry.path, depth + 1)}
        </div>
      )
    })
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{title ?? t("title")}</DialogTitle>
        </DialogHeader>

        <div className="space-y-3">
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="icon"
              className="size-8 shrink-0"
              onClick={handleGoHome}
              title={t("goHome")}
              type="button"
            >
              <Home className="size-4" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="size-8 shrink-0"
              onClick={handleNavigateUp}
              title={t("navigateUp")}
              type="button"
            >
              <ChevronUp className="size-4" />
            </Button>
            <Input
              value={pathInput}
              onChange={(e) => setPathInput(e.target.value)}
              onKeyDown={handlePathInputKeyDown}
              placeholder={t("pathPlaceholder")}
              className="flex-1 h-8 text-sm font-mono"
            />
          </div>

          <ScrollArea className="h-[300px] rounded-md border">
            <div className="p-1">
              {renderEntries(rootPath, 0)}
              {error && !loading.size && (
                <div className="p-4 text-center text-sm text-destructive">
                  {error}
                </div>
              )}
            </div>
          </ScrollArea>
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            type="button"
          >
            {t("cancel")}
          </Button>
          <Button
            onClick={handleConfirm}
            disabled={!pathInput.trim() || confirming}
            type="button"
          >
            {t("select")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
