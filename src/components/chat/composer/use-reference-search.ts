"use client"

import { useCallback, useEffect, useLayoutEffect, useRef } from "react"
import { useLocale } from "next-intl"

import { useAcpAgents } from "@/hooks/use-acp-agents"
import { useAgentExperts } from "@/hooks/use-agent-experts"
import { useAgentSkills } from "@/hooks/use-agent-skills"
import { useBuiltInExperts } from "@/hooks/use-built-in-experts"
import { useFileTree, type FlatFileEntry } from "@/hooks/use-file-tree"
import { gitLog, listAllConversations } from "@/lib/api"
import type {
  AcpAgentInfo,
  AgentType,
  DbConversationSummary,
  ExpertListItem,
  GitLogEntry,
} from "@/lib/types"

import {
  agentToSuggestion,
  commitToSuggestion,
  expertToSuggestion,
  fileToSuggestion,
  sessionToSuggestion,
  skillToSuggestion,
} from "./suggestion/adapters"
import type {
  ReferenceSearch,
  SuggestionGroup,
  SuggestionItem,
} from "./suggestion/types"
import type { AgentSkillItem } from "@/lib/types"

// Commit-synchronous on the client (so the guard-critical refs are updated
// during commit, before any later macrotask/microtask can resolve a stale
// in-flight fetch), but a no-op-safe passive effect during the static-export
// prerender where `useLayoutEffect` would warn.
const useIsomorphicLayoutEffect =
  typeof window !== "undefined" ? useLayoutEffect : useEffect

/** Max rows surfaced per group (mirrors the textarea `@` menu's file cap). */
const MAX_PER_GROUP = 50
/** How many commits the git-log group pulls (client-filtered down from here). */
const GIT_LOG_LIMIT = 100
const EMPTY_COMMITS: Promise<GitLogEntry[]> = Promise.resolve([])

/** Display headings for each group; injected so the host can localize them. */
export interface ReferenceGroupLabels {
  file: string
  agent: string
  session: string
  commit: string
  skill: string
}

/**
 * English fallbacks, matching the suggestion popup's `emptyLabel`/`loadingLabel`
 * convention (the host passes localized strings at the integration layer).
 */
export const DEFAULT_GROUP_LABELS: ReferenceGroupLabels = {
  file: "Files",
  agent: "Agents",
  session: "Sessions",
  commit: "Commits",
  skill: "Skills",
}

/** Raw, already-loaded data the pure group builder turns into suggestions. */
export interface ReferenceSearchSources {
  files: FlatFileEntry[]
  /** Workspace root the `files` were loaded under; null disables the group. */
  workspaceRoot: string | null
  agents: AcpAgentInfo[]
  sessions: DbConversationSummary[]
  commits: GitLogEntry[]
  /** Repo identity for commit URIs; null disables the commit group. */
  repoKey: string | null
  skills: AgentSkillItem[]
  builtInExperts: ExpertListItem[]
  agentExperts: ExpertListItem[]
  locale: string
}

/** Case-insensitive substring match against an adapted item's searchable text. */
function suggestionMatches(item: SuggestionItem, lowerQuery: string): boolean {
  if (!lowerQuery) return true
  const ref = item.reference
  return (
    ref.label.toLowerCase().includes(lowerQuery) ||
    ref.id.toLowerCase().includes(lowerQuery) ||
    (item.keywords ?? "").toLowerCase().includes(lowerQuery) ||
    (item.detail ?? "").toLowerCase().includes(lowerQuery)
  )
}

/**
 * Pure: filter + adapt the raw sources into the fixed-order grouped suggestions
 * the `@` panel renders (files → agents → sessions → commits → skills). Each
 * group is independently capped at {@link MAX_PER_GROUP}; empty groups are kept
 * (the popup hides them) so the order is always stable. Extracted from the hook
 * so the matching/ordering/dedup logic is testable without React.
 */
