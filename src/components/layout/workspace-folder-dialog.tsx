"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  ChevronRight,
  FolderOpen,
  Link2,
  Link2Off,
  Loader2,
  Monitor,
  MonitorDot,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Server,
  Trash2,
  X,
  type LucideIcon,
} from "lucide-react"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { InputGroupButton } from "@/components/ui/input-group"
import { Checkbox } from "@/components/ui/checkbox"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { toast } from "sonner"
import { cn } from "@/lib/utils"
import {
  DirectoryBrowser,
  normalizeFsPath,
  type DirectoryBrowserHandle,
} from "@/components/shared/directory-browser"
import {
  createFolderLinks,
  listFolderLinks,
  previewFolderLinks,
  removeFolderLink,
  renameFolderLink,
  repairFolderLink,
} from "@/lib/api"
import { useImeGuard } from "@/hooks/use-ime-guard"
import { useRemoteWorkspaceConnections } from "@/hooks/use-remote-workspace-connections"
import { isDesktop, openFileDialog } from "@/lib/platform"
import { parentFsPath } from "@/lib/path-utils"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { toErrorMessage } from "@/lib/app-error"
import {
  getRemoteHomeDirectory,
  listRemoteDirectoryEntries,
  openRemoteWorkspaceFolder,
} from "@/lib/remote-workspace"
import {
  basenameOf,
  linkNameKey,
  partitionPlans,
  validateLinkName,
  type LinkNameIssue,
} from "@/lib/folder-links"
import type {
  FolderDetail,
  FolderLinkDetail,
  FolderLinkPlan,
  FolderLinkStatus,
  RemoteWorkspaceConnection,
} from "@/lib/types"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"

type View = "pick-workspace" | "pick-root" | "links" | "add-targets"

/**
 * Where the folder being picked will live. `local` is the machine codeg is
 * running on; `remote` is one of the connected codeg-servers, whose folders
 * can only be opened in that workspace's own window.
 */
type WorkspaceChoice =
  | { kind: "local" }
  | { kind: "remote"; connection: RemoteWorkspaceConnection }

interface WorkspaceFolderDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /**
   * Manage mode: skip root selection and edit this folder's links directly.
   * When absent the dialog starts at directory selection and opens the folder
   * before moving on.
   */
  folder?: FolderDetail | null
  /** Called once the workspace folder has been opened (creation flow only). */
  onFolderOpened?: (folder: FolderDetail) => void
}

/**
 * The single entry point for adding a workspace folder.
 *
 * Step 1 picks the workspace root (the same in-app browser on desktop and web —
 * the native picker is offered as a shortcut, not as a separate code path), and
 * step 2 links any number of other directories in as subdirectories of that
 * root. Reopened from the folder menu, it starts at step 2 so links can be
 * renamed, repaired, or removed later.
 *
 * On a local desktop window with remote workspaces connected, step 0 asks
 * *which* workspace to browse first: this machine, or one of the connected
 * servers. Picking a remote one browses that host's filesystem and opens the
 * folder in its own window — folders belong to the backend that owns their
 * paths, so a remote folder can't be opened into this workspace's list.
 */
