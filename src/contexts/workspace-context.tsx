"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { useTranslations } from "next-intl"
import { useActiveFolder } from "@/contexts/active-folder-context"
import {
  gitDiff,
  gitDiffWithBranch,
  gitIsTracked,
  gitShowDiff,
  gitShowFile,
  readFileBase64,
  readFileForEdit,
  readFilePreview,
  saveFileContent,
} from "@/lib/api"
import type { FileEditContent } from "@/lib/types"
import { languageFromPath } from "@/lib/language-detect"
import { toErrorMessage } from "@/lib/app-error"

export type WorkspaceMode = "conversation" | "fusion"
export type WorkspacePane = "conversation" | "files"

type FileWorkspaceTabKind = "file" | "diff" | "rich-diff"
type FileSaveState = "idle" | "saving" | "error"
type LineEnding = "lf" | "crlf" | "mixed" | "none"

export interface FileWorkspaceTab {
  id: string
  kind: FileWorkspaceTabKind
  title: string
  description: string | null
  path: string | null
  language: string
  content: string
  loading: boolean
  originalContent?: string
  modifiedContent?: string
  gitBaseContent?: string
  savedContent?: string
  isDirty?: boolean
  etag?: string | null
  mtimeMs?: number | null
  readonly?: boolean
  lineEnding?: LineEnding
  saveState?: FileSaveState
  saveError?: string | null
  // True iff an external change to this tab's path was observed by the
  // workspace watcher while the tab was inactive or otherwise not yet
  // resolved against disk. Cleared by any successful content reload.
  stale?: boolean
}

interface WorkspaceContextValue {
  mode: WorkspaceMode
  activePane: WorkspacePane
  setActivePane: (pane: WorkspacePane) => void
  activateConversationPane: () => void
  activateFilePane: () => void
  fileTabs: FileWorkspaceTab[]
  activeFileTabId: string | null
  activeFileTab: FileWorkspaceTab | null
  activeFilePath: string | null
  switchFileTab: (tabId: string) => void
  closeFileTab: (tabId: string) => void
  closeOtherFileTabs: (tabId: string) => void
  closeAllFileTabs: () => void
  reorderFileTabs: (tabs: FileWorkspaceTab[]) => void
  openFilePreview: (
    path: string,
    options?: { line?: number; reload?: boolean }
  ) => Promise<void>
  // Refetch the open tab matching `path` without changing activeFileTabId.
  // No-op when no tab matches or when the tab has unsaved local edits
  // (use markTabsStale for that case).
  reloadOpenFileBackground: (path: string) => Promise<void>
  // Write prefetched file content into the open tab matching `path` without
  // issuing a second readFileForEdit. Used by the change-detection watcher
  // whose resolver has already paid for the read — avoids the I/O double
  // when many tabs are affected by a single workspace event. Skips dirty
  // tabs and tabs that aren't open.
  applyExternalReload: (path: string, fetched: FileEditContent) => Promise<void>
  // Flip stale=true on the tab matching `path`. Activating a stale tab
  // forces a refetch (clean) or triggers conflict resolution (dirty).
  markTabsStale: (path: string) => void
  // Mark a clean open tab as load-failed, replacing its body with the
  // supplied error message and routing it into the editor's error state.
  // No-op when no tab matches OR when the tab is dirty — unsaved edits
  // must never be silently clobbered. Used by the watcher when a workspace
  // event reports a path whose disk read fails (external delete, locked,
  // permission revoked, …), so the user is never shown a stale buffer that
  // no longer corresponds to disk.
  rejectFileTab: (path: string, errorMessage: string) => void
  pendingFileReveal: {
    requestId: number
    path: string
    line: number
  } | null
  consumePendingFileReveal: (requestId: number) => void
  openWorkingTreeDiff: (
    path?: string,
    options?: { mode?: "auto" | "unified" | "overview" }
  ) => Promise<void>
  openBranchDiff: (
    branch: string,
    path?: string,
    options?: { mode?: "default" | "overview" }
  ) => Promise<void>
  openCommitDiff: (
    commit: string,
    path?: string,
    message?: string
  ) => Promise<void>
  openSessionFileDiff: (
    filePath: string,
    diffContent: string,
    groupLabel: string
  ) => void
  openExternalConflictDiff: (
    filePath: string,
    diskContent: string,
    unsavedContent: string
  ) => void
  updateActiveFileContent: (content: string) => void
  saveActiveFile: (options?: { force?: boolean }) => Promise<boolean>
  reloadActiveFile: () => Promise<void>
  previewFileTabIds: Set<string>
  toggleFileTabPreview: (tabId: string) => void
  filesMaximized: boolean
  toggleFilesMaximized: () => void
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null)

function normalizePath(path: string): string {
  return path.replace(/\\/g, "/")
}

function fileName(path: string): string {
  return path.split("/").pop() || path
}

function isDirtyFileTab(tab: FileWorkspaceTab): boolean {
  return tab.kind === "file" && Boolean(tab.isDirty)
}

const IMAGE_EXTENSIONS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "svg",
  "webp",
  "bmp",
  "ico",
])

const IMAGE_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  svg: "image/svg+xml",
  webp: "image/webp",
  bmp: "image/bmp",
  ico: "image/x-icon",
}

export function isImageFile(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? ""
  return IMAGE_EXTENSIONS.has(ext)
}

function loadingTab(
  id: string,
  kind: FileWorkspaceTabKind,
  title: string,
  description: string | null,
  path: string | null,
  language: string
): FileWorkspaceTab {
  return {
    id,
    kind,
    title,
    description,
    path,
    language,
    content: "",
    loading: true,
    savedContent: "",
    isDirty: false,
    etag: null,
    mtimeMs: null,
    readonly: kind !== "file",
    lineEnding: "none",
    saveState: "idle",
    saveError: null,
  }
}

type LoadDecision = { kind: "skip" } | { kind: "fetch"; gen: number }