export function buildReferenceGroups(
  query: string,
  sources: ReferenceSearchSources,
  labels: ReferenceGroupLabels = DEFAULT_GROUP_LABELS
): SuggestionGroup[] {
  const q = query.trim().toLowerCase()

  // Files: filter the (potentially large) list on its pre-lowered fields before
  // paying to adapt the survivors. `truncated` is a cheap boolean — set when a
  // match is found past the cap — so we never scan the whole list for a count.
  const fileItems: SuggestionItem[] = []
  let fileTruncated = false
  const root = sources.workspaceRoot
  if (root) {
    for (const entry of sources.files) {
      if (q && !entry.lowerName.includes(q) && !entry.lowerPath.includes(q)) {
        continue
      }
      if (fileItems.length >= MAX_PER_GROUP) {
        fileTruncated = true
        break
      }
      fileItems.push(fileToSuggestion(entry, root))
    }
  }

  const agentMatches = sources.agents
    .map(agentToSuggestion)
    .filter((item) => suggestionMatches(item, q))
  const agentItems = agentMatches.slice(0, MAX_PER_GROUP)

  const sessionMatches = sources.sessions
    .map(sessionToSuggestion)
    .filter((item) => suggestionMatches(item, q))
  const sessionItems = sessionMatches.slice(0, MAX_PER_GROUP)

  const commitItems: SuggestionItem[] = []
  let commitTruncated = false
  if (sources.repoKey) {
    const repoKey = sources.repoKey
    for (const entry of sources.commits) {
      const item = commitToSuggestion(entry, repoKey)
      if (!suggestionMatches(item, q)) continue
      if (commitItems.length >= MAX_PER_GROUP) {
        commitTruncated = true
        break
      }
      commitItems.push(item)
    }
  }

  // Skills + built-in experts + agent-linked experts share one group. An expert
  // can surface from more than one source, so dedupe by reference id (skill id),
  // keeping the first occurrence, before filtering.
  const skillItems: SuggestionItem[] = []
  let skillTruncated = false
  const seenSkillIds = new Set<string>()
  const skillCandidates: SuggestionItem[] = [
    ...sources.skills.map(skillToSuggestion),
    ...sources.builtInExperts.map((e) => expertToSuggestion(e, sources.locale)),
    ...sources.agentExperts.map((e) => expertToSuggestion(e, sources.locale)),
  ]
  for (const item of skillCandidates) {
    if (seenSkillIds.has(item.reference.id)) continue
    seenSkillIds.add(item.reference.id)
    if (!suggestionMatches(item, q)) continue
    if (skillItems.length >= MAX_PER_GROUP) {
      skillTruncated = true
      break
    }
    skillItems.push(item)
  }

  return [
    {
      kind: "file",
      label: labels.file,
      items: fileItems,
      truncated: fileTruncated,
    },
    {
      kind: "agent",
      label: labels.agent,
      items: agentItems,
      truncated: agentMatches.length > MAX_PER_GROUP,
    },
    {
      kind: "session",
      label: labels.session,
      items: sessionItems,
      truncated: sessionMatches.length > MAX_PER_GROUP,
    },
    {
      kind: "commit",
      label: labels.commit,
      items: commitItems,
      truncated: commitTruncated,
    },
    {
      kind: "skill",
      label: labels.skill,
      items: skillItems,
      truncated: skillTruncated,
    },
  ]
}

export interface UseReferenceSearchOptions {
  /**
   * Workspace root for the file + commit groups (and the commit `repoKey`).
   * When empty/null those two groups stay empty while agents/sessions/skills
   * still resolve, so a brand-new draft tab degrades gracefully (R8).
   */
  defaultPath?: string | null
  /** Active agent type, scoping the skill + expert lists. */
  agentType?: AgentType | null
  /**
   * Gates loading. When false the search resolves to empty groups and the file
   * tree is never fetched — let the host pre-warm only the active composer.
   */
  enabled?: boolean
  /** Localized group headings; English fallbacks when omitted. */
  labels?: ReferenceGroupLabels
}

/**
 * Compose the live data sources (file tree, ACP agents, conversations, git log,
 * skills, experts) into a single {@link ReferenceSearch} for the composer's `@`
 * panel.
 *
 * Referential stability is the contract: the suggestion popup re-runs its fetch
 * whenever the `search` identity changes (`suggestion-popup.tsx`), so the
 * returned function is an empty-dependency `useCallback` that reads every source
 * from a ref. A background refresh of any source (e.g. the agent list reloading
 * on window focus) updates the refs but leaves `search` identity untouched — the
 * open panel keeps its results and the user's selection (R7).
 *
 * Files/agents/skills/experts are hook-loaded (and pre-warmed via `enabled`).
 * Sessions and the git log are fetched lazily on the first `@`, key-cached in a
 * ref, and awaited by `search` so the first open is populated without an extra
 * keystroke; window focus busts those caches so they stay fresh.
 */
