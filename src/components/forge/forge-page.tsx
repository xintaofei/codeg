"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { useTranslations } from "next-intl"
import {
  Check,
  ExternalLink,
  Funnel,
  GitPullRequestArrow,
  Search,
  X,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command"
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group"
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from "@/components/ui/pagination"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { WorkbenchPageTitle } from "@/components/workbench/workbench-page-title"
import {
  FolderSelect,
  type FolderSelectOption,
} from "@/components/shared/folder-select"
import { ForgeBetaBadge } from "@/components/forge/forge-beta-badge"
import { OPEN_FORGE_SETTINGS_EVENT } from "@/components/forge/forge-chrome-actions"
import { ForgeIssueDetailSheet } from "@/components/forge/forge-issue-detail-sheet"
import { ForgeIssueRowItem } from "@/components/forge/forge-issue-row"
import { ForgeSettingsDialog } from "@/components/forge/forge-settings-dialog"
import { ForgeStartDialog } from "@/components/forge/forge-start-dialog"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  folderForgeRemote,
  forgeListIssues,
  forgeListLabels,
  forgeSettingsGet,
  forgeTabCount,
  openSettingsWindow,
  workTaskLookupBySource,
} from "@/lib/api"
import {
  extractAppCommandError,
  toLocalizedErrorMessage,
  type AppErrorTranslator,
} from "@/lib/app-error"
import { buildForgeSourceKey } from "@/lib/forge-source-key"
import {
  FORGE_PAGE_SIZES,
  loadForgePageSize,
  saveForgePageSize,
  type ForgePageSize,
} from "@/lib/forge-list-prefs"
import { pageCount, pageSlots } from "@/lib/forge-pagination"
import { effectiveForgeSettings } from "@/lib/forge-settings"
import { openUrl, subscribe } from "@/lib/platform"
import { cn } from "@/lib/utils"
import type {
  ForgeIssueList,
  ForgeIssueRow,
  ForgeLabel,
  ForgeRemote,
  ForgeSort,
  ForgeTab,
  ForgeSettingsStore,
  ForgeTaskLink,
} from "@/lib/types"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useForgeRefreshStore } from "@/stores/forge-refresh-store"

const WORK_TASK_CHANGED_EVENT = "task://changed"
const FOLDER_STORAGE_KEY = "forge:folderId"

/** Separates label names inside the counts scope key, so that a label carrying
 *  the outer `:` cannot make two different filter sets read as one scope.
 *
 *  Built at runtime rather than written as an escape sequence, because this
 *  file is where the alternative was found in the wild: the escape had landed
 *  as a real NUL byte, and nothing complained. It compiled, the tests passed,
 *  and git kept diffing it as text — git sniffs only the first few thousand
 *  bytes for NUL, and this one sat well past that. What it did break was
 *  search: grep and rg classify the whole file as binary and return no
 *  matches, so every symbol in here read as one that does not exist. */
const LABEL_SCOPE_SEP = String.fromCharCode(0)

/** Must mirror `NO_ACCOUNT_I18N_KEY` in src-tauri/src/forge/mod.rs. The key —
 *  not the error `code` — is the discriminator: `configuration_missing` is a
 *  generic code that other failures share, and offering "add an account" for
 *  a dead token or a mismatched pin would send the user somewhere useless. */
const NO_ACCOUNT_I18N_KEY = "Forge.errors.noAccount"

/** How long typing has to stop before the filter becomes a request. GitHub's
 *  search endpoint allows THIRTY calls a minute (its own quota, separate from
 *  the 5000/hour core one), so a per-keystroke fetch would exhaust it inside
 *  one sentence. */
const SEARCH_DEBOUNCE_MS = 350

/** Page-number slots the strip may use. Narrow phones cannot fit the desktop
 *  seven next to the page-size select without the footer wrapping twice. */
const PAGE_SLOTS_DESKTOP = 7
const PAGE_SLOTS_COMPACT = 5

/** The three states the list can be narrowed to. `all` is not a third KIND of
 *  item — it is the absence of a state qualifier, which both forges express
 *  (GitHub by leaving `is:` off the query, GitLab by `state=all`). */
type StateFilter = "open" | "closed" | "all"

/** Their labels and count phrasings, keyed so next-intl can check them. */
const STATE_OPTIONS = [
  { value: "open", labelKey: "stateOpen", countKey: "countOpen" },
  { value: "closed", labelKey: "stateClosed", countKey: "countClosed" },
  { value: "all", labelKey: "stateAll", countKey: "countAll" },
] as const satisfies ReadonlyArray<{
  value: StateFilter
  labelKey: string
  countKey: string
}>

/** The pill treatment the workbench toolbars share (see the Tasks page). */
const PILL =
  "h-8 gap-1.5 rounded-full bg-muted/70 px-3 text-[0.8125rem] font-medium ws-msg-chip hover:bg-muted"

/** The four orders both forges can honour, in the order they are offered.
 *  `as const` so the label keys stay literal and next-intl can check them. */
const SORT_OPTIONS = [
  { value: "newest", labelKey: "sortNewest" },
  { value: "oldest", labelKey: "sortOldest" },
  { value: "recently_updated", labelKey: "sortRecentlyUpdated" },
  { value: "least_recently_updated", labelKey: "sortLeastRecentlyUpdated" },
] as const satisfies ReadonlyArray<{ value: ForgeSort; labelKey: string }>

/** The raw thrown value, boxed. Boxed for two reasons: `setState` would CALL a
 *  bare thrown function as an updater, and a raw `null` is indistinguishable
 *  from "no error". Localization happens at render, not here, so the translator
 *  stays out of `fetchPage`'s dependency list — `fetchPage` drives the refetch
 *  effect, and a translator that changed identity would re-fetch on every
 *  render. It also means the message follows a language switch. */
interface ListFailure {
  raw: unknown
}

/**
 * A loaded page together with WHICH list it is a page of.
 *
 * The page is deliberately kept on screen while the next one loads — dimmed,
 * not thrown away, so a filter change or a page turn does not blank out what
 * you were reading. That is only honest while it is the same list, though:
 * change the folder or the tab and those rows stop being a stale view of this
 * list and become rows of a DIFFERENT one, under a switcher and a count badge
 * that now contradict them. So the scope travels with the data, and a page
 * whose scope no longer matches is not shown at all.
 *
 * The folder half of that is a correctness rule, not a cosmetic one: those rows
 * are numbered and linked against a repository the page has stopped showing,
 * and "Start" on one of them would mint a task for that number in the NEW
 * repository — which the folder/remote gate would wave through, because the
 * repository it checks is the new one.
 */
interface LoadedList {
  scope: string
  data: ForgeIssueList
}