async function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  timeoutMessage: string
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | null = null
  const timeoutPromise = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      reject(new Error(timeoutMessage))
    }, timeoutMs)
  })

  try {
    return await Promise.race([promise, timeoutPromise])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

interface WorkspaceProviderProps {
  children: ReactNode
}

export function WorkspaceProvider({ children }: WorkspaceProviderProps) {
  const t = useTranslations("Folder.workspaceContext")
  const { activeFolder, activeFolderId } = useActiveFolder()
  const folderPath = activeFolder?.path
  /* activeFolderId used in effect below to reset file tabs on folder switch */
  void activeFolderId
  const [activePane, setActivePaneState] =
    useState<WorkspacePane>("conversation")
  const [fileTabs, setFileTabs] = useState<FileWorkspaceTab[]>([])
  const [activeFileTabId, setActiveFileTabId] = useState<string | null>(null)
  const [pendingFileReveal, setPendingFileReveal] = useState<{
    requestId: number
    path: string
    line: number
  } | null>(null)
  const [previewFileTabIds, setPreviewFileTabIds] = useState<Set<string>>(
    new Set()
  )
  const [filesMaximized, setFilesMaximized] = useState(false)
  const fileTabsRef = useRef<FileWorkspaceTab[]>([])
  const fileRevealRequestIdRef = useRef(0)
  // tabId -> generation of its current in-flight fetch. Serves two roles:
  //   (a) Dedup: `has(tabId)` collapses rapid re-clicks within one event
  //       loop turn (where fileTabsRef.current is still pre-render-stale).
  //   (b) Staleness check: each fetch captures the generation it was
  //       started with and only commits state on resolve if it still
  //       matches — preventing an orphaned fetch (after close+reopen, or
  //       a superseding refresh) from clobbering the tab.
  const inFlightLoadsRef = useRef<Map<string, number>>(new Map())
  const nextLoadGenRef = useRef(0)

  useEffect(() => {
    fileTabsRef.current = fileTabs
  }, [fileTabs])

  const mode: WorkspaceMode = fileTabs.length > 0 ? "fusion" : "conversation"
  const effectiveFilesMaximized = mode === "fusion" && filesMaximized

  // Reset maximize state once the file workspace is empty so reopening a file
  // later starts from the normal split instead of a stale maximized layout.
  useEffect(() => {
    if (fileTabs.length === 0 && filesMaximized) {
      /* eslint-disable react-hooks/set-state-in-effect */
      setFilesMaximized(false)
      /* eslint-enable react-hooks/set-state-in-effect */
    }
  }, [fileTabs.length, filesMaximized])

  const toggleFilesMaximized = useCallback(() => {
    setFilesMaximized((prev) => !prev)
  }, [])

  // Clear file tabs when the active folder changes — files are not persisted
  // across folder switches in the workspace model.
  useEffect(() => {
    /* eslint-disable react-hooks/set-state-in-effect */
    setFileTabs([])
    setActiveFileTabId(null)
    setPreviewFileTabIds(new Set())
    setPendingFileReveal(null)
    // Any in-flight fetches belong to the previous folder. Their resolve
    // handlers will no-op against the now-empty tab list, but we must drop
    // their markers so a subsequent re-open of the same path is not
    // erroneously deduped.
    inFlightLoadsRef.current.clear()
    /* eslint-enable react-hooks/set-state-in-effect */
  }, [activeFolderId])

  const setActivePane = useCallback((nextPane: WorkspacePane) => {
    setActivePaneState((prev) => (prev === nextPane ? prev : nextPane))
  }, [])

  const activateConversationPane = useCallback(() => {
    setActivePaneState((prev) =>
      prev === "conversation" ? prev : "conversation"
    )
    // Releasing the files overlay so a session opened from the sidebar (or any
    // other path that activates the conversation pane) becomes visible instead
    // of staying hidden behind a maximized files pane.
    setFilesMaximized(false)
  }, [])

  const activateFilePane = useCallback(() => {
    setActivePaneState((prev) => (prev === "files" ? prev : "files"))
  }, [])

  // Pure activation — no content mutation.
  const activateTab = useCallback(
    (tabId: string) => {
      setActiveFileTabId(tabId)
      activateFilePane()
    },
    [activateFilePane]
  )

  // Insert a freshly created (loading, empty) tab. Caller has verified no tab
  // with this id exists. If a race introduced one, leave it alone.
  const seedLoadingTab = useCallback(
    (nextTab: FileWorkspaceTab) => {
      setFileTabs((prev) => {
        if (prev.some((tab) => tab.id === nextTab.id)) return prev
        return [...prev, nextTab]
      })
      setActiveFileTabId(nextTab.id)
      activateFilePane()
    },
    [activateFilePane]
  )

  // Mark an existing tab as refreshing. Preserves content / originalContent /
  // modifiedContent / gitBaseContent / savedContent / etag / mtimeMs /
  // isDirty / readonly / lineEnding. Clears any prior error state.
  const markTabRefreshing = useCallback((tabId: string) => {
    setFileTabs((prev) =>
      prev.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              loading: true,
              saveState: "idle",
              saveError: null,
            }
          : tab
      )
    )
  }, [])

  // Reset an errored tab to a clean cold-load state. The previous error
  // message is currently stored in `content`; clear it so the next load
  // re-enters the placeholder branch instead of flashing the error string.
  const markErrorRetry = useCallback(
    (tabId: string, kind: FileWorkspaceTabKind) => {
      setFileTabs((prev) =>
        prev.map((tab) =>
          tab.id === tabId
            ? {
                ...tab,
                loading: true,
                content: "",
                originalContent:
                  kind === "rich-diff" ? undefined : tab.originalContent,
                modifiedContent:
                  kind === "rich-diff" ? undefined : tab.modifiedContent,
                saveState: "idle",
                saveError: null,
              }
            : tab
        )
      )
    },
    []
  )

  // Replace an entire tab atomically. Used for synchronous content sources
  // (session diffs, external-conflict diffs) where the caller already holds
  // the final content.
  const replaceTabContent = useCallback(
    (nextTab: FileWorkspaceTab) => {
      setFileTabs((prev) => {
        const idx = prev.findIndex((tab) => tab.id === nextTab.id)
        if (idx < 0) return [...prev, nextTab]
        const updated = [...prev]
        updated[idx] = nextTab
        return updated
      })
      setActiveFileTabId(nextTab.id)
      activateFilePane()
    },
    [activateFilePane]
  )

  // Orchestrates the "I want to start (or restart) a load for this tab" flow.
  // Encapsulates: cache short-circuit, in-flight dedup, error retry, forced
  // refresh, and cold-load creation. Returns whether the caller should
  // proceed with its fetch.
  const beginFetchGeneration = useCallback((tabId: string): number => {
    nextLoadGenRef.current += 1
    const gen = nextLoadGenRef.current
    inFlightLoadsRef.current.set(tabId, gen)
    return gen
  }, [])

  const decideLoad = useCallback(
    (seed: FileWorkspaceTab, reload: boolean): LoadDecision => {
      // Dedup synchronously. inFlightLoadsRef is updated immediately on
      // generation start, so rapid re-clicks within a single event loop
      // turn collapse here — unlike fileTabsRef.current, which only
      // reflects state after React flushes a render.
      if (inFlightLoadsRef.current.has(seed.id)) {
        activateTab(seed.id)
        return { kind: "skip" }
      }

      const existing = fileTabsRef.current.find((t) => t.id === seed.id)
      if (!existing) {
        // "reload" means "refresh an existing tab". If the tab is gone —
        // e.g. the user closed it while a watcher-driven reload was in
        // flight — do not resurrect it as a phantom tab.
        if (reload) return { kind: "skip" }
        seedLoadingTab(seed)
        return { kind: "fetch", gen: beginFetchGeneration(seed.id) }
      }

      activateTab(existing.id)

      if (existing.saveState === "error") {
        markErrorRetry(existing.id, existing.kind)
        return { kind: "fetch", gen: beginFetchGeneration(seed.id) }
      }

      // Stale clean tab — the watcher saw an external change while we were
      // inactive. Promote to reload now so the user never sees stale bytes.
      // Stale dirty tabs are NOT auto-reloaded: conflict resolution belongs
      // to the watcher, which surfaces the prompt instead of clobbering
      // unsaved edits.
      const stalePromotesReload =
        existing.kind === "file" && existing.stale === true && !existing.isDirty

      if (!reload && !stalePromotesReload) {
        // Cache hit — nothing to do.
        return { kind: "skip" }
      }

      markTabRefreshing(existing.id)
      return { kind: "fetch", gen: beginFetchGeneration(seed.id) }
    },
    [
      activateTab,
      beginFetchGeneration,
      markErrorRetry,
      markTabRefreshing,
      seedLoadingTab,
    ]
  )

  // Variant of decideLoad for diff tabs: content is inherently volatile
  // (git state changes), so we always refetch — but non-destructively.
  const beginDiffLoad = useCallback(
    (seed: FileWorkspaceTab): { skip: true } | { skip: false; gen: number } => {
      if (inFlightLoadsRef.current.has(seed.id)) {
        activateTab(seed.id)
        return { skip: true }
      }

      const existing = fileTabsRef.current.find((t) => t.id === seed.id)
      if (!existing) {
        seedLoadingTab(seed)
        return { skip: false, gen: beginFetchGeneration(seed.id) }
      }

      activateTab(seed.id)
      if (existing.saveState === "error") {
        markErrorRetry(seed.id, seed.kind)
      } else {
        markTabRefreshing(seed.id)
      }
      return { skip: false, gen: beginFetchGeneration(seed.id) }
    },
    [
      activateTab,
      beginFetchGeneration,
      markErrorRetry,
      markTabRefreshing,
      seedLoadingTab,
    ]
  )

  // Called from every fetch's resolve/error path. Returns true iff this
  // particular fetch is still the canonical in-flight load for the tab —
  // i.e. the user hasn't closed the tab, switched folders, or started a
  // newer fetch in the meantime. Also performs the cleanup atomically.
  const settleFetch = useCallback((tabId: string, gen: number): boolean => {
    if (inFlightLoadsRef.current.get(tabId) !== gen) return false
    inFlightLoadsRef.current.delete(tabId)
    return true
  }, [])

  const resolveTab = useCallback(
    (tabId: string, content: string, loading = false) => {
      setFileTabs((prev) =>
        prev.map((tab) =>
          tab.id === tabId
            ? {
                ...tab,
                content,
                loading,
              }
            : tab
        )
      )
    },
    []
  )

  const rejectTab = useCallback(
    (tabId: string, errorMessage: string) => {
      resolveTab(
        tabId,
        t("unableLoadContent", { message: errorMessage }),
        false
      )
      setFileTabs((prev) =>
        prev.map((tab) =>
          tab.id === tabId
            ? {
                ...tab,
                saveState: "error",
                saveError: errorMessage,
              }
            : tab
        )
      )
    },
    [resolveTab, t]
  )

  const resolveRichDiffTab = useCallback(
    (
      tabId: string,
      originalContent: string,
      modifiedContent: string,
      loading = false
    ) => {
      setFileTabs((prev) =>
        prev.map((tab) =>
          tab.id === tabId
            ? { ...tab, originalContent, modifiedContent, content: "", loading }
            : tab
        )
      )
    },
    []
  )

  const consumePendingFileReveal = useCallback((requestId: number) => {
    setPendingFileReveal((prev) =>
      prev && prev.requestId === requestId ? null : prev
    )
  }, [])

  // Background reload: refresh an open tab's content without changing
  // activeFileTabId or activating the file pane. Used by the workspace
  // watcher when an external change touches a clean tab the user isn't
  // currently looking at — VS Code / IntelliJ silently absorb such changes
  // so the next activation sees the latest bytes. Dirty tabs are off-limits
  // (conflict resolution belongs to the watcher via markTabsStale).
  const reloadOpenFileBackground = useCallback(
    async (rawPath: string) => {
      if (!folderPath) return
      const path = normalizePath(rawPath)
      const tabId = `file:${path}`
      const existing = fileTabsRef.current.find((t) => t.id === tabId)
      if (!existing || existing.kind !== "file") return
      if (existing.isDirty) return
      if (inFlightLoadsRef.current.has(tabId)) return

      const image = isImageFile(path)

      markTabRefreshing(tabId)
      const gen = beginFetchGeneration(tabId)

      try {
        if (image) {
          const absPath = `${folderPath}/${path}`
          const ext = path.split(".").pop()?.toLowerCase() ?? ""
          const mime = IMAGE_MIME[ext] ?? "image/png"
          const b64 = await withTimeout(
            readFileBase64(absPath),
            15_000,
            t("previewRequestTimedOut")
          )
          if (!settleFetch(tabId, gen)) return
          setFileTabs((prev) =>
            prev.map((tab) =>
              tab.id === tabId
                ? {
                    ...tab,
                    content: `data:${mime};base64,${b64}`,
                    readonly: true,
                    loading: false,
                    saveState: "idle",
                    saveError: null,
                    stale: false,
                  }
                : tab
            )
          )
          return
        }

        const [result, gitBaseContent] = await withTimeout(
          Promise.all([
            readFileForEdit(folderPath, path),
            (async () => {
              const tracked = await gitIsTracked(folderPath, path).catch(
                () => false
              )
              if (!tracked) return undefined
              return gitShowFile(folderPath, path).catch(() => "")
            })(),
          ]),
          15_000,
          t("previewRequestTimedOut")
        )
        if (!settleFetch(tabId, gen)) return
        setFileTabs((prev) =>
          prev.map((tab) =>
            tab.id === tabId
              ? {
                  ...tab,
                  content: result.content,
                  gitBaseContent,
                  savedContent: result.content,
                  isDirty: false,
                  etag: result.etag,
                  mtimeMs: result.mtime_ms,
                  readonly: result.readonly,
                  lineEnding: result.line_ending,
                  saveState: "idle",
                  saveError: null,
                  loading: false,
                  stale: false,
                }
              : tab
          )
        )
      } catch (error) {
        if (!settleFetch(tabId, gen)) return
        rejectTab(tabId, toErrorMessage(error))
      }
    },
    [
      folderPath,
      beginFetchGeneration,
      markTabRefreshing,
      rejectTab,
      settleFetch,
      t,
    ]
  )

  // Mark the tab matching `path` as stale so the next activation triggers a
  // reload (clean) or a conflict prompt (dirty). The watcher calls this for
  // dirty non-active tabs when an external change is observed, since silently
  // reloading would discard the user's unsaved edits.
  const markTabsStale = useCallback((rawPath: string) => {
    const path = normalizePath(rawPath)
    const tabId = `file:${path}`
    setFileTabs((prev) => {
      const idx = prev.findIndex((tab) => tab.id === tabId)
      if (idx < 0) return prev
      const tab = prev[idx]
      if (tab.stale === true) return prev
      const updated = [...prev]
      updated[idx] = { ...tab, stale: true }
      return updated
    })
  }, [])

  // Write a prefetched FileEditContent into the matching tab. The change-
  // detection watcher uses this after its resolver has already read the
  // latest disk content — without this we would re-read every file twice
  // per workspace event (resolver + reload). Dirty tabs are skipped so
  // unsaved edits are never silently clobbered.
  //
  // Concurrency contract: the in-flight marker is bumped to invalidate any
  // concurrent openFilePreview's pending settle (so an older read cannot
  // overwrite our newer payload) and is then settled IMMEDIATELY after the
  // synchronous content write. The slow, cosmetic git-base refresh runs
  // out-of-band — it does NOT extend the in-flight marker's lifetime —
  // so a stuck git invocation cannot block a subsequent user-initiated
  // reload via the openFilePreview dedup path.
  const applyExternalReload = useCallback(
    async (rawPath: string, fetched: FileEditContent) => {
      if (!folderPath) return
      const path = normalizePath(rawPath)
      const tabId = `file:${path}`
      // Outer existence check — purely to avoid bumping the in-flight gen
      // for a non-existent path (which would pollute openFilePreview's
      // dedup). The dirty guard is NOT outer: fileTabsRef can lag a tick
      // behind a user keystroke whose dirty update is already enqueued
      // but not yet committed. The atomic check lives inside the
      // setFileTabs updater below, where prev reflects every earlier
      // queued updater (including the keystroke).
      const existing = fileTabsRef.current.find((t) => t.id === tabId)
      if (!existing || existing.kind !== "file") return

      const gen = beginFetchGeneration(tabId)
      const fetchedEtag = fetched.etag

      // Atomic write: refuses the apply if the tab became dirty between
      // our outer existence check and the actual commit (e.g. user typed
      // in the same React batch as the watcher's apply call). The refused
      // branch flips stale=true so the aux-panel effect (stale && isDirty
      // → announceConflict) surfaces the divergence immediately instead
      // of waiting for the next save to discover the etag mismatch.
      setFileTabs((prev) =>
        prev.map((tab) => {
          if (tab.id !== tabId || tab.kind !== "file") return tab
          if (tab.isDirty) return { ...tab, stale: true }
          return {
            ...tab,
            content: fetched.content,
            savedContent: fetched.content,
            isDirty: false,
            etag: fetched.etag,
            mtimeMs: fetched.mtime_ms,
            readonly: fetched.readonly,
            lineEnding: fetched.line_ending,
            loading: false,
            stale: false,
            saveState: "idle",
            saveError: null,
          }
        })
      )

      // Release the in-flight marker NOW. Two-stage invalidation: the
      // beginFetchGeneration above already poisoned any concurrent
      // openFilePreview fetch (its settleFetch will fail), so clearing
      // here cannot resurrect an in-flight overwrite. The cosmetic git
      // base refresh below is decoupled — slow git must not block user
      // reload dedup. (Each call's settle is mutually exclusive: the
      // last applyExternalReload's gen wins, prior gens are stale.)
      settleFetch(tabId, gen)

      // Fire-and-forget git base refresh, etag-gated.
      //
      // The captured fetchedEtag doubles as a staleness token: if our
      // atomic write above succeeded, the tab now carries fetchedEtag;
      // if it was refused (dirty), or a later applyExternalReload /
      // openFilePreview reload / close+reopen changed the tab, the tab
      // carries a different etag. The final write checks tab.etag ===
      // fetchedEtag inside the updater so a stale fetch can never paint
      // gitter decorations onto a tab whose content has moved on. No
      // separate generation token needed — etag is the natural fingerprint.
      void (async () => {
        try {
          const gitBaseContent = await withTimeout(
            (async () => {
              const tracked = await gitIsTracked(folderPath, path).catch(
                () => false
              )
              if (!tracked) return undefined
              return gitShowFile(folderPath, path).catch(() => "")
            })(),
            15_000,
            t("previewRequestTimedOut")
          )
          setFileTabs((prev) =>
            prev.map((tab) => {
              if (tab.id !== tabId || tab.kind !== "file") return tab
              if (tab.etag !== fetchedEtag) return tab
              return { ...tab, gitBaseContent }
            })
          )
        } catch {
          // Timeout or unexpected failure: leave existing gitBaseContent.
        }
      })()
    },
    [folderPath, beginFetchGeneration, settleFetch, t]
  )

  // Mark a clean open tab as load-failed. Used by the change-detection
  // watcher when a readFileForEdit on a changed path fails (most commonly
  // external delete). Dirty tabs are deliberately not touched here — the
  // watcher routes them to markTabsStale so unsaved edits are preserved.
  const rejectFileTab = useCallback(
    (rawPath: string, errorMessage: string) => {
      const path = normalizePath(rawPath)
      const tabId = `file:${path}`
      // Outer existence check only; the dirty guard is atomic inside the
      // updater (see applyExternalReload for the same race shape).
      const existing = fileTabsRef.current.find((t) => t.id === tabId)
      if (!existing || existing.kind !== "file") return

      // Bump generation so any concurrent fetch's settle is invalidated
      // and cannot overwrite the error message we are about to write.
      const gen = beginFetchGeneration(tabId)
      setFileTabs((prev) =>
        prev.map((tab) => {
          if (tab.id !== tabId || tab.kind !== "file") return tab
          // Symmetric with applyExternalReload's dirty refusal: surface
          // the divergence via stale rather than silently no-op. Callers
          // typically also call markTabsStale, so this is usually
          // idempotent; the in-updater write protects direct callers.
          if (tab.isDirty) return { ...tab, stale: true }
          return {
            ...tab,
            content: t("unableLoadContent", { message: errorMessage }),
            loading: false,
            stale: false,
            saveState: "error",
            saveError: errorMessage,
          }
        })
      )
      settleFetch(tabId, gen)
    },
    [beginFetchGeneration, settleFetch, t]
  )

  const openFilePreview = useCallback(
    async (rawPath: string, options?: { line?: number; reload?: boolean }) => {
      if (!folderPath) return
      const path = normalizePath(rawPath)
      const requestedLine =
        typeof options?.line === "number" && Number.isFinite(options.line)
          ? Math.max(1, Math.floor(options.line))
          : null
      if (requestedLine) {
        fileRevealRequestIdRef.current += 1
        setPendingFileReveal({
          requestId: fileRevealRequestIdRef.current,
          path,
          line: requestedLine,
        })
      } else {
        setPendingFileReveal(null)
      }
      const tabId = `file:${path}`
      const image = isImageFile(path)
      const seed = loadingTab(
        tabId,
        "file",
        fileName(path),
        path,
        path,
        image ? "image" : languageFromPath(path)
      )

      const decision = decideLoad(seed, options?.reload ?? false)
      if (decision.kind === "skip") return
      const { gen } = decision

      try {
        if (image) {
          const absPath = `${folderPath}/${path}`
          const ext = path.split(".").pop()?.toLowerCase() ?? ""
          const mime = IMAGE_MIME[ext] ?? "image/png"
          const b64 = await withTimeout(
            readFileBase64(absPath),
            15_000,
            t("previewRequestTimedOut")
          )
          if (!settleFetch(tabId, gen)) return
          setFileTabs((prev) =>
            prev.map((tab) =>
              tab.id === tabId
                ? {
                    ...tab,
                    content: `data:${mime};base64,${b64}`,
                    readonly: true,
                    loading: false,
                    saveState: "idle",
                    saveError: null,
                    stale: false,
                  }
                : tab
            )
          )
          return
        }

        const [result, gitBaseContent] = await withTimeout(
          Promise.all([
            readFileForEdit(folderPath, path),
            (async () => {
              const tracked = await gitIsTracked(folderPath, path).catch(
                () => false
              )
              if (!tracked) return undefined
              return gitShowFile(folderPath, path).catch(() => "")
            })(),
          ]),
          15_000,
          t("previewRequestTimedOut")
        )
        if (!settleFetch(tabId, gen)) return
        setFileTabs((prev) =>
          prev.map((tab) =>
            tab.id === tabId
              ? {
                  ...tab,
                  content: result.content,
                  gitBaseContent,
                  savedContent: result.content,
                  isDirty: false,
                  etag: result.etag,
                  mtimeMs: result.mtime_ms,
                  readonly: result.readonly,
                  lineEnding: result.line_ending,
                  saveState: "idle",
                  saveError: null,
                  loading: false,
                  stale: false,
                }
              : tab
          )
        )
      } catch (error) {
        if (!settleFetch(tabId, gen)) return
        if (requestedLine) {
          setPendingFileReveal((prev) =>
            prev && prev.path === path ? null : prev
          )
        }
        rejectTab(tabId, toErrorMessage(error))
      }
    },
    [folderPath, decideLoad, rejectTab, settleFetch, t]
  )

  const openWorkingTreeDiff = useCallback(
    async (
      rawPath?: string,
      options?: { mode?: "auto" | "unified" | "overview" }
    ) => {
      if (!folderPath) return

      if (!rawPath) {
        const tabId = "diff:working:all"
        const title = t("diffTitleWorkspace")
        const description = t("diffDescriptionWorkingTree")
        const seed = loadingTab(tabId, "diff", title, description, null, "diff")
        const decision = beginDiffLoad(seed)
        if (decision.skip) return
        const { gen } = decision
        try {
          const result = await withTimeout(
            gitDiff(folderPath),
            20_000,
            t("diffRequestTimedOut")
          )
          if (settleFetch(tabId, gen))
            resolveTab(tabId, result || t("noChanges"), false)
        } catch (error) {
          if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
        }
        return
      }

      const path = normalizePath(rawPath)
      const mode = options?.mode ?? "auto"

      if (mode === "overview") {
        const isRoot = path === "."
        const displayPath = isRoot ? folderPath : path
        const encodedPath = encodeURIComponent(path)
        const tabId = `diff:working-overview:${encodedPath}`
        const title = t("diffTitleFile", {
          name: fileName(displayPath ?? path),
        })
        const description = displayPath ?? path
        const seed = loadingTab(tabId, "diff", title, description, path, "diff")
        const decision = beginDiffLoad(seed)
        if (decision.skip) return
        const { gen } = decision
        try {
          const result = await withTimeout(
            gitDiff(folderPath, path),
            20_000,
            t("diffRequestTimedOut")
          )
          if (settleFetch(tabId, gen))
            resolveTab(tabId, result || t("noChanges"), false)
        } catch (error) {
          if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
        }
        return
      }

      if (mode === "unified") {
        const tabId = `diff:working:${path}:unified`
        const title = t("diffTitleFile", { name: fileName(path) })
        const description = path
        const seed = loadingTab(tabId, "diff", title, description, path, "diff")
        const decision = beginDiffLoad(seed)
        if (decision.skip) return
        const { gen } = decision
        try {
          const result = await withTimeout(
            gitDiff(folderPath, path),
            20_000,
            t("diffRequestTimedOut")
          )
          if (settleFetch(tabId, gen))
            resolveTab(tabId, result || t("noChanges"), false)
        } catch (error) {
          if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
        }
        return
      }

      const tabId = `diff:working:${path}`
      const title = t("diffTitleFile", { name: fileName(path) })
      const description = path
      const lang = languageFromPath(path)

      const seed = loadingTab(
        tabId,
        "rich-diff",
        title,
        description,
        path,
        lang
      )
      const decision = beginDiffLoad(seed)
      if (decision.skip) return
      const { gen } = decision
      try {
        const [originalContent, modifiedResult] = await withTimeout(
          Promise.all([
            gitShowFile(folderPath, path).catch(() => ""),
            readFilePreview(folderPath, path).catch(() => ({
              content: "",
              path: "",
            })),
          ]),
          20_000,
          t("diffRequestTimedOut")
        )
        if (settleFetch(tabId, gen))
          resolveRichDiffTab(tabId, originalContent, modifiedResult.content)
      } catch (error) {
        if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
      }
    },
    [
      folderPath,
      beginDiffLoad,
      rejectTab,
      resolveTab,
      resolveRichDiffTab,
      settleFetch,
      t,
    ]
  )

  const openBranchDiff = useCallback(
    async (
      branch: string,
      rawPath?: string,
      options?: { mode?: "default" | "overview" }
    ) => {
      if (!folderPath) return
      const targetBranch = branch.trim()
      if (!targetBranch) return

      const path = rawPath ? normalizePath(rawPath) : null
      const mode = options?.mode ?? "default"
      const encodedBranch = encodeURIComponent(targetBranch)
      const encodedPath = encodeURIComponent(path ?? "all")
      const tabId =
        mode === "overview"
          ? `diff:branch-overview:${encodedBranch}:${encodedPath}`
          : `diff:branch:${targetBranch}:${path ?? "all"}`
      const title = path
        ? t("compareTitleFile", { name: fileName(path) })
        : t("compareTitleBranch", { branch: targetBranch })
      const description = path
        ? t("compareDescriptionPath", { path, branch: targetBranch })
        : t("compareDescriptionBranch", { branch: targetBranch })

      if (mode !== "overview" && path) {
        const lang = languageFromPath(path)
        const seed = loadingTab(
          tabId,
          "rich-diff",
          title,
          description,
          path,
          lang
        )
        const decision = beginDiffLoad(seed)
        if (decision.skip) return
        const { gen } = decision
        try {
          const [originalContent, modifiedResult] = await withTimeout(
            Promise.all([
              gitShowFile(folderPath, path, targetBranch).catch(() => ""),
              readFilePreview(folderPath, path).catch(() => ({
                content: "",
                path: "",
              })),
            ]),
            20_000,
            t("branchCompareRequestTimedOut")
          )
          if (settleFetch(tabId, gen))
            resolveRichDiffTab(tabId, originalContent, modifiedResult.content)
        } catch (error) {
          if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
        }
        return
      }

      const seed = loadingTab(tabId, "diff", title, description, path, "diff")
      const decision = beginDiffLoad(seed)
      if (decision.skip) return
      const { gen } = decision
      try {
        const result = await withTimeout(
          gitDiffWithBranch(folderPath, targetBranch, path ?? undefined),
          20_000,
          t("branchCompareRequestTimedOut")
        )
        if (settleFetch(tabId, gen))
          resolveTab(tabId, result || t("noChanges"), false)
      } catch (error) {
        if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
      }
    },
    [
      folderPath,
      beginDiffLoad,
      rejectTab,
      resolveRichDiffTab,
      resolveTab,
      settleFetch,
      t,
    ]
  )

  const openCommitDiff = useCallback(
    async (commit: string, rawPath?: string, message?: string) => {
      if (!folderPath) return
      const path = rawPath ? normalizePath(rawPath) : null
      const tabId = `diff:commit:${commit}:${path ?? "all"}`
      const title = path
        ? t("diffTitleCommitFile", {
            name: fileName(path),
            hash: commit.slice(0, 7),
          })
        : t("diffTitleCommit", { hash: commit.slice(0, 7) })
      const description = path
        ? t("diffDescriptionCommitPath", { path, commit })
        : message || t("diffDescriptionCommit", { commit })

      if (path) {
        const lang = languageFromPath(path)
        const seed = loadingTab(
          tabId,
          "rich-diff",
          title,
          description,
          path,
          lang
        )
        const decision = beginDiffLoad(seed)
        if (decision.skip) return
        const { gen } = decision
        try {
          const [originalContent, modifiedContent] = await withTimeout(
            Promise.all([
              gitShowFile(folderPath, path, `${commit}~1`).catch(() => ""),
              gitShowFile(folderPath, path, commit).catch(() => ""),
            ]),
            20_000,
            t("commitDiffRequestTimedOut")
          )
          if (settleFetch(tabId, gen))
            resolveRichDiffTab(tabId, originalContent, modifiedContent)
        } catch (error) {
          if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
        }
      } else {
        const seed = loadingTab(tabId, "diff", title, description, path, "diff")
        const decision = beginDiffLoad(seed)
        if (decision.skip) return
        const { gen } = decision
        try {
          const result = await withTimeout(
            gitShowDiff(folderPath, commit, undefined),
            20_000,
            t("commitDiffRequestTimedOut")
          )
          if (settleFetch(tabId, gen))
            resolveTab(tabId, result || t("noDiffOutput"), false)
        } catch (error) {
          if (settleFetch(tabId, gen)) rejectTab(tabId, toErrorMessage(error))
        }
      }
    },
    [
      folderPath,
      beginDiffLoad,
      rejectTab,
      resolveTab,
      resolveRichDiffTab,
      settleFetch,
      t,
    ]
  )

  const openSessionFileDiff = useCallback(
    (filePath: string, diffContent: string, groupLabel: string) => {
      const path = normalizePath(filePath)
      const tabId = `diff:session:${groupLabel}:${path}`
      const title = t("diffTitleFile", { name: fileName(path) })
      const description = `${path} · ${groupLabel}`

      const tab: FileWorkspaceTab = {
        id: tabId,
        kind: "diff",
        title,
        description,
        path: null,
        language: "diff",
        content: diffContent,
        loading: false,
      }

      replaceTabContent(tab)
    },
    [replaceTabContent, t]
  )

  const openExternalConflictDiff = useCallback(
    (filePath: string, diskContent: string, unsavedContent: string) => {
      const path = normalizePath(filePath)
      const tabId = `diff:external-conflict:${path}`
      const title = t("diffTitleConflictFile", { name: fileName(path) })
      const description = t("diffDescriptionConflict", { path })
      const language = languageFromPath(path)

      const tab: FileWorkspaceTab = {
        id: tabId,
        kind: "rich-diff",
        title,
        description,
        path,
        language,
        content: "",
        loading: false,
        originalContent: diskContent,
        modifiedContent: unsavedContent,
      }

      replaceTabContent(tab)
    },
    [replaceTabContent, t]
  )

  const updateActiveFileContent = useCallback(
    (content: string) => {
      if (!activeFileTabId) return

      setFileTabs((prev) =>
        prev.map((tab) => {
          if (tab.id !== activeFileTabId || tab.kind !== "file") return tab
          if (tab.loading || tab.readonly) return tab
          if (tab.content === content) return tab

          const savedContent = tab.savedContent ?? ""
          return {
            ...tab,
            content,
            isDirty: content !== savedContent,
            saveState: tab.saveState === "saving" ? "saving" : "idle",
            saveError: null,
          }
        })
      )
    },
    [activeFileTabId]
  )

  const saveFileTab = useCallback(
    async (tabId: string, options?: { force?: boolean }): Promise<boolean> => {
      if (!folderPath) return false
      const tab = fileTabsRef.current.find(
        (candidate) => candidate.id === tabId
      )
      if (!tab || tab.kind !== "file") return false
      if (tab.loading || tab.readonly) return false
      if (!tab.path) return false
      if (!tab.isDirty) return true

      const contentAtSaveStart = tab.content
      const expectedEtag = options?.force ? null : (tab.etag ?? null)

      setFileTabs((prev) =>
        prev.map((candidate) =>
          candidate.id === tabId
            ? {
                ...candidate,
                saveState: "saving",
                saveError: null,
              }
            : candidate
        )
      )

      try {
        const result = await withTimeout(
          saveFileContent(
            folderPath,
            tab.path,
            contentAtSaveStart,
            expectedEtag
          ),
          20_000,
          t("saveRequestTimedOut")
        )

        setFileTabs((prev) =>
          prev.map((candidate) => {
            if (candidate.id !== tabId || candidate.kind !== "file") {
              return candidate
            }

            const savedContent = contentAtSaveStart
            return {
              ...candidate,
              etag: result.etag,
              mtimeMs: result.mtime_ms,
              readonly: result.readonly,
              lineEnding: result.line_ending,
              savedContent,
              isDirty: candidate.content !== savedContent,
              saveState: "idle",
              saveError: null,
            }
          })
        )

        return true
      } catch (error) {
        const message = toErrorMessage(error)
        setFileTabs((prev) =>
          prev.map((candidate) =>
            candidate.id === tabId
              ? {
                  ...candidate,
                  saveState: "error",
                  saveError: message,
                }
              : candidate
          )
        )
        return false
      }
    },
    [folderPath, t]
  )

  const saveActiveFile = useCallback(
    async (options?: { force?: boolean }) => {
      if (!activeFileTabId) return false
      return saveFileTab(activeFileTabId, options)
    },
    [activeFileTabId, saveFileTab]
  )

  const reloadFileTab = useCallback(
    async (tabId: string) => {
      if (!folderPath) return
      const tab = fileTabsRef.current.find(
        (candidate) => candidate.id === tabId
      )
      if (!tab || tab.kind !== "file" || !tab.path) return
      const tabPath = tab.path

      setFileTabs((prev) =>
        prev.map((candidate) =>
          candidate.id === tabId
            ? {
                ...candidate,
                loading: true,
                saveError: null,
                saveState: "idle",
              }
            : candidate
        )
      )

      try {
        const [result, gitBaseContent] = await withTimeout(
          Promise.all([
            readFileForEdit(folderPath, tabPath),
            (async () => {
              const tracked = await gitIsTracked(folderPath, tabPath).catch(
                () => false
              )
              if (!tracked) return undefined
              return gitShowFile(folderPath, tabPath).catch(() => "")
            })(),
          ]),
          15_000,
          t("reloadRequestTimedOut")
        )
        setFileTabs((prev) =>
          prev.map((candidate) =>
            candidate.id === tabId
              ? {
                  ...candidate,
                  content: result.content,
                  gitBaseContent,
                  savedContent: result.content,
                  isDirty: false,
                  etag: result.etag,
                  mtimeMs: result.mtime_ms,
                  readonly: result.readonly,
                  lineEnding: result.line_ending,
                  saveState: "idle",
                  saveError: null,
                  loading: false,
                }
              : candidate
          )
        )
      } catch (error) {
        const message = toErrorMessage(error)
        setFileTabs((prev) =>
          prev.map((candidate) =>
            candidate.id === tabId
              ? {
                  ...candidate,
                  loading: false,
                  saveState: "error",
                  saveError: message,
                }
              : candidate
          )
        )
      }
    },
    [folderPath, t]
  )

  const reloadActiveFile = useCallback(async () => {
    if (!activeFileTabId) return
    await reloadFileTab(activeFileTabId)
  }, [activeFileTabId, reloadFileTab])

  const switchFileTab = useCallback(
    (tabId: string) => {
      if (activeFileTabId && activeFileTabId !== tabId) {
        void saveFileTab(activeFileTabId)
      }
      setActiveFileTabId(tabId)
      activateFilePane()
    },
    [activeFileTabId, activateFilePane, saveFileTab]
  )

  const closeFileTab = useCallback(
    (tabId: string) => {
      setFileTabs((prev) => {
        const idx = prev.findIndex((tab) => tab.id === tabId)
        if (idx < 0) return prev

        const tab = prev[idx]
        if (isDirtyFileTab(tab)) {
          const confirmed = window.confirm(
            t("confirmCloseDirtyTab", { title: tab.title })
          )
          if (!confirmed) return prev
        }

        const next = prev.filter((candidate) => candidate.id !== tabId)

        setActiveFileTabId((current) => {
          if (current !== tabId) return current
          if (next.length === 0) {
            activateConversationPane()
            return null
          }
          const nextIdx = Math.min(idx, next.length - 1)
          return next[nextIdx].id
        })

        setPreviewFileTabIds((prev) => {
          if (!prev.has(tabId)) return prev
          const updated = new Set(prev)
          updated.delete(tabId)
          return updated
        })

        // Drop any in-flight marker so reopening this path does not get
        // deduped against a now-orphaned fetch.
        inFlightLoadsRef.current.delete(tabId)

        return next
      })
    },
    [activateConversationPane, t]
  )

  const closeOtherFileTabs = useCallback(
    (tabId: string) => {
      setFileTabs((prev) => {
        const remaining = prev.filter((tab) => tab.id === tabId)
        if (remaining.length === 0) return prev

        const closingTabs = prev.filter((tab) => tab.id !== tabId)
        if (closingTabs.some(isDirtyFileTab)) {
          const confirmed = window.confirm(t("confirmCloseOtherDirtyTabs"))
          if (!confirmed) return prev
        }

        for (const closing of closingTabs) {
          inFlightLoadsRef.current.delete(closing.id)
        }

        setActiveFileTabId(tabId)
        activateFilePane()
        return remaining
      })
    },
    [activateFilePane, t]
  )

  const closeAllFileTabs = useCallback(() => {
    setFileTabs((prev) => {
      if (prev.some(isDirtyFileTab)) {
        const confirmed = window.confirm(t("confirmCloseAllDirtyTabs"))
        if (!confirmed) return prev
      }

      inFlightLoadsRef.current.clear()
      setActiveFileTabId(null)
      setPreviewFileTabIds(new Set())
      activateConversationPane()
      return []
    })
  }, [activateConversationPane, t])

  const reorderFileTabs = useCallback((tabs: FileWorkspaceTab[]) => {
    setFileTabs(tabs)
  }, [])

  const activeFileTab = useMemo(
    () => fileTabs.find((tab) => tab.id === activeFileTabId) ?? null,
    [fileTabs, activeFileTabId]
  )

  const activeFilePath = activeFileTab?.path ?? null

  const toggleFileTabPreview = useCallback((tabId: string) => {
    setPreviewFileTabIds((prev) => {
      const next = new Set(prev)
      if (next.has(tabId)) {
        next.delete(tabId)
      } else {
        next.add(tabId)
      }
      return next
    })
  }, [])

  const value = useMemo<WorkspaceContextValue>(
    () => ({
      mode,
      activePane,
      setActivePane,
      activateConversationPane,
      activateFilePane,
      fileTabs,
      activeFileTabId,
      activeFileTab,
      activeFilePath,
      switchFileTab,
      closeFileTab,
      closeOtherFileTabs,
      closeAllFileTabs,
      reorderFileTabs,
      openFilePreview,
      reloadOpenFileBackground,
      applyExternalReload,
      markTabsStale,
      rejectFileTab,
      pendingFileReveal,
      consumePendingFileReveal,
      openWorkingTreeDiff,
      openBranchDiff,
      openCommitDiff,
      openSessionFileDiff,
      openExternalConflictDiff,
      updateActiveFileContent,
      saveActiveFile,
      reloadActiveFile,
      previewFileTabIds,
      toggleFileTabPreview,
      filesMaximized: effectiveFilesMaximized,
      toggleFilesMaximized,
    }),
    [
      mode,
      activePane,
      setActivePane,
      activateConversationPane,
      activateFilePane,
      fileTabs,
      activeFileTabId,
      activeFileTab,
      activeFilePath,
      switchFileTab,
      closeFileTab,
      closeOtherFileTabs,
      closeAllFileTabs,
      reorderFileTabs,
      openFilePreview,
      reloadOpenFileBackground,
      applyExternalReload,
      markTabsStale,
      rejectFileTab,
      pendingFileReveal,
      consumePendingFileReveal,
      openWorkingTreeDiff,
      openBranchDiff,
      openCommitDiff,
      openSessionFileDiff,
      openExternalConflictDiff,
      updateActiveFileContent,
      saveActiveFile,
      reloadActiveFile,
      previewFileTabIds,
      toggleFileTabPreview,
      effectiveFilesMaximized,
      toggleFilesMaximized,
    ]
  )

  return (
    <WorkspaceContext.Provider value={value}>
      {children}
    </WorkspaceContext.Provider>
  )
}

export function useWorkspaceContext() {
  const ctx = useContext(WorkspaceContext)
  if (!ctx) {
    throw new Error("useWorkspaceContext must be used within WorkspaceProvider")
  }
  return ctx
}