export function WorkspaceFolderDialog({
  open,
  onOpenChange,
  folder,
  onFolderOpened,
}: WorkspaceFolderDialogProps) {
  const t = useTranslations("Folder.workspaceDialog")
  const tBrowser = useTranslations("DirectoryBrowser")
  const openFolder = useAppWorkspaceStore((s) => s.openFolder)
  const { connections, refresh: refreshConnections } =
    useRemoteWorkspaceConnections()

  const manageMode = !!folder
  const [view, setView] = useState<View>(manageMode ? "links" : "pick-root")
  const [choice, setChoice] = useState<WorkspaceChoice | null>(null)
  const [workspaceQuery, setWorkspaceQuery] = useState("")
  const [rootFolder, setRootFolder] = useState<FolderDetail | null>(
    folder ?? null
  )
  const [rootPath, setRootPath] = useState("")
  const [browserBusy, setBrowserBusy] = useState(false)
  const [openingRoot, setOpeningRoot] = useState(false)
  const rootBrowserRef = useRef<DirectoryBrowserHandle>(null)

  // Link targets being picked in the "add" view.
  const [targetPath, setTargetPath] = useState("")
  const [pickedTargets, setPickedTargets] = useState<string[]>([])
  const [pending, setPending] = useState<PendingLink[]>([])
  const [skipped, setSkipped] = useState<FolderLinkPlan[]>([])
  const [previewing, setPreviewing] = useState(false)
  const [creating, setCreating] = useState(false)

  const [links, setLinks] = useState<FolderLinkDetail[]>([])
  const [loadingLinks, setLoadingLinks] = useState(false)
  const [gitExclude, setGitExclude] = useState(true)
  const [renamingId, setRenamingId] = useState<number | null>(null)
  const [renameValue, setRenameValue] = useState("")
  const [busyLinkId, setBusyLinkId] = useState<number | null>(null)

  // A remote-desktop window already *is* a workspace; only the local window
  // gets to choose between them. The system picker is local-only for the same
  // reason: it opens a dialog on this machine.
  const isLocalDesktop = isDesktop() && getActiveRemoteConnectionId() === null
  const nativePickerAvailable = isLocalDesktop && choice?.kind !== "remote"

  // Stable per chosen host — DirectoryBrowser restarts its session whenever the
  // source identity changes, so an inline object would reset it every render.
  const remoteFileSystem = useMemo(() => {
    if (choice?.kind !== "remote") return undefined
    const { id } = choice.connection
    return {
      home: () => getRemoteHomeDirectory(id),
      list: (path: string) => listRemoteDirectoryEntries(id, path),
    }
  }, [choice])

  const linkBrowseStart = useMemo(
    () =>
      rootFolder ? (parentFsPath(rootFolder.path) ?? undefined) : undefined,
    [rootFolder]
  )

  // Re-read on mount and on every open: connections come and go from another
  // window (the settings window), so a list captured at mount goes stale. A
  // window bound to a remote server has no local list to offer.
  useEffect(() => {
    if (manageMode || !isLocalDesktop) return
    void refreshConnections()
  }, [open, manageMode, isLocalDesktop, refreshConnections])

  // Identity of the connection list, compared by content: every load hands back
  // a fresh array, so an id-based key is what tells "the list changed" apart
  // from "the same list arrived again".
  const connectionKey = connections.map((c) => c.id).join(",")
  // Mirrored so the reset below can read the list the view was decided from
  // without re-running when it loads.
  const connectionKeyRef = useRef(connectionKey)
  useEffect(() => {
    connectionKeyRef.current = connectionKey
  }, [connectionKey])
  const decidedKeyRef = useRef<string | null>(null)

  // Reset to a clean session on every real open, so a dialog reopened after a
  // cancel never shows the previous run's picks.
  useEffect(() => {
    if (!open) return
    const known = connectionKeyRef.current
    decidedKeyRef.current = known
    const canChooseWorkspace = !folder && isLocalDesktop && known !== ""
    setView(
      folder ? "links" : canChooseWorkspace ? "pick-workspace" : "pick-root"
    )
    setChoice(canChooseWorkspace ? null : { kind: "local" })
    setWorkspaceQuery("")
    setRootFolder(folder ?? null)
    setRootPath(folder?.path ?? "")
    setTargetPath("")
    setPickedTargets([])
    setPending([])
    setSkipped([])
    setLinks([])
    setRenamingId(null)
    setBusyLinkId(null)
    setOpeningRoot(false)
    setCreating(false)
    setPreviewing(false)
  }, [open, folder, isLocalDesktop])

  // The reload kicked off on open lands *after* the view was decided, so the
  // decision above is made from whatever the list held before it. Correct the
  // one case that matters — the list gained or lost every connection while the
  // dialog sat closed — and then leave the user alone: later steps (a picked
  // root, the linking view) are never interrupted.
  useEffect(() => {
    if (!open || manageMode || !isLocalDesktop) return
    const previous = decidedKeyRef.current
    if (previous === null || previous === connectionKey) return
    decidedKeyRef.current = connectionKey

    if (previous === "" && connectionKey !== "" && view === "pick-root") {
      // Nothing to choose between when the dialog opened; now there is.
      setChoice(null)
      setView("pick-workspace")
    } else if (
      previous !== "" &&
      connectionKey === "" &&
      view === "pick-workspace"
    ) {
      // The last connection went away while the chooser was up.
      setChoice({ kind: "local" })
      setView("pick-root")
    }
  }, [open, manageMode, isLocalDesktop, connectionKey, view])

  const refreshLinks = useCallback(async (folderId: number) => {
    setLoadingLinks(true)
    try {
      setLinks(await listFolderLinks(folderId))
    } catch (err) {
      console.error("[WorkspaceFolderDialog] failed to list links:", err)
    } finally {
      setLoadingLinks(false)
    }
  }, [])

  useEffect(() => {
    if (!open || !rootFolder) return
    void refreshLinks(rootFolder.id)
  }, [open, rootFolder, refreshLinks])

  // ── Step 1: workspace root ────────────────────────────────────────────────

  /**
   * Hand a remote folder to its own workspace window. There is no step 2 here:
   * the folder lands in another backend, so linking, renaming and repairing it
   * are that window's job (its folder menu reopens this dialog in manage mode,
   * against the backend that owns the links).
   */
  const commitRemoteRoot = useCallback(
    async (connection: RemoteWorkspaceConnection, path: string) => {
      setOpeningRoot(true)
      try {
        await openRemoteWorkspaceFolder(connection.id, path)
        onOpenChange(false)
      } catch (err) {
        toast.error(t("remoteOpenFailed", { name: connection.name }), {
          description: toErrorMessage(err),
        })
      } finally {
        setOpeningRoot(false)
      }
    },
    [onOpenChange, t]
  )

  const commitRoot = useCallback(
    async (path: string) => {
      if (choice?.kind === "remote") {
        await commitRemoteRoot(choice.connection, path)
        return
      }
      setOpeningRoot(true)
      try {
        const detail = await openFolder(path)
        setRootFolder(detail)
        onFolderOpened?.(detail)
        setView("links")
      } catch (err) {
        toast.error(t("openFailed"), { description: toErrorMessage(err) })
      } finally {
        setOpeningRoot(false)
      }
    },
    [choice, commitRemoteRoot, openFolder, onFolderOpened, t]
  )

  const handleConfirmRoot = useCallback(async () => {
    if (openingRoot || browserBusy) return
    const selected = await rootBrowserRef.current?.confirm()
    if (selected) await commitRoot(selected)
  }, [openingRoot, browserBusy, commitRoot])

  // The picker fills the path box and stops there. It sits inside that box, so
  // it has to behave like typing into it — opening the folder outright would
  // skip the confirm step and land it in the sidebar before the user asked.
  const handleNativeRootPick = useCallback(async () => {
    const selected = await openFileDialog({ directory: true, multiple: false })
    if (!selected) return
    const path = Array.isArray(selected) ? selected[0] : selected
    if (path) setRootPath(path)
  }, [])

  // ── Step 2: link targets ──────────────────────────────────────────────────

  const toggleTarget = useCallback((path: string) => {
    const key = normalizeFsPath(path)
    setPickedTargets((prev) =>
      prev.some((p) => normalizeFsPath(p) === key)
        ? prev.filter((p) => normalizeFsPath(p) !== key)
        : [...prev, path]
    )
  }, [])

  const runPreview = useCallback(
    async (paths: string[]) => {
      if (!rootFolder || paths.length === 0) return
      setPreviewing(true)
      try {
        const plans = await previewFolderLinks(rootFolder.id, paths)
        const { usable, skipped: rejected } = partitionPlans(plans)
        // Merge rather than replace: a second "add" round keeps what the user
        // already queued (and edited) instead of dropping it.
        setPending((prev) => {
          const seen = new Set(prev.map((p) => normalizeFsPath(p.targetPath)))
          const additions = usable
            .filter((plan) => !seen.has(normalizeFsPath(plan.targetPath)))
            .map<PendingLink>((plan) => ({
              targetPath: plan.targetPath,
              name: plan.name,
              baseName: plan.baseName,
              renamed: plan.renamed,
              collidesWithExistingEntry: plan.collidesWithExistingEntry,
            }))
          return [...prev, ...additions]
        })
        setSkipped(rejected)
        setView("links")
      } catch (err) {
        toast.error(t("previewFailed"), { description: toErrorMessage(err) })
      } finally {
        setPreviewing(false)
      }
    },
    [rootFolder, t]
  )

  const handleAddPicked = useCallback(async () => {
    // With nothing ticked, fall back to whatever is in the path box — that's
    // the whole interaction for a typed path. Once anything IS ticked the box
    // only tracks the last row clicked, so honouring it too would re-add a
    // folder the user had just unticked.
    const typed = targetPath.trim()
    const all = pickedTargets.length > 0 ? pickedTargets : typed ? [typed] : []
    setPickedTargets([])
    await runPreview(all)
  }, [targetPath, pickedTargets, runPreview])

  // Same rule as the root picker: stage, don't commit. A multi-select lands in
  // the tick list (which is what "Add" reads) with the box tracking the last
  // one, exactly as if the rows had been clicked in the tree.
  const handleNativeTargetPick = useCallback(async () => {
    const selected = await openFileDialog({ directory: true, multiple: true })
    if (!selected) return
    const paths = (Array.isArray(selected) ? selected : [selected]).filter(
      Boolean
    )
    if (paths.length === 0) return
    setPickedTargets((prev) => {
      const seen = new Set(prev.map(normalizeFsPath))
      return [...prev, ...paths.filter((p) => !seen.has(normalizeFsPath(p)))]
    })
    setTargetPath(paths[paths.length - 1])
  }, [])

  // Names already spoken for, so an inline edit can be validated before saving.
  const takenNames = useMemo(
    () => new Set(links.map((l) => linkNameKey(l.name))),
    [links]
  )

  const pendingIssues = useMemo(() => {
    const issues = new Map<string, LinkNameIssue>()
    const seen = new Set(takenNames)
    for (const item of pending) {
      const issue = validateLinkName(item.name, seen)
      if (issue) issues.set(item.targetPath, issue)
      else seen.add(linkNameKey(item.name))
    }
    return issues
  }, [pending, takenNames])

  const handleCreate = useCallback(async () => {
    if (!rootFolder || pending.length === 0 || pendingIssues.size > 0) return
    setCreating(true)
    try {
      const created = await createFolderLinks(
        rootFolder.id,
        pending.map((item) => ({ path: item.targetPath, name: item.name })),
        gitExclude
      )
      setPending([])
      setSkipped([])
      await refreshLinks(rootFolder.id)
      if (created.length < pending.length) {
        toast.warning(t("partiallyCreated", { count: created.length }))
      }
    } catch (err) {
      toast.error(t("createFailed"), { description: toErrorMessage(err) })
    } finally {
      setCreating(false)
    }
  }, [rootFolder, pending, pendingIssues, gitExclude, refreshLinks, t])

  // ── Existing link actions ─────────────────────────────────────────────────

  const renameIssue = useMemo(() => {
    if (renamingId === null) return null
    const others = new Set(
      links.filter((l) => l.id !== renamingId).map((l) => linkNameKey(l.name))
    )
    return validateLinkName(renameValue, others)
  }, [renamingId, renameValue, links])

  const commitRename = useCallback(async () => {
    if (renamingId === null || renameIssue) return
    const id = renamingId
    setBusyLinkId(id)
    try {
      await renameFolderLink(id, renameValue.trim())
      setRenamingId(null)
      if (rootFolder) await refreshLinks(rootFolder.id)
    } catch (err) {
      toast.error(t("renameFailed"), { description: toErrorMessage(err) })
    } finally {
      setBusyLinkId(null)
    }
  }, [renamingId, renameIssue, renameValue, rootFolder, refreshLinks, t])

  const handleRemove = useCallback(
    async (link: FolderLinkDetail) => {
      setBusyLinkId(link.id)
      try {
        await removeFolderLink(link.id, true)
        if (rootFolder) await refreshLinks(rootFolder.id)
      } catch (err) {
        toast.error(t("removeFailed"), { description: toErrorMessage(err) })
      } finally {
        setBusyLinkId(null)
      }
    },
    [rootFolder, refreshLinks, t]
  )

  const handleRepair = useCallback(
    async (link: FolderLinkDetail) => {
      setBusyLinkId(link.id)
      try {
        await repairFolderLink(link.id)
        if (rootFolder) await refreshLinks(rootFolder.id)
      } catch (err) {
        toast.error(t("repairFailed"), { description: toErrorMessage(err) })
      } finally {
        setBusyLinkId(null)
      }
    },
    [rootFolder, refreshLinks, t]
  )

  // ── Render ────────────────────────────────────────────────────────────────

  const remoteChoice = choice?.kind === "remote" ? choice.connection : null

  const title =
    view === "add-targets"
      ? t("addTargetsTitle")
      : manageMode
        ? t("manageTitle")
        : t("title")

  // Same rule for every row: match the name or the host it reads.
  const query = workspaceQuery.trim().toLowerCase()
  const matchesQuery = (...fields: string[]) =>
    query === "" || fields.some((f) => f.toLowerCase().includes(query))
  const localMatches = matchesQuery(t("thisPc"), t("thisPcHint"))
  const matchingConnections = connections.filter((connection) =>
    matchesQuery(connection.name, connection.base_url)
  )

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {view === "pick-workspace"
              ? t("pickWorkspaceDescription")
              : view === "pick-root"
                ? remoteChoice
                  ? t("pickRootRemoteDescription", { name: remoteChoice.name })
                  : t("pickRootDescription")
                : view === "add-targets"
                  ? t("addTargetsDescription")
                  : t("linksDescription")}
          </DialogDescription>
        </DialogHeader>

        {view === "pick-workspace" ? (
          <>
            <div className="relative">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={workspaceQuery}
                onChange={(e) => setWorkspaceQuery(e.target.value)}
                placeholder={t("searchWorkspaces")}
                className="h-8 pl-8 text-sm"
                autoFocus
              />
            </div>
            <ScrollArea className="h-[18.75rem] rounded-md border">
              <div className="divide-y">
                {localMatches ? (
                  <WorkspaceRow
                    icon={Monitor}
                    name={t("thisPc")}
                    hint={t("thisPcHint")}
                    onSelect={() => {
                      setChoice({ kind: "local" })
                      setView("pick-root")
                    }}
                  />
                ) : null}
                {matchingConnections.map((connection) => (
                  <WorkspaceRow
                    key={connection.id}
                    icon={Server}
                    name={connection.name}
                    hint={connection.base_url}
                    onSelect={() => {
                      setChoice({ kind: "remote", connection })
                      setRootPath("")
                      setView("pick-root")
                    }}
                  />
                ))}
                {!localMatches && matchingConnections.length === 0 ? (
                  <div className="p-6 text-center text-sm text-muted-foreground">
                    {t("noWorkspaceMatch", { query: workspaceQuery.trim() })}
                  </div>
                ) : null}
              </div>
            </ScrollArea>
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() => onOpenChange(false)}
              >
                {tBrowser("cancel")}
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {view === "pick-root" ? (
          <>
            {isLocalDesktop && connections.length > 0 ? (
              <div className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2">
                {remoteChoice ? (
                  <Server className="size-4 shrink-0 text-muted-foreground" />
                ) : (
                  <Monitor className="size-4 shrink-0 text-muted-foreground" />
                )}
                <span className="min-w-0 flex-1 truncate text-xs font-medium">
                  {remoteChoice ? remoteChoice.name : t("thisPc")}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  type="button"
                  className="h-6 shrink-0 px-2 text-xs"
                  onClick={() => setView("pick-workspace")}
                >
                  {t("changeWorkspace")}
                </Button>
              </div>
            ) : null}
            <DirectoryBrowser
              ref={rootBrowserRef}
              active={open && view === "pick-root"}
              value={rootPath}
              onValueChange={setRootPath}
              onQuickSelect={commitRoot}
              onBusyChange={setBrowserBusy}
              fileSystem={remoteFileSystem}
              pathInputAction={
                nativePickerAvailable ? (
                  <NativePickerButton
                    onPick={handleNativeRootPick}
                    disabled={openingRoot}
                  />
                ) : null
              }
            />
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() =>
                  isLocalDesktop && connections.length > 0
                    ? setView("pick-workspace")
                    : onOpenChange(false)
                }
              >
                {isLocalDesktop && connections.length > 0
                  ? t("back")
                  : tBrowser("cancel")}
              </Button>
              <Button
                type="button"
                onClick={handleConfirmRoot}
                disabled={!rootPath.trim() || openingRoot || browserBusy}
              >
                {openingRoot ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : null}
                {remoteChoice ? t("openInWorkspace") : t("next")}
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {view === "add-targets" ? (
          <>
            <DirectoryBrowser
              active={open && view === "add-targets"}
              // Start beside the workspace, not inside it: anything under the
              // root is already reachable and gets rejected, while sibling
              // projects are what people actually link.
              initialPath={linkBrowseStart}
              value={targetPath}
              onValueChange={setTargetPath}
              multiple
              selectedPaths={pickedTargets}
              onToggleSelected={toggleTarget}
              pathInputAction={
                nativePickerAvailable ? (
                  <NativePickerButton
                    onPick={handleNativeTargetPick}
                    disabled={previewing}
                  />
                ) : null
              }
            />
            <DialogFooter>
              <Button
                variant="outline"
                type="button"
                onClick={() => {
                  setPickedTargets([])
                  setView("links")
                }}
              >
                <ArrowLeft className="size-4" />
                {t("back")}
              </Button>
              <Button
                type="button"
                onClick={handleAddPicked}
                disabled={
                  previewing ||
                  (pickedTargets.length === 0 && !targetPath.trim())
                }
              >
                {previewing ? (
                  <Loader2 className="size-4 animate-spin" />
                ) : null}
                {pickedTargets.length > 1
                  ? t("addSelectedCount", { count: pickedTargets.length })
                  : t("addSelected")}
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {view === "links" ? (
          <>
            <div className="flex min-h-0 flex-col gap-3">
              <div className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2">
                <FolderOpen className="size-4 shrink-0 text-muted-foreground" />
                <span
                  className="min-w-0 flex-1 truncate font-mono text-xs"
                  title={rootFolder?.path}
                >
                  {rootFolder?.path ?? ""}
                </span>
                {!manageMode ? (
                  <Button
                    variant="ghost"
                    size="sm"
                    type="button"
                    className="h-6 shrink-0 px-2 text-xs"
                    onClick={() => setView("pick-root")}
                  >
                    {t("change")}
                  </Button>
                ) : null}
              </div>

              <ScrollArea className="min-h-0 flex-1 rounded-md border">
                <div className="divide-y">
                  {loadingLinks &&
                  links.length === 0 &&
                  pending.length === 0 ? (
                    <div className="flex items-center gap-2 p-4 text-sm text-muted-foreground">
                      <Loader2 className="size-3.5 animate-spin" />
                      {tBrowser("loading")}
                    </div>
                  ) : null}

                  {links.map((link) => (
                    <LinkRow
                      key={link.id}
                      link={link}
                      busy={busyLinkId === link.id}
                      editing={renamingId === link.id}
                      renameValue={renameValue}
                      renameIssue={renameIssue}
                      onRenameChange={setRenameValue}
                      onStartRename={() => {
                        setRenamingId(link.id)
                        setRenameValue(link.name)
                      }}
                      onCancelRename={() => setRenamingId(null)}
                      onCommitRename={commitRename}
                      onRemove={() => handleRemove(link)}
                      onRepair={() => handleRepair(link)}
                    />
                  ))}

                  {pending.map((item, index) => (
                    <PendingRow
                      key={item.targetPath}
                      item={item}
                      issue={pendingIssues.get(item.targetPath) ?? null}
                      onNameChange={(name) =>
                        setPending((prev) =>
                          prev.map((p, i) => (i === index ? { ...p, name } : p))
                        )
                      }
                      onRemove={() =>
                        setPending((prev) => prev.filter((_, i) => i !== index))
                      }
                    />
                  ))}

                  {skipped.map((plan) => (
                    <div
                      key={plan.targetPath}
                      className="flex items-start gap-2 px-3 py-2 text-xs text-muted-foreground"
                    >
                      <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-amber-500" />
                      <div className="min-w-0">
                        <div className="truncate font-mono">
                          {plan.targetPath}
                        </div>
                        <div>
                          {plan.rejection === "already_linked" &&
                          plan.existingLinkName
                            ? t("rejection.alreadyLinkedAs", {
                                name: plan.existingLinkName,
                              })
                            : t(`rejection.${plan.rejection ?? "not_found"}`)}
                        </div>
                      </div>
                    </div>
                  ))}

                  {!loadingLinks &&
                  links.length === 0 &&
                  pending.length === 0 &&
                  skipped.length === 0 ? (
                    <div className="p-6 text-center text-sm text-muted-foreground">
                      {t("noLinks")}
                    </div>
                  ) : null}
                </div>
              </ScrollArea>

              <div className="flex items-center justify-between gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  type="button"
                  onClick={() => {
                    setPickedTargets([])
                    setTargetPath("")
                    setView("add-targets")
                  }}
                >
                  <Plus className="size-4" />
                  {t("addFolders")}
                </Button>
                {pending.length > 0 ? (
                  <label className="flex cursor-pointer items-center gap-2 text-xs text-muted-foreground">
                    <Checkbox
                      checked={gitExclude}
                      onCheckedChange={(v) => setGitExclude(v === true)}
                    />
                    {t("gitExclude")}
                  </label>
                ) : null}
              </div>
            </div>

            <DialogFooter>
              {pending.length > 0 ? (
                <>
                  <Button
                    variant="outline"
                    type="button"
                    onClick={() => {
                      setPending([])
                      setSkipped([])
                    }}
                  >
                    {t("discardPending")}
                  </Button>
                  <Button
                    type="button"
                    onClick={handleCreate}
                    disabled={creating || pendingIssues.size > 0}
                  >
                    {creating ? (
                      <Loader2 className="size-4 animate-spin" />
                    ) : null}
                    {t("createCount", { count: pending.length })}
                  </Button>
                </>
              ) : (
                <Button type="button" onClick={() => onOpenChange(false)}>
                  {t("done")}
                </Button>
              )}
            </DialogFooter>
          </>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}

/**
 * Native-picker shortcut that lives at the trailing edge of the path box.
 * Icon-only — the label moves into a tooltip, so the affordance sits next to
 * the thing it fills in instead of competing with the footer's real actions.
 */
function NativePickerButton({
  onPick,
  disabled,
}: {
  onPick: () => void
  disabled: boolean
}) {
  const t = useTranslations("Folder.workspaceDialog")

  return (
    // The app mounts no global tooltip provider — each surface brings its own,
    // and Radix throws without one.
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <InputGroupButton
            size="icon-xs"
            variant="ghost"
            type="button"
            onClick={onPick}
            disabled={disabled}
            aria-label={t("useSystemPicker")}
          >
            <MonitorDot className="size-3.5" />
          </InputGroupButton>
        </TooltipTrigger>
        <TooltipContent>{t("useSystemPicker")}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}

/**
 * One row of the workspace chooser: a full-width target with the workspace name
 * and, underneath, what machine it actually reads (the local machine, or the
 * server URL). Rows commit on click — the chooser is a single decision, so a
 * select-then-confirm step would just add a keystroke.
 */
function WorkspaceRow({
  icon: Icon,
  name,
  hint,
  onSelect,
}: {
  icon: LucideIcon
  name: string
  hint: string
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-muted/50"
    >
      <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-sm font-medium">{name}</span>
        <span className="block truncate font-mono text-xs text-muted-foreground">
          {hint}
        </span>
      </span>
      <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />
    </button>
  )
}

interface PendingLink {
  targetPath: string
  name: string
  baseName: string
  renamed: boolean
  collidesWithExistingEntry: boolean
}

function statusTone(status: FolderLinkStatus) {
  switch (status) {
    case "ok":
      return "text-muted-foreground"
    case "missing":
    case "broken":
      return "text-amber-500"
    case "conflicted":
      return "text-destructive"
  }
}

function LinkRow({
  link,
  busy,
  editing,
  renameValue,
  renameIssue,
  onRenameChange,
  onStartRename,
  onCancelRename,
  onCommitRename,
  onRemove,
  onRepair,
}: {
  link: FolderLinkDetail
  busy: boolean
  editing: boolean
  renameValue: string
  renameIssue: LinkNameIssue | null
  onRenameChange: (value: string) => void
  onStartRename: () => void
  onCancelRename: () => void
  onCommitRename: () => void
  onRemove: () => void
  onRepair: () => void
}) {
  const t = useTranslations("Folder.workspaceDialog")
  const ime = useImeGuard()

  return (
    <div className="flex items-center gap-2 px-3 py-2">
      <Link2
        className={cn("size-4 shrink-0", statusTone(link.status))}
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        {editing ? (
          <div className="flex items-center gap-1">
            <Input
              value={renameValue}
              onChange={(e) => onRenameChange(e.target.value)}
              {...ime.props}
              onKeyDown={(e) => {
                if (ime.isComposing(e)) return
                if (e.key === "Enter") onCommitRename()
                if (e.key === "Escape") onCancelRename()
              }}
              className={cn(
                "h-7 text-sm",
                renameIssue && "border-destructive focus-visible:ring-0"
              )}
              autoFocus
            />
            <Button
              size="icon"
              variant="ghost"
              className="size-7 shrink-0"
              type="button"
              onClick={onCommitRename}
              disabled={!!renameIssue || busy}
              title={t("saveName")}
            >
              <Check className="size-3.5" />
            </Button>
            <Button
              size="icon"
              variant="ghost"
              className="size-7 shrink-0"
              type="button"
              onClick={onCancelRename}
              title={t("cancelRename")}
            >
              <X className="size-3.5" />
            </Button>
          </div>
        ) : (
          <div className="truncate text-sm font-medium">{link.name}</div>
        )}
        {editing && renameIssue ? (
          <div className="mt-0.5 text-xs text-destructive">
            {t(`nameIssue.${renameIssue}`)}
          </div>
        ) : (
          <div
            className="truncate font-mono text-xs text-muted-foreground"
            title={link.targetPath}
          >
            {link.targetPath}
          </div>
        )}
        {link.status !== "ok" ? (
          <div className={cn("mt-0.5 text-xs", statusTone(link.status))}>
            {t(`status.${link.status}`)}
          </div>
        ) : null}
      </div>

      {!editing ? (
        // The app mounts no global tooltip provider — each surface brings its
        // own, and Radix throws without one.
        <TooltipProvider delayDuration={200}>
          <div className="flex shrink-0 items-center gap-0.5">
            {link.status === "missing" || link.status === "broken" ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    size="icon"
                    variant="ghost"
                    className="size-7"
                    type="button"
                    onClick={onRepair}
                    disabled={busy}
                    aria-label={t("repair")}
                  >
                    <RefreshCw className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t("repair")}</TooltipContent>
              </Tooltip>
            ) : null}
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon"
                  variant="ghost"
                  className="size-7"
                  type="button"
                  onClick={onStartRename}
                  disabled={busy}
                  aria-label={t("rename")}
                >
                  <Pencil className="size-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t("rename")}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon"
                  variant="ghost"
                  className="size-7 text-muted-foreground hover:text-destructive"
                  type="button"
                  onClick={onRemove}
                  disabled={busy}
                  aria-label={t("unlink")}
                >
                  {busy ? (
                    <Loader2 className="size-3.5 animate-spin" />
                  ) : (
                    <Link2Off className="size-3.5" />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t("unlink")}</TooltipContent>
            </Tooltip>
          </div>
        </TooltipProvider>
      ) : null}
    </div>
  )
}

function PendingRow({
  item,
  issue,
  onNameChange,
  onRemove,
}: {
  item: PendingLink
  issue: LinkNameIssue | null
  onNameChange: (name: string) => void
  onRemove: () => void
}) {
  const t = useTranslations("Folder.workspaceDialog")

  return (
    <div className="flex items-center gap-2 bg-primary/5 px-3 py-2">
      <Plus className="size-4 shrink-0 text-primary" aria-hidden />
      <div className="min-w-0 flex-1 space-y-1">
        <Input
          value={item.name}
          onChange={(e) => onNameChange(e.target.value)}
          className={cn(
            "h-7 text-sm",
            issue && "border-destructive focus-visible:ring-0"
          )}
        />
        <div
          className="truncate font-mono text-xs text-muted-foreground"
          title={item.targetPath}
        >
          {item.targetPath}
        </div>
        {issue ? (
          <div className="text-xs text-destructive">
            {t(`nameIssue.${issue}`)}
          </div>
        ) : item.renamed ? (
          <div className="text-xs text-amber-600 dark:text-amber-500">
            {item.collidesWithExistingEntry
              ? t("renamedForExistingEntry", { base: item.baseName })
              : t("renamedForDuplicate", { base: item.baseName })}
          </div>
        ) : (
          <div className="text-xs text-muted-foreground">
            {t("willAppearAs", { name: basenameOf(item.targetPath) })}
          </div>
        )}
      </div>
      <Button
        size="icon"
        variant="ghost"
        className="size-7 shrink-0"
        type="button"
        onClick={onRemove}
        title={t("removePending")}
      >
        <Trash2 className="size-3.5" />
      </Button>
    </div>
  )
}