/**
 * One badge number, and the filter set it was counted under.
 *
 * `value: null` is "asked, and this forge would not say" — distinct from an
 * absent entry, which is "not asked yet". That difference is what stops a
 * forge that declines to count from being re-asked on every render; both draw
 * no badge.
 *
 * The scope lives on the NUMBER rather than on the pair, and that is the
 * load-bearing part. The two numbers arrive from different places at different
 * times — the visible tab's from its list, the hidden tab's from a probe — so
 * a pair-level scope would have to either drop the number that had not caught
 * up yet (making the badge blink on every filter change) or carry it forward
 * as if it were current, at which point nothing can tell an up-to-date number
 * from a leftover one. Per-number scope keeps both: a leftover can still be
 * SHOWN while its replacement is in flight, and can never be MISTAKEN for an
 * answer that is owed.
 */
interface TabCount {
  scope: string
  value: number | null
}

/**
 * The switcher's two badges.
 *
 * Only one is ever fetched. The tab on screen has its count inside the list
 * response the page already paid for, so a probe is owed only for the tab you
 * cannot see — which is what lets a tab switch, a page turn and a re-order
 * cost nothing. GitHub's search quota is thirty calls a MINUTE, and badges
 * must not compete for it with the list they sit above.
 */
type TabCounts = Partial<Record<ForgeTab, TabCount>>

/**
 * A browser-openable address for the repository.
 *
 * An HTTPS remote already IS the web URL (bar the `.git`), and using it keeps
 * whatever scheme, port and mount path a self-hosted instance runs under. An
 * SSH remote (`git@host:owner/repo.git`, `ssh://…`) is not something a browser
 * can open at all, so those fall back to the canonical https page built from
 * the coordinates the backend derived.
 */