export function useReferenceSearch({
  defaultPath,
  agentType = null,
  enabled = true,
  labels,
}: UseReferenceSearchOptions): ReferenceSearch {
  const path = defaultPath || null
  const locale = useLocale()

  const { allFiles, loaded } = useFileTree({
    folderPath: path ?? undefined,
    enabled,
  })
  const { agents } = useAcpAgents()
  const skills = useAgentSkills(agentType, path)
  const builtInExperts = useBuiltInExperts()
  const agentExperts = useAgentExperts(agentType)

  // Mirror every changing source into a ref so `search` can stay identity-stable
  // (see the doc comment). Initialized from the first render so the refs are
  // sane even before the sync effect below runs.
  const filesRef = useRef<{ root: string | null; files: FlatFileEntry[] }>({
    root: null,
    files: [],
  })
  const agentsRef = useRef(agents)
  const skillsRef = useRef(skills)
  const builtInExpertsRef = useRef(builtInExperts)
  const agentExpertsRef = useRef(agentExperts)
  const localeRef = useRef(locale)
  const pathRef = useRef(path)
  const enabledRef = useRef(enabled)
  const labelsRef = useRef(labels)

  // `pathRef` and `enabledRef` gate the post-await freshness check in `search`,
  // so they must reflect the *committed* folder/enabled state synchronously at
  // commit — a passive effect can lag behind a stale in-flight fetch that
  // resolves in the post-commit / pre-effect window, leaking the old folder's
  // commits into the new panel. A layout effect (not a render-phase write) keeps
  // them commit-accurate without updating from an uncommitted transition render.
  useIsomorphicLayoutEffect(() => {
    pathRef.current = path
    enabledRef.current = enabled
  }, [path, enabled])

  useEffect(() => {
    // Only expose files once the tree has loaded for the *current* path, so the
    // search never joins the current workspace root onto a previous folder's
    // relative paths during a folder switch.
    filesRef.current =
      loaded && path
        ? { root: path, files: allFiles }
        : { root: null, files: [] }
    agentsRef.current = agents
    skillsRef.current = skills
    builtInExpertsRef.current = builtInExperts
    agentExpertsRef.current = agentExperts
    localeRef.current = locale
    labelsRef.current = labels
  }, [
    allFiles,
    loaded,
    path,
    agents,
    skills,
    builtInExperts,
    agentExperts,
    locale,
    labels,
  ])

  // Lazily-fetched network sources, key-cached so repeat searches reuse the
  // in-flight/resolved promise while a folder switch refetches.
  const sessionsRef = useRef<{
    key: string
    promise: Promise<DbConversationSummary[]>
  } | null>(null)
  const commitsRef = useRef<{
    key: string
    promise: Promise<GitLogEntry[]>
  } | null>(null)

  // Bust the lazy caches when the window regains focus so a session created in
  // another window (or new commits) show up on the next `@` — matching the
  // focus-refresh idiom of the other data hooks, without per-keystroke fetches.
  useEffect(() => {
    const onFocus = () => {
      sessionsRef.current = null
      commitsRef.current = null
    }
    window.addEventListener("focus", onFocus)
    return () => window.removeEventListener("focus", onFocus)
  }, [])

  return useCallback<ReferenceSearch>(async (query, signal) => {
    if (!enabledRef.current) return []

    const path = pathRef.current

    // Lazy session fetch. On rejection the cache entry is cleared (not cached as
    // an empty result) so the next `@` retries instead of wedging on `[]`.
    const sessionsKey = "all"
    let sessionsEntry = sessionsRef.current
    if (sessionsEntry?.key !== sessionsKey) {
      const created: NonNullable<typeof sessionsRef.current> = {
        key: sessionsKey,
        promise: listAllConversations().catch(() => {
          if (sessionsRef.current === created) sessionsRef.current = null
          return [] as DbConversationSummary[]
        }),
      }
      sessionsRef.current = created
      sessionsEntry = created
    }

    // Lazy git-log fetch, keyed by path with the same retry-on-rejection policy.
    let commitsPromise = EMPTY_COMMITS
    if (path) {
      let commitsEntry = commitsRef.current
      if (commitsEntry?.key !== path) {
        const created: NonNullable<typeof commitsRef.current> = {
          key: path,
          promise: gitLog(path, GIT_LOG_LIMIT)
            .then((result) => result.entries)
            .catch(() => {
              if (commitsRef.current === created) commitsRef.current = null
              return [] as GitLogEntry[]
            }),
        }
        commitsRef.current = created
        commitsEntry = created
      }
      commitsPromise = commitsEntry.promise
    } else {
      commitsRef.current = null
    }

    const [sessions, commits] = await Promise.all([
      sessionsEntry.promise,
      commitsPromise,
    ])
    // Discard this result if it can no longer be trusted for the live panel: a
    // newer query aborted us, the composer was disabled, or the workspace folder
    // changed while the network fetch was in flight (the popup only aborts on a
    // query change, so a folder switch would otherwise leak the old repo's
    // commits — built against `path` — into the new folder's panel). The next
    // keystroke re-runs the search against the current folder.
    if (signal?.aborted || !enabledRef.current || pathRef.current !== path) {
      return []
    }

    const fileState = filesRef.current
    return buildReferenceGroups(
      query,
      {
        files: fileState.files,
        workspaceRoot: fileState.root,
        agents: agentsRef.current,
        sessions,
        commits,
        repoKey: path,
        skills: skillsRef.current,
        builtInExperts: builtInExpertsRef.current,
        agentExperts: agentExpertsRef.current,
        locale: localeRef.current,
      },
      labelsRef.current ?? DEFAULT_GROUP_LABELS
    )
  }, [])
}