export function repoWebUrl(remote: ForgeRemote): string {
  const url = remote.remote_url.trim()
  if (/^https?:\/\//i.test(url)) return url.replace(/\.git$/i, "")
  return `https://${remote.server_host}/${remote.owner_repo}`
}

export function ForgePageTitle() {
  const t = useTranslations("Forge")
  return <WorkbenchPageTitle title={t("title")} badge={<ForgeBetaBadge />} />
}

function loadStoredFolderId(): number | null {
  if (typeof window === "undefined") return null
  const raw = window.localStorage.getItem(FOLDER_STORAGE_KEY)
  const parsed = raw == null ? NaN : Number.parseInt(raw, 10)
  return Number.isFinite(parsed) ? parsed : null
}

/**
 * The forge workbench: pick a project folder, see its repository's issues and
 * PRs, and turn an issue into a work task with one click. The row chips are a
 * reverse lookup by source key, refreshed on the same `task://changed` nudges
 * the board listens to — the page itself holds no task state.
 *
 * Paging is server-side and by PAGE NUMBER (see `forge/mod.rs ForgeIssueList`),
 * so one page is one request and the footer can offer real numbers. So are the
 * text, label and sort filters: filtering one already-fetched page would show a
 * different list than the count and the page numbers describe. The page SIZE is
 * remembered across sessions; the page number deliberately is not — coming back
 * to page 7 of a list that has since moved is worse than page 1.
 */
export function ForgePage() {
  const t = useTranslations("Forge")
  // Root-scoped translator: backend errors carry FULL dotted keys
  // (`Forge.errors.noAccount`), which the namespaced `t` above cannot resolve.
  // next-intl's typed `t` is widened to the loose shape app-error expects.
  const tRoot = useTranslations()
  const isMobile = useIsMobile()
  const folders = useAppWorkspaceStore((s) => s.folders)
  const projectFolders = useMemo(
    () => folders.filter((f) => f.parent_id == null && f.kind === "regular"),
    [folders]
  )

  const [folderId, setFolderId] = useState<number | null>(loadStoredFolderId)
  // Fall back to the first project folder once folders arrive.
  const effectiveFolderId = useMemo(() => {
    if (folderId != null && projectFolders.some((f) => f.id === folderId)) {
      return folderId
    }
    return projectFolders[0]?.id ?? null
  }, [folderId, projectFolders])

  const [remote, setRemote] = useState<ForgeRemote | null>(null)
  const [remoteLoading, setRemoteLoading] = useState(false)
  const [tab, setTab] = useState<ForgeTab>("issues")
  // Open by default: a triage list opens on the work that is still work.
  const [stateFilter, setStateFilter] = useState<StateFilter>("open")
  const [assignedMe, setAssignedMe] = useState(false)
  const [labelFilter, setLabelFilter] = useState<string[]>([])
  // Two values on purpose: the field follows every keystroke, the REQUEST
  // follows the debounced one.
  const [searchInput, setSearchInput] = useState("")
  const [search, setSearch] = useState("")
  const [sort, setSort] = useState<ForgeSort>("newest")
  const [page, setPage] = useState(1)
  // Restored synchronously: this page only mounts after a client-side route
  // switch, so there is no SSR markup for the remembered size to mismatch.
  const [pageSize, setPageSize] = useState<ForgePageSize>(loadForgePageSize)
  const [loaded, setLoaded] = useState<LoadedList | null>(null)
  const [loading, setLoading] = useState(false)
  const [counts, setCounts] = useState<TabCounts>({})
  /** How many probes are outstanding, not whether one is: both tabs can be in
   *  flight at once (switch during a cold load), and a boolean would be
   *  cleared by whichever finished first. */
  const [countsInFlight, setCountsInFlight] = useState(0)
  const countsLoading = countsInFlight > 0
  const [error, setError] = useState<ListFailure | null>(null)
  const [links, setLinks] = useState<Map<string, ForgeTaskLink>>(new Map())
  const [startRow, setStartRow] = useState<ForgeIssueRow | null>(null)
  /** The item the right-side detail panel is open on, or `null` for closed. */
  const [detailRow, setDetailRow] = useState<ForgeIssueRow | null>(null)
  /** The panel's preferences, EVERY scope — what a trigger dialog OPENS with,
   *  and nothing else this page reads. Loaded once and replaced in place when
   *  the settings dialog saves; `null` means "not loaded yet, or the read
   *  failed", which the trigger dialog treats as the built-in defaults rather
   *  than as a reason to wait. Held as the whole store rather than as one
   *  folder's resolved values so switching folders costs no round trip. */
  const [settings, setSettings] = useState<ForgeSettingsStore | null>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [labelOptions, setLabelOptions] = useState<ForgeLabel[]>([])
  const [labelsTruncated, setLabelsTruncated] = useState(false)
  const reqRef = useRef(0)
  /** Generation counter for the task-link lookup (see `refreshLinks`). */
  const linksReqRef = useRef(0)
  /** Request generations kept PER TAB. One shared counter would make a probe
   *  for Issues invalidate an in-flight probe for pull requests, so toggling
   *  the switcher during a slow load would throw away each answer as the next
   *  one started and re-spend it on the way back. */
  const countsReqRef = useRef<Record<ForgeTab, number>>({ issues: 0, prs: 0 })
  /** `scope:tab` of each tab's in-flight probe, so toggling back before one
   *  lands does not fire a second copy of it. */
  const probingRef = useRef<Record<ForgeTab, string | null>>({
    issues: null,
    prs: null,
  })

  // Folder → remote resolution. Everything the PREVIOUS repository produced
  // goes with it: its rows would still be numbered and linked against a
  // repository this page is no longer showing, and a trigger dialog left open
  // over the switch would mint a task for that row's number in the NEW
  // repository. The detail panel goes for the same reason — it carries its own
  // copy of a row, and its footer offers that same trigger. (`scope` keeps the
  // rows off screen either way; this is what stops them lingering in memory and
  // the dialog and panel from surviving at all.)
  //
  // The clean-up runs BEFORE the "no folder at all" exit rather than inside the
  // branch that resolves one, so the two paths cannot drift apart. They did:
  // losing the last project folder took `effectiveFolderId` to null, which
  // returned early and left the panel mounted over a page that had gone back to
  // "pick a folder" — showing an item of a repository no longer on screen, with
  // a "Start" whose dialog is gated on a folder and so did nothing at all.
  useEffect(() => {
    setRemote(null)
    setLoaded(null)
    setCounts({})
    setStartRow(null)
    setDetailRow(null)
    if (effectiveFolderId == null) return
    let cancelled = false
    setRemoteLoading(true)
    folderForgeRemote(effectiveFolderId)
      .then((r) => {
        if (!cancelled) setRemote(r)
      })
      .catch(() => {
        if (!cancelled) setRemote(null)
      })
      .finally(() => {
        if (!cancelled) setRemoteLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [effectiveFolderId])

  /** Which list the rows belong to — see [`LoadedList`]. */
  const listScope = `${effectiveFolderId}:${tab}`
  /** Which RESULT SET the badges count — see [`TabCounts`]. No tab, no page,
   *  no order: none of the three can change either number. */
  const countsScope = `${effectiveFolderId}:${stateFilter}:${assignedMe}:${labelFilter.join(LABEL_SCOPE_SEP)}:${search}`
  /** The only tab a probe is ever spent on. */
  const otherTab: ForgeTab = tab === "issues" ? "prs" : "issues"

  /**
   * File one badge number under the filter set it was counted for.
   *
   * The other tab's entry is left exactly as it was, scope and all: the two
   * are independent facts that arrive independently, and merging them into one
   * "current pair" is what would let a leftover number inherit a scope nobody
   * counted it under.
   */
  const recordCount = useCallback(
    (scope: string, which: ForgeTab, value: number | null) => {
      setCounts((prev) => ({ ...prev, [which]: { scope, value } }))
    },
    []
  )

  /**
   * Claim one tab's count slot for an answer that is about to arrive, and get
   * back the generation that says so.
   *
   * BOTH writers go through this, and that is the point. A list is every bit
   * as authoritative about the tab it just listed as a probe is — so a probe
   * that was already in the air when the list answered is not "the other
   * writer", it is an OLDER answer about the same number, and it has to be
   * invalidated exactly as a superseded probe would be. Without this, changing
   * the filters while on a tab lets that tab's stale probe land on top of the
   * fresh count its own list just brought back, and nothing is left in flight
   * to correct it.
   */
  const claimCount = useCallback((which: ForgeTab) => {
    probingRef.current[which] = null
    countsReqRef.current[which] += 1
    return countsReqRef.current[which]
  }, [])

  const fetchPage = useCallback(async () => {
    if (effectiveFolderId == null || remote == null) return
    const id = ++reqRef.current
    setLoading(true)
    setError(null)
    try {
      const result = await forgeListIssues(effectiveFolderId, {
        tab,
        state: stateFilter,
        assignedMe,
        labels: labelFilter,
        search,
        sort,
        page,
        perPage: pageSize,
      })
      if (id !== reqRef.current) return
      setLoaded({ scope: listScope, data: result })
      // The badge for the tab on screen, free: this response already counted
      // exactly what a probe would have. `incomplete` withholds it — GitHub
      // says the search timed out, so the number is short of the truth, and a
      // bare digit on a tab has nowhere to admit that (the footer, which has
      // the room, says it there instead).
      claimCount(tab)
      recordCount(
        countsScope,
        tab,
        result.incomplete ? null : result.total_count
      )
    } catch (e) {
      if (id !== reqRef.current) return
      setLoaded(null)
      setError({ raw: e })
    } finally {
      if (id === reqRef.current) setLoading(false)
    }
  }, [
    effectiveFolderId,
    remote,
    listScope,
    countsScope,
    claimCount,
    recordCount,
    tab,
    stateFilter,
    assignedMe,
    labelFilter,
    search,
    sort,
    page,
    pageSize,
  ])

  /**
   * The OTHER tab's badge — the one number this page cannot get for free.
   *
   * A failure is a missing badge, never an error: the list reports its own
   * problems with its own message, and a switcher that lost a digit is not a
   * page that lost anything.
   */
  const fetchCount = useCallback(
    async (which: ForgeTab) => {
      if (effectiveFolderId == null || remote == null) return
      const scope = countsScope
      const id = claimCount(which)
      probingRef.current[which] = `${scope}:${which}`
      setCountsInFlight((n) => n + 1)
      try {
        const value = await forgeTabCount(effectiveFolderId, which, {
          state: stateFilter,
          assignedMe,
          labels: labelFilter,
          search,
        })
        if (id !== countsReqRef.current[which]) return
        recordCount(scope, which, value)
      } catch {
        if (id !== countsReqRef.current[which]) return
        // Recorded as "asked, no answer" rather than left unknown, or the
        // effect below would ask again on the very next render.
        recordCount(scope, which, null)
      } finally {
        // Unconditional: a probe that has been superseded still has to give
        // its slot back, or the header stays marked busy for good.
        setCountsInFlight((n) => n - 1)
        if (id === countsReqRef.current[which]) probingRef.current[which] = null
      }
    },
    [
      effectiveFolderId,
      remote,
      countsScope,
      claimCount,
      recordCount,
      stateFilter,
      assignedMe,
      labelFilter,
      search,
    ]
  )

  // (Re)load on any filter/paging/remote change. `page` is in `fetchPage`'s
  // deps, so a page click IS the refetch — there is no separate trigger.
  useEffect(() => {
    if (remote != null) void fetchPage()
  }, [remote, fetchPage])

  // Read through a ref so the probe below can consult the numbers it writes
  // without depending on them — a dependency there would re-arm the effect
  // with its own answer. Synced in an effect DECLARED FIRST, so it has already
  // run by the time the probe's effect reads it in the same commit; assigning
  // during render would let a render React threw away decide what to skip.
  const countsRef = useRef<TabCounts>(counts)
  useEffect(() => {
    countsRef.current = counts
  }, [counts])

  // Probe the hidden tab, and only when its number is neither in hand nor
  // already on its way FOR THESE EXACT FILTERS. Switching tabs therefore costs
  // nothing in the settled case: the number the user is switching TO arrived
  // with its own list, and the one they are switching AWAY from is already
  // filed under this same scope. Mid-load it costs at most one probe per tab,
  // because the second condition recognizes its own request.
  //
  // Both conditions test the scope, and that is the point: a number left over
  // from filters that no longer apply is not an answer to this question, so it
  // must not be what stops the question from being asked.
  useEffect(() => {
    if (countsRef.current[otherTab]?.scope === countsScope) return
    if (probingRef.current[otherTab] === `${countsScope}:${otherTab}`) return
    void fetchCount(otherTab)
  }, [countsScope, otherTab, fetchCount])

  // Reloading the whole list is the one control that does not narrow it, so it
  // lives in the window chrome next to the settings gear rather than in the
  // toolbar. That button is in another branch of the tree entirely; this is how
  // it reaches the fetch — and how it learns there is nothing to fetch yet.
  const publishRefresh = useForgeRefreshStore((s) => s.publish)
  const withdrawRefresh = useForgeRefreshStore((s) => s.withdraw)
  useEffect(() => {
    publishRefresh({
      refresh:
        remote == null
          ? null
          : () => {
              // Both, or "reload" would leave a stale number sitting on the
              // switcher above a list that had just been refreshed under it.
              // The visible tab's badge rides along with `fetchPage`; this is
              // the one the user cannot see, asked for unconditionally because
              // "reload" means reload, cached or not.
              void fetchPage()
              void fetchCount(otherTab)
            },
      busy: loading || countsLoading,
    })
  }, [
    publishRefresh,
    fetchPage,
    fetchCount,
    otherTab,
    remote,
    loading,
    countsLoading,
  ])
  // Only on unmount: leaving a handler behind would give the next route's
  // chrome a live button pointing at a page that no longer exists.
  useEffect(() => () => withdrawRefresh(), [withdrawRefresh])

  // The chrome cluster's gear, and the preferences it edits. One read for the
  // page's whole life: the store holds every scope, it only changes through the
  // dialog next to this listener, and that dialog hands the stored values
  // straight back. A failure is silent on purpose — the trigger dialog falls
  // back to the built-in defaults, and a toast about preferences nobody asked
  // for yet would be noise over a page that works.
  useEffect(() => {
    let cancelled = false
    forgeSettingsGet()
      .then((s) => {
        if (!cancelled) setSettings(s)
      })
      .catch(() => {})
    const open = () => setSettingsOpen(true)
    window.addEventListener(OPEN_FORGE_SETTINGS_EVENT, open)
    return () => {
      cancelled = true
      window.removeEventListener(OPEN_FORGE_SETTINGS_EVENT, open)
    }
  }, [])

  // The typed text becomes a request only once typing stops. `search` is in the
  // dependency list so the settled value short-circuits the next run instead of
  // re-arming the timer forever.
  useEffect(() => {
    if (searchInput.trim() === search) return
    const handle = setTimeout(() => {
      setPage(1)
      setSearch(searchInput.trim())
    }, SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(handle)
  }, [searchInput, search])

  // A different repository has a different label vocabulary, so a selection
  // made against the old one would filter by labels that may not exist here.
  // Derived during render rather than in an effect: this has to catch the
  // FALLBACK path too (the stored folder disappearing from the workspace), and
  // an effect would spend an extra render — and an extra request — doing it.
  const [labelledFolder, setLabelledFolder] = useState(effectiveFolderId)
  if (labelledFolder !== effectiveFolderId) {
    setLabelledFolder(effectiveFolderId)
    if (labelFilter.length > 0) {
      setLabelFilter([])
      setPage(1)
    }
  }

  // The repository's label vocabulary — once per repository, not per page:
  // labels barely change, and on GitHub this runs on the core quota rather than
  // search's much smaller one. Best-effort: a repository whose labels cannot be
  // read still lists perfectly well, it just offers no label filter.
  useEffect(() => {
    if (effectiveFolderId == null || remote == null) return
    let cancelled = false
    setLabelOptions([])
    setLabelsTruncated(false)
    forgeListLabels(effectiveFolderId)
      .then((result) => {
        if (cancelled) return
        setLabelOptions(result.labels)
        setLabelsTruncated(result.truncated)
      })
      .catch(() => {
        /* no filter rather than no list */
      })
    return () => {
      cancelled = true
    }
  }, [effectiveFolderId, remote])

  /** Any change that redefines the result set puts you back on page 1 —
   *  otherwise a narrower filter lands on a page number that no longer
   *  exists and the list reads as empty. */
  const resetTo = useCallback(<T,>(apply: () => T) => {
    setPage(1)
    return apply()
  }, [])

  /** The page currently on screen, or nothing when the last one belongs to a
   *  list this is no longer showing (see [`LoadedList`]). */
  const list = loaded?.scope === listScope ? loaded.data : null
  const rows = useMemo(() => list?.rows ?? [], [list])

  /**
   * Everything on screen that DESCRIBES the list is out of date.
   *
   * ONE flag over three sources, and that is the whole fix: a control responds
   * to a click instantly — it has to, it is a control — while the rows, the
   * counts and the page numbers can only answer a round trip later, and letting
   * each of them arrive on its own schedule is what made a slow forge look
   * broken rather than slow. So they age together and clear together.
   *
   * Typing counts as pending before the debounce has even fired: the moment the
   * box says something the list does not, the list is stale, and saying so is
   * more useful than a third of a second of pretending otherwise.
   */
  const pending = loading || countsLoading || searchInput.trim() !== search

  /**
   * What one tab's badge shows right now.
   *
   * A number counted under filters that no longer apply is shown only while
   * something is still in flight — dimmed with everything else, and about to be
   * corrected. Once the page settles, a number that could not be brought up to
   * date is dropped instead: a dimmed stale digit is a promise to fix it, and
   * an undimmed one is just a wrong answer.
   */
  const badge = (which: ForgeTab) => {
    const held = counts[which]
    if (held == null) return undefined
    return held.scope === countsScope || pending ? held.value : undefined
  }

  /** Placeholder rows, capped: enough to fill a window, not a hundred of them
   *  because the page size says so. */
  const skeletonRows = Math.min(pageSize, 8)

  // Reverse lookup for visible rows; re-run on board nudges so a chip follows
  // its task through the pipeline without polling.
  const keyFor = useCallback(
    (row: ForgeIssueRow) =>
      remote == null
        ? null
        : buildForgeSourceKey({
            // The backend decided which forge this host is; using anything
            // else here builds keys that match no task's provenance.
            provider: remote.provider,
            serverHost: remote.server_host,
            ownerRepo: remote.owner_repo,
            kind: row.is_pr ? "pr" : "issue",
            number: row.number,
          }),
    [remote]
  )
  /** The latest task that has ever handled a row, if any. Shared by the list
   *  and the detail panel, so both read the same chip off the same lookup. */
  const linkFor = useCallback(
    (row: ForgeIssueRow) => {
      const key = keyFor(row)
      return key != null ? (links.get(key) ?? null) : null
    },
    [keyFor, links]
  )
  /**
   * The panel's item, re-read from the list on every render.
   *
   * The panel is opened with the row that was clicked, and a row is a SNAPSHOT
   * — reload the list (or turn a filter) and the item's title, state, labels
   * and body all arrive again in a new object. Matching by identity keeps the
   * panel on the fresh copy, so a refresh behind it updates what it shows
   * instead of leaving it frozen at whatever the list said when it opened.
   *
   * It falls back to the held snapshot when the item is not in the page any
   * more — a tab switch, a page turn or a narrowed filter takes the row away
   * without saying anything about the ITEM, and blanking a panel someone is
   * reading is the one thing worse than showing a slightly stale copy.
   */
  const detail = useMemo(() => {
    if (detailRow == null) return null
    return (
      rows.find(
        (r) => r.is_pr === detailRow.is_pr && r.number === detailRow.number
      ) ?? detailRow
    )
  }, [rows, detailRow])

  const refreshLinks = useCallback(async () => {
    // Its own generation counter, not `reqRef`: this stream and the list fetch
    // run on different triggers (a `work-task://changed` event refreshes links
    // without touching the list), so sharing one would have each cancel the
    // other's answer.
    //
    // The guard is load-bearing rather than hygiene. The answer REPLACES this
    // map wholesale, and three sources fire it — the deps below, the task
    // event, and the trigger dialog — so two lookups are routinely in flight
    // at once. Without a generation the SLOWER one wins whatever order they
    // were sent in, dropping links that exist; the detail panel reads a
    // missing link as "no task yet" and offers "Start" for work that is
    // already running (`forge-issue-detail-sheet.tsx`), which is how a stale
    // response turns into a duplicate task.
    const id = ++linksReqRef.current
    // The panel's item is asked about too, and not only while it is on screen.
    // It deliberately outlives the row it was opened from (see `detail`), and
    // the answer REPLACES this map wholesale — so a page turn, a narrowed
    // filter or a tab switch used to drop that item's task along with its row,
    // and the panel's footer fell back from a live status chip to "Start",
    // offering to trigger work that was already running. Reference equality is
    // the right test: `detail` IS the row object when the list still holds it,
    // and only a panel outliving its row adds a key here.
    const wanted =
      detail == null || rows.includes(detail) ? rows : [...rows, detail]
    const keys = wanted
      .map((r) => keyFor(r))
      .filter((k): k is string => k != null)
    if (keys.length === 0) {
      // Claimed the generation above, so this clear is ordered against the
      // in-flight lookups like any other answer: it lands only while it is
      // still the newest word, and a lookup sent after it cannot be undone by
      // it either.
      if (id === linksReqRef.current) setLinks(new Map())
      return
    }
    try {
      const found = await workTaskLookupBySource(keys)
      if (id !== linksReqRef.current) return
      setLinks(new Map(found.map((l) => [l.source_key, l])))
    } catch {
      // Chips are best-effort decoration; the list itself stays useful.
    }
  }, [rows, detail, keyFor])
  useEffect(() => {
    void refreshLinks()
  }, [refreshLinks])
  useEffect(() => {
    let cancelled = false
    let unsub: (() => void) | undefined
    void subscribe(WORK_TASK_CHANGED_EVENT, () => {
      void refreshLinks()
    }).then((u: () => void) => {
      if (cancelled) u()
      else unsub = u
    })
    return () => {
      cancelled = true
      unsub?.()
    }
  }, [refreshLinks])

  const pickFolder = useCallback(
    (id: number) => {
      resetTo(() => setFolderId(id))
      window.localStorage.setItem(FOLDER_STORAGE_KEY, String(id))
    },
    [resetTo]
  )

  // A rejected `invoke()` hands back the SERIALIZED AppCommandError — a plain
  // object, not an Error, so `String(e)` on it renders the literal text
  // "[object Object]". app-error unwraps `{code, message, detail}` and prefers
  // the backend's `i18n_key` when it sent one.
  const failure = useMemo(() => {
    if (error == null) return null
    return {
      message: toLocalizedErrorMessage(
        error.raw,
        tRoot as unknown as AppErrorTranslator
      ),
      needsAccount:
        extractAppCommandError(error.raw)?.i18n_key === NO_ACCOUNT_I18N_KEY,
    }
  }, [error, tRoot])

  const changeNounKey = remote?.provider === "gitlab" ? "tabMrs" : "tabPrs"

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* `relative` for the pending bar, which rides the bottom edge. */}
      <div className="relative flex shrink-0 flex-col gap-2.5 border-b border-border/60 px-4 pb-2.5 pt-3">
        {/* WHERE the list comes from, and WHICH kind of thing is in it. No
            heading: the switcher already names what you are looking at, in a
            control you can act on, and the route's own name is in the chrome
            strip directly above — three words for one fact was two too many.
            The count went with it, onto the switcher, where it also says
            something about the tab you are NOT on.

            Two ends, nothing between: the source on the left, the switcher
            pinned right. Everything that merely NARROWS the list is a row
            down, the search box included. */}
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <RepoBar
            folders={projectFolders}
            folderId={effectiveFolderId}
            onPickFolder={pickFolder}
            remote={remote}
          />

          <Tabs
            className="shrink-0 sm:ms-auto"
            value={tab}
            onValueChange={(v) => resetTo(() => setTab(v as ForgeTab))}
          >
            <TabsList className="h-8">
              <TabsTrigger value="issues">
                {t("tabIssues")}
                <TabCount
                  value={badge("issues")}
                  state={stateFilter}
                  pending={pending}
                />
              </TabsTrigger>
              <TabsTrigger value="prs">
                {t(changeNounKey)}
                <TabCount
                  value={badge("prs")}
                  state={stateFilter}
                  pending={pending}
                />
              </TabsTrigger>
            </TabsList>
          </Tabs>
        </div>

        {/* One family of h-8 controls, all of them narrowing the same list —
            WHERE it comes from is the row above. */}
        <div className="flex flex-wrap items-center gap-2">
          {/* Leads the row: it is the widest net of the four, and the three
              pills after it narrow whatever it caught. A whole row to itself
              on a phone (`w-full` forces the wrap) — sharing one would leave
              too little of it to read back what you typed. */}
          <InputGroup className="h-8 w-full rounded-full sm:w-64">
            <InputGroupAddon align="inline-start">
              <Search className="size-3.5" aria-hidden />
            </InputGroupAddon>
            <InputGroupInput
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape" && searchInput !== "") {
                  e.preventDefault()
                  setSearchInput("")
                }
              }}
              placeholder={t("searchPlaceholder")}
              aria-label={t("searchPlaceholder")}
              className="h-8 text-[0.8125rem]"
            />
            {searchInput !== "" ? (
              <InputGroupAddon align="inline-end">
                <InputGroupButton
                  size="icon-xs"
                  aria-label={t("clearSearch")}
                  title={t("clearSearch")}
                  onClick={() => setSearchInput("")}
                >
                  <X className="size-3" />
                </InputGroupButton>
              </InputGroupAddon>
            ) : null}
          </InputGroup>

          {/* Was a button whose LABEL changed on click, which reads equally as
              "you are here" and "go here". Named options remove the guess —
              and "All" is one of them, because "open" and "closed" together
              are not everything: a merged pull request is in neither on
              GitLab, and neither tab shows you a whole history. */}
          <Select
            value={stateFilter}
            onValueChange={(v) =>
              resetTo(() => setStateFilter(v as StateFilter))
            }
          >
            <SelectTrigger
              size="sm"
              aria-label={t("stateFilter")}
              className="h-8 w-auto gap-1.5 rounded-full border-transparent bg-muted/70 px-3 text-[0.8125rem] font-medium shadow-none hover:bg-muted"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STATE_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {t(option.labelKey)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          {/* A toggle, so its OFF state is still a control: as a bare ghost
              button it had no surface at all and read as floating text. */}
          <Button
            type="button"
            size="sm"
            variant="ghost"
            aria-pressed={assignedMe}
            className={cn(
              PILL,
              assignedMe &&
                "bg-primary/10 text-primary hover:bg-primary/15 dark:hover:bg-primary/15"
            )}
            onClick={() => resetTo(() => setAssignedMe((v) => !v))}
          >
            {t("assignedMe")}
          </Button>

          {/* Hidden entirely when the repository has no labels: a control that
              can only ever open an empty list is worse than no control. */}
          {labelOptions.length > 0 || labelFilter.length > 0 ? (
            <LabelFilter
              options={labelOptions}
              truncated={labelsTruncated}
              selected={labelFilter}
              onChange={(next) => resetTo(() => setLabelFilter(next))}
            />
          ) : null}

          {/* Trails the row on a wide screen; on a phone it just wraps in
              flow, because `ms-auto` there would strand it on its own line. */}
          <Select
            value={sort}
            onValueChange={(v) => resetTo(() => setSort(v as ForgeSort))}
          >
            <SelectTrigger
              size="sm"
              aria-label={t("sortBy")}
              className="h-8 w-auto gap-1.5 rounded-full border-transparent px-3 text-[0.8125rem] font-medium text-muted-foreground shadow-none hover:bg-muted sm:ms-auto"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {SORT_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {t(option.labelKey)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {pending ? <PendingBar /> : null}
      </div>

      {/* The app's own scrollbar (OverlayScrollbars, same as the sidebar's
          conversation list) rather than the platform's. Besides matching, it
          OVERLAYS: a native scrollbar eats a column of the viewport, which
          pulled every row's trailing action a scrollbar's width left of the
          filters directly above them. */}
      <ScrollArea className="min-h-0 flex-1">
        {effectiveFolderId == null ? (
          <EmptyHint text={t("pickFolder")} />
        ) : remoteLoading ? (
          <ListSkeleton rows={skeletonRows} />
        ) : remote == null ? (
          <EmptyHint text={t("noRemote")} />
        ) : failure != null ? (
          <EmptyHint
            text={failure.message}
            action={
              // Only for the ONE failure adding an account fixes. A dead token
              // or a stale pinned account id lands here too, and sending those
              // to the "add" flow would be advice that cannot work.
              failure.needsAccount ? (
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => {
                    void openSettingsWindow("version-control").catch(() => {
                      // Settings is a separate window; a failure to open it
                      // must not take down the page that offered the button.
                    })
                  }}
                >
                  {t("addAccount")}
                </Button>
              ) : null
            }
          />
        ) : list == null ? (
          // Nothing to keep: either the first page has not arrived yet, or the
          // last one belongs to a repository/tab this is no longer showing.
          <ListSkeleton rows={skeletonRows} />
        ) : rows.length === 0 ? (
          <EmptyHint text={page > 1 ? t("emptyPage") : t("empty")} />
        ) : (
          // Still the page you were reading, marked as no longer current. The
          // alternative — swapping in a skeleton on every filter change —
          // throws away the thing you are looking at to tell you it is being
          // replaced.
          <div
            aria-busy={pending}
            className={cn(
              "flex flex-col divide-y divide-border/40 transition-opacity",
              pending && "opacity-50"
            )}
          >
            {rows.map((row) => (
              <ForgeIssueRowItem
                key={`${row.is_pr ? "pr" : "issue"}-${row.number}`}
                row={row}
                link={linkFor(row)}
                compact={isMobile}
                onOpenDetail={() => setDetailRow(row)}
                onStart={() => setStartRow(row)}
              />
            ))}
          </div>
        )}
      </ScrollArea>

      {remote != null && failure == null ? (
        list != null ? (
          <ListFooter
            list={list}
            pageSize={pageSize}
            busy={pending}
            compact={isMobile}
            onPage={setPage}
            onPageSize={(size) => {
              // The remembered size is the one the user chose, not the one a
              // later session happened to land on.
              saveForgePageSize(size)
              resetTo(() => setPageSize(size))
            }}
          />
        ) : (
          <FooterPlaceholder />
        )
      ) : null}

      {/* Before the trigger dialog in the tree, so the dialog's portal lands
          after the panel's and covers it: "Start" from inside the panel leaves
          the panel open behind the dialog, and comes back to a footer that now
          carries the new task's chip. (The drawer survives the dialog on its
          own — see the Radix-layer shield in `drawer.tsx`.) */}
      <ForgeIssueDetailSheet
        row={detail}
        link={detail != null ? linkFor(detail) : null}
        onOpenChange={(open) => {
          if (!open) setDetailRow(null)
        }}
        onStart={() => {
          if (detail != null) setStartRow(detail)
        }}
      />

      {startRow != null && remote != null && effectiveFolderId != null ? (
        <ForgeStartDialog
          row={startRow}
          remote={remote}
          folderId={effectiveFolderId}
          // Resolved for the folder on screen: its own panel settings if it has
          // any, else the global row (see `effectiveForgeSettings`).
          settings={effectiveForgeSettings(settings, effectiveFolderId)}
          onClose={() => setStartRow(null)}
          onCreated={() => {
            setStartRow(null)
            void refreshLinks()
          }}
        />
      ) : null}

      <ForgeSettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        // Opens on the folder whose list you were looking at — the scope you
        // are almost certainly there to change — with the picker inside to go
        // global or elsewhere.
        folderId={effectiveFolderId}
        // Kept in the page rather than re-fetched: the next trigger dialog
        // opens on what was just saved, and the read that seeded this page may
        // have happened minutes ago.
        onSaved={setSettings}
      />
    </div>
  )
}

/**
 * How many items this tab holds, under the filters currently in force.
 *
 * On the switcher rather than under a heading, because the count that earns
 * its place is the one for the tab you are NOT on: "3 pull requests" is a
 * reason to click, and a number that only ever described the side you were
 * already looking at was decoration. Filtered, not repository-wide — it
 * replaces a caption that was filtered, and it has to agree with the page
 * numbers at the bottom of the same screen.
 *
 * Absent, never zero, when the forge declined to count (GitLab past 10k rows,
 * and its locally-filtered closed-MR query): "0" is a claim, and nobody made it.
 */
function TabCount({
  value,
  state,
  pending,
}: {
  value: number | null | undefined
  /** Which state the number counts — the badge is a bare digit, and "12 open"
   *  and "12 in total" are very different things to have found. */
  state: StateFilter
  /** Out of date — dimmed in step with the rows it counts, rather than left
   *  looking authoritative next to a list that is visibly reloading. */
  pending: boolean
}) {
  const t = useTranslations("Forge")
  if (value == null) return null
  const countKey =
    STATE_OPTIONS.find((option) => option.value === state)?.countKey ??
    "countOpen"
  return (
    <span
      // The tab's accessible name becomes "Issues 12", which is the whole
      // point; the title is what says WHICH twelve.
      title={t(countKey, { count: value })}
      className={cn(
        "rounded-full bg-foreground/10 px-1.5 py-0.5 text-[0.6875rem] font-medium leading-none tabular-nums transition-opacity",
        pending && "opacity-40"
      )}
    >
      {value}
    </span>
  )
}

/**
 * A request is out.
 *
 * On the header's own edge, because it is the one mark that can appear and
 * disappear without moving anything: a spinner in the toolbar takes width from
 * a control, and a screen of skeletons throws away the list you are still
 * reading. It is deliberately the only MOVING thing on the page while a fetch
 * runs — everything else just dims.
 *
 * A sweep ALONG the divider, not a bar under it. The first cut was two solid
 * pixels of `bg-primary`, and this theme's primary is a neutral near-black
 * (`oklch(0.205 0 0)`) — so on a light background it read as a stray black
 * rule someone had left in the corner of the list, which is exactly how it got
 * reported. One pixel, fading out at both ends, sitting ON the border that is
 * already there: nothing is added to the layout, the line you can already see
 * just brightens and travels.
 */
function PendingBar() {
  return (
    <span
      aria-hidden
      className="pointer-events-none absolute inset-x-0 -bottom-px h-px overflow-hidden"
    >
      <span className="forge-progress block h-full w-1/3 bg-gradient-to-r from-transparent via-primary/70 to-transparent" />
    </span>
  )
}

/**
 * WHERE the list comes from, as one control: the project folder, and the
 * repository its `origin` resolves to.
 *
 * One group rather than two loose pills because they are one fact read in two
 * halves — you pick the folder, the repository follows from it — and because
 * the repository would otherwise be a caption under a heading, which is not
 * where anyone looks for "am I on the right project?". First on the bar for
 * the same reason: it is the choice every other control on the page is
 * relative to.
 *
 * Just `owner/repo` on the face: the host is the same for every row on the page
 * and only ever pushed the identifying half out of view. It survives in the
 * tooltip, which is where a self-hosted user looks.
 */
function RepoBar({
  folders,
  folderId,
  onPickFolder,
  remote,
}: {
  folders: readonly FolderSelectOption[]
  folderId: number | null
  onPickFolder: (id: number) => void
  /** `null` until the folder resolves, or for a folder with no forge remote —
   *  the picker still has to be usable, so only the right half goes away. */
  remote: ForgeRemote | null
}) {
  const t = useTranslations("Forge")

  return (
    // One chip of the same family as the filter pills below (`ws-msg-chip` is
    // what keeps it in step once a workspace background is on).
    <div className="ws-msg-chip flex h-8 min-w-0 items-center gap-1 rounded-full bg-muted/50 px-0.5 py-0.5">
      <FolderSelect
        folders={folders}
        value={folderId}
        onChange={onPickFolder}
        placeholder={t("pickFolder")}
        title={t("pickFolder")}
        variant="ghost"
      />
      {remote != null ? (
        <>
          <Separator orientation="vertical" className="!h-4" />
          <button
            type="button"
            className="inline-flex h-7 min-w-0 items-center gap-1 rounded-full px-2 font-mono text-[0.8125rem] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title={`${t("openRepo")} · ${remote.server_host}/${remote.owner_repo}`}
            onClick={() => {
              void openUrl(repoWebUrl(remote)).catch(() => {
                // An unopenable remote must not take the page down with it.
              })
            }}
          >
            <span className="min-w-0 truncate">{remote.owner_repo}</span>
            <ExternalLink className="size-3 shrink-0 opacity-60" aria-hidden />
          </button>
        </>
      ) : null}
    </div>
  )
}

/**
 * Multi-select over the repository's own labels. Both forges AND the selection
 * together, which is what the checkbox-style list implies — and why the count
 * badge matters: two labels usually means far fewer rows, not more.
 */
function LabelFilter({
  options,
  truncated,
  selected,
  onChange,
}: {
  options: ForgeLabel[]
  truncated: boolean
  /** Names, not labels: the filter travels to the forge as names, and the
   *  colour is only ever decoration on the way out. */
  selected: string[]
  onChange: (next: string[]) => void
}) {
  const t = useTranslations("Forge")
  const [open, setOpen] = useState(false)
  const active = selected.length > 0

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          aria-pressed={active}
          className={cn(
            PILL,
            active &&
              "bg-primary/10 text-primary hover:bg-primary/15 dark:hover:bg-primary/15"
          )}
        >
          <Funnel
            className={cn("size-3.5", !active && "text-muted-foreground")}
            aria-hidden
          />
          {t("labelsFilter")}
          {active ? (
            <span className="rounded-full bg-primary/15 px-1.5 py-0.5 text-[0.625rem] font-medium leading-none tabular-nums">
              {selected.length}
            </span>
          ) : null}
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 overflow-hidden p-0">
        <Command className="rounded-2xl">
          <CommandInput placeholder={t("searchLabels")} />
          <CommandList>
            <CommandEmpty>{t("noLabels")}</CommandEmpty>
            <CommandGroup>
              {options.map(({ name, color }) => {
                const checked = selected.includes(name)
                return (
                  <CommandItem
                    key={name}
                    value={name}
                    // Stays open: picking labels is a multi-step action, and
                    // closing after each one would make two labels two trips.
                    onSelect={() =>
                      onChange(
                        checked
                          ? selected.filter((n) => n !== name)
                          : [...selected, name]
                      )
                    }
                  >
                    {/* A dot, not the full chip: this list is scanned for a
                        NAME, and twenty coloured pills stacked vertically
                        fight the row's own selected state. The forge's raw
                        colour is safe as a fill — it needs no contrast against
                        text sitting on it. An uncoloured label keeps a hollow
                        ring, so the column of dots stays a column. */}
                    <span
                      aria-hidden
                      className="size-2.5 shrink-0 rounded-full border border-border"
                      style={
                        color != null ? { backgroundColor: color } : undefined
                      }
                    />
                    <span className="min-w-0 flex-1 truncate">{name}</span>
                    {checked ? <Check className="size-4 shrink-0" /> : null}
                  </CommandItem>
                )
              })}
            </CommandGroup>
            {active ? (
              <>
                <CommandSeparator />
                <CommandGroup>
                  <CommandItem
                    value="__clear_labels__"
                    onSelect={() => {
                      setOpen(false)
                      onChange([])
                    }}
                  >
                    <X className="size-4" />
                    <span>{t("clearLabels")}</span>
                  </CommandItem>
                </CommandGroup>
              </>
            ) : null}
          </CommandList>
          {truncated ? (
            // A list that silently stopped at one page would read as the whole
            // vocabulary, and the missing label as one that does not exist.
            <p className="border-t border-border/60 px-3 py-2 text-[0.6875rem] text-muted-foreground">
              {t("labelsTruncated")}
            </p>
          ) : null}
        </Command>
      </PopoverContent>
    </Popover>
  )
}

/**
 * One centred cluster below the list: the page strip and the page size, split
 * by a rule — two halves of "which slice of the list is on screen". Below the
 * list rather than in the toolbar, because pagination describes what you are
 * reading rather than what you are asking for.
 *
 * Page NUMBERS need a total. When the forge declined to give one (see
 * `ForgeIssueList.total_count`) this degrades to previous/next plus the
 * current page and a gap marker — an honest "there is more", instead of page
 * numbers invented from a count nobody sent.
 *
 * The total they are built from is `reachable_count`, NOT `total_count`:
 * GitHub search matches without limit but PAGES only the first thousand, so a
 * 24 000-hit query would otherwise draw a button to page 1 200 that answers
 * 422. The strip stops where the forge stops, and the gap between the two
 * numbers is said in words above it rather than left to be discovered.
 */
function ListFooter({
  list,
  pageSize,
  busy,
  compact,
  onPage,
  onPageSize,
}: {
  list: ForgeIssueList
  pageSize: ForgePageSize
  busy: boolean
  /** Phone width: fewer page slots, so the cluster still fits on one line. */
  compact: boolean
  onPage: (page: number) => void
  onPageSize: (size: ForgePageSize) => void
}) {
  const t = useTranslations("Forge")
  const totalPages = pageCount(
    list.reachable_count ?? list.total_count,
    list.per_page
  )

  return (
    <div
      data-testid="forge-footer"
      className="flex shrink-0 flex-col items-center gap-1 border-t border-border/60 px-4 py-2"
    >
      {list.incomplete ? (
        // Above the controls rather than beside them: a short page from a
        // timed-out search otherwise reads as "that is all there is", and this
        // is the one thing here that is not a control.
        <span className="text-xs text-amber-600 dark:text-amber-500">
          {t("incompleteResults")}
        </span>
      ) : null}
      {list.reachable_count != null ? (
        // Why the strip ends early. Without this the pager just stops, which
        // reads as a bug rather than as the forge's own ceiling — and the
        // remedy (narrow the filters) is not guessable from a missing button.
        <span className="text-xs text-muted-foreground">
          {t("reachableCap", { count: list.reachable_count })}
        </span>
      ) : null}

      <div className="flex flex-wrap items-center justify-center gap-2">
        <Pagination aria-label={t("pagination")} className="mx-0 w-auto">
          <PaginationContent>
            <PaginationItem>
              <PaginationPrevious
                aria-label={t("previousPage")}
                title={t("previousPage")}
                disabled={busy || list.page <= 1}
                onClick={() => onPage(list.page - 1)}
              />
            </PaginationItem>
            {totalPages != null ? (
              pageSlots(
                list.page,
                totalPages,
                compact ? PAGE_SLOTS_COMPACT : PAGE_SLOTS_DESKTOP
              ).map((slot, i) =>
                slot === "ellipsis" ? (
                  <PaginationItem key={`gap-${i}`}>
                    <PaginationEllipsis />
                  </PaginationItem>
                ) : (
                  <PaginationItem key={slot}>
                    <PaginationLink
                      isActive={slot === list.page}
                      disabled={busy}
                      aria-label={t("goToPage", { page: slot })}
                      onClick={() => onPage(slot)}
                    >
                      {slot}
                    </PaginationLink>
                  </PaginationItem>
                )
              )
            ) : (
              // No total: say which page you are on and that more may follow.
              <>
                <PaginationItem>
                  <PaginationLink isActive disabled>
                    {list.page}
                  </PaginationLink>
                </PaginationItem>
                {list.has_next ? (
                  <PaginationItem>
                    <PaginationEllipsis />
                  </PaginationItem>
                ) : null}
              </>
            )}
            <PaginationItem>
              <PaginationNext
                aria-label={t("nextPage")}
                title={t("nextPage")}
                disabled={busy || !list.has_next}
                onClick={() => onPage(list.page + 1)}
              />
            </PaginationItem>
          </PaginationContent>
        </Pagination>

        <Separator orientation="vertical" className="!h-5" />

        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span className="hidden sm:inline">{t("pageSize")}</span>
          <Select
            value={String(pageSize)}
            onValueChange={(v) =>
              onPageSize(Number.parseInt(v, 10) as ForgePageSize)
            }
          >
            <SelectTrigger
              size="sm"
              aria-label={t("pageSize")}
              className="h-7 w-auto gap-1 rounded-full px-2.5 text-xs"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {FORGE_PAGE_SIZES.map((size) => (
                <SelectItem key={size} value={String(size)}>
                  {size}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
    </div>
  )
}

function EmptyHint({
  text,
  action,
}: {
  text: string
  /** Optional way OUT of the state being described (e.g. "add an account"). */
  action?: ReactNode
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center text-sm text-muted-foreground">
      <GitPullRequestArrow className="h-6 w-6 opacity-40" />
      <span className="max-w-md text-balance">{text}</span>
      {action != null ? <div className="mt-1">{action}</div> : null}
    </div>
  )
}

/**
 * Placeholder rows in the real rows' own rhythm — glyph, two lines of text,
 * an action — rather than a stack of plain bars. Same divider, same padding,
 * same heights, so the arriving page settles into the shape already on screen
 * instead of shoving it.
 */
function ListSkeleton({ rows }: { rows: number }) {
  return (
    <div
      aria-hidden
      className="flex flex-col divide-y divide-border/40"
      data-testid="forge-list-skeleton"
    >
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-start gap-3 px-4 py-2.5">
          <Skeleton className="mt-0.5 size-3.5 shrink-0 rounded-full" />
          <div className="min-w-0 flex-1 space-y-1.5">
            {/* Varied widths: a column of identical bars reads as a table. */}
            <Skeleton
              className="h-4"
              style={{ width: `${45 + ((i * 13) % 40)}%` }}
            />
            <Skeleton className="h-3 w-28" />
          </div>
          <Skeleton className="h-7 w-16 shrink-0 self-center rounded-full" />
        </div>
      ))}
    </div>
  )
}

/**
 * The footer's height, held while the first page is still on its way.
 *
 * Letting the bar appear WITH the data would lift the whole list by its own
 * height at the exact moment the reader's eye lands on the first row — the
 * cheapest way to lose someone's place there is.
 */
function FooterPlaceholder() {
  return (
    <div
      data-testid="forge-footer"
      className="flex shrink-0 items-center justify-center border-t border-border/60 px-4 py-2"
    >
      <Skeleton className="h-8 w-56 rounded-full" />
    </div>
  )
}
