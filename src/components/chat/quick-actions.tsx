"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { useLocale, useTranslations } from "next-intl"
import { toast } from "sonner"
import { Lock, type LucideIcon } from "lucide-react"

import { cn } from "@/lib/utils"
import { openSettingsWindow, type SettingsSection } from "@/lib/api"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { useBuiltInExperts } from "@/hooks/use-built-in-experts"
import { useEnabledSkillIds } from "@/hooks/use-enabled-skill-ids"
import { useWelcomeQuickActions } from "@/hooks/use-appearance"
import { getExpertIcon, pickLocalized } from "@/lib/expert-presentation"
import {
  OFFICE_ACTIONS,
  OFFICE_FEATURED_ACCENTS,
  type OfficeAction,
} from "@/lib/office-actions"
import {
  RESEARCH_ACTIONS,
  RESEARCH_FEATURED_ACCENTS,
  type ResearchAction,
} from "@/lib/research-actions"
import {
  loadQuickActionsTab,
  saveQuickActionsTab,
  type QuickActionsTab,
} from "@/lib/quick-actions-tab-storage"
import type { ComposerInjectContent } from "@/components/chat/message-input"
import { AGENT_LABELS, type AgentType, type ExpertListItem } from "@/lib/types"

// Three primary office categories — prominent fixed cards (keep their color).
const OFFICE_FIXED: (OfficeAction & { accent: string })[] =
  OFFICE_ACTIONS.filter((action) => action.id in OFFICE_FEATURED_ACCENTS).map(
    (action) => ({ ...action, accent: OFFICE_FEATURED_ACCENTS[action.id] })
  )

// Remaining office skills — de-colored bars in the scrolling row (icons kept).
const OFFICE_SCROLL: OfficeAction[] = OFFICE_ACTIONS.filter(
  (action) => !(action.id in OFFICE_FEATURED_ACCENTS)
)

// Three primary research skills — prominent fixed cards (keep their color).
const RESEARCH_FIXED: (ResearchAction & { accent: string })[] =
  RESEARCH_ACTIONS.filter(
    (action) => action.id in RESEARCH_FEATURED_ACCENTS
  ).map((action) => ({
    ...action,
    accent: RESEARCH_FEATURED_ACCENTS[action.id],
  }))

// Remaining research skills — de-colored bars in the scrolling row.
const RESEARCH_SCROLL: ResearchAction[] = RESEARCH_ACTIONS.filter(
  (action) => !(action.id in RESEARCH_FEATURED_ACCENTS)
)

// Three featured coding experts get prominent fixed cards (color + curated
// short description); the rest fill the scrolling row. ids match the bundled
// superpowers skills.
const CODING_FEATURED: { id: string; accent: string; descKey: string }[] = [
  { id: "brainstorming", accent: "amber", descKey: "coding.brainstormingDesc" },
  {
    id: "systematic-debugging",
    accent: "pink",
    descKey: "coding.debuggingDesc",
  },
  {
    id: "writing-skills",
    accent: "purple",
    descKey: "coding.writingSkillsDesc",
  },
]
const CODING_FEATURED_IDS = new Set(CODING_FEATURED.map((f) => f.id))

// Static accent class fragments — full literals so Tailwind's JIT keeps them
// (never compose color classes from template strings, those get purged). Only
// the prominent fixed cards are colored; scrolling bars are neutral.
const ACCENTS: Record<string, { icon: string; surface: string }> = {
  green: {
    icon: "text-green-600 dark:text-green-400",
    surface:
      "border-green-500/20 hover:border-green-500/40 hover:bg-green-500/5",
  },
  blue: {
    icon: "text-blue-600 dark:text-blue-400",
    surface: "border-blue-500/20 hover:border-blue-500/40 hover:bg-blue-500/5",
  },
  orange: {
    icon: "text-orange-600 dark:text-orange-400",
    surface:
      "border-orange-500/20 hover:border-orange-500/40 hover:bg-orange-500/5",
  },
  amber: {
    icon: "text-amber-600 dark:text-amber-400",
    surface:
      "border-amber-500/20 hover:border-amber-500/40 hover:bg-amber-500/5",
  },
  pink: {
    icon: "text-pink-600 dark:text-pink-400",
    surface: "border-pink-500/20 hover:border-pink-500/40 hover:bg-pink-500/5",
  },
  purple: {
    icon: "text-purple-600 dark:text-purple-400",
    surface:
      "border-purple-500/20 hover:border-purple-500/40 hover:bg-purple-500/5",
  },
  violet: {
    icon: "text-violet-600 dark:text-violet-400",
    surface:
      "border-violet-500/20 hover:border-violet-500/40 hover:bg-violet-500/5",
  },
}

/** A prominent fixed category card (icon + title + one-line description). */
function BigCard({
  icon: Icon,
  accent,
  title,
  description,
  onClick,
  locked,
  lockHint,
}: {
  icon: LucideIcon
  accent: string
  title: string
  description: string
  onClick: () => void
  /** Skill not enabled for the current agent — shows a lock badge; the card
   *  keeps its normal look and stays clickable (the click surfaces a hint). */
  locked?: boolean
  lockHint?: string
}) {
  const a = ACCENTS[accent] ?? ACCENTS.green
  return (
    <button
      type="button"
      onClick={onClick}
      title={locked ? lockHint : undefined}
      className={cn(
        "group relative flex flex-col items-start gap-1.5 rounded-lg border bg-card/50 px-3 py-2.5 text-left transition-colors",
        a.surface
      )}
    >
      {locked && (
        <Lock
          aria-hidden
          className="absolute right-2 top-2 h-3.5 w-3.5 text-muted-foreground/70"
        />
      )}
      <Icon aria-hidden className={cn("h-4 w-4 transition-colors", a.icon)} />
      <div className="text-sm font-medium text-foreground">{title}</div>
      <div className="line-clamp-1 text-xs text-muted-foreground">
        {description}
      </div>
    </button>
  )
}

/** A neutral (de-colored) skill bar in the scrolling row. */
function SkillBar({
  icon: Icon,
  label,
  title,
  onClick,
  clone,
  locked,
  lockHint,
}: {
  icon: LucideIcon
  label: string
  title?: string
  onClick: () => void
  /** A marquee duplicate: hidden from a11y + keyboard, removed under reduced motion. */
  clone?: boolean
  /** Skill not enabled for the current agent — appends a small lock glyph; the
   *  bar keeps its look and stays clickable (the click surfaces a hint). */
  locked?: boolean
  lockHint?: string
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={locked ? lockHint : title}
      aria-hidden={clone || undefined}
      tabIndex={clone ? -1 : undefined}
      data-qa-clone={clone ? "" : undefined}
      className="group mr-2 flex shrink-0 items-center gap-2 rounded-lg border border-border bg-card/50 px-3 py-2 transition-colors hover:border-foreground/20 hover:bg-accent/40"
    >
      <Icon
        aria-hidden
        className="h-4 w-4 shrink-0 text-muted-foreground transition-colors group-hover:text-foreground"
      />
      <span className="whitespace-nowrap text-xs font-medium text-foreground/90">
        {label}
      </span>
      {locked && (
        <Lock
          aria-hidden
          className="h-3 w-3 shrink-0 text-muted-foreground/60"
        />
      )}
    </button>
  )
}

/**
 * Single-row seamless marquee. Renders the items twice (real + an aria-hidden
 * clone) and a requestAnimationFrame loop translates the track left at a
 * constant speed, wrapping by exactly one copy width (`scrollWidth / 2`, exact
 * because each bar carries its own `mr-2` instead of a flex gap) so the loop
 * has no seam.
 *
 * Driving the transform from JS — rather than a CSS animation — is deliberate:
 * a composited CSS transform animation snaps backwards when paused on hover
 * (WebKit reverts to the lagging main-thread clock). Setting the transform
 * ourselves each frame means "pause" just stops updating, freezing it in place.
 * Pauses on pointer hover / keyboard focus; bails entirely under reduced motion
 * (CSS then makes the viewport a normal horizontal scroller).
 */
function Marquee({
  itemCount,
  children,
}: {
  itemCount: number
  children: (clone: boolean) => ReactNode
}) {
  const trackRef = useRef<HTMLDivElement | null>(null)
  const pausedRef = useRef(false)

  useEffect(() => {
    const track = trackRef.current
    if (!track || typeof window === "undefined") return
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return

    const SPEED = 35 // px per second — constant feel regardless of item count
    let offset = 0
    let half = track.scrollWidth / 2
    let last = 0
    let raf = 0

    const ro = new ResizeObserver(() => {
      half = track.scrollWidth / 2
    })
    ro.observe(track)

    const step = (now: number) => {
      raf = requestAnimationFrame(step)
      // Clamp dt so returning from a backgrounded tab doesn't lurch forward.
      const dt = last === 0 ? 0 : Math.min(0.05, (now - last) / 1000)
      last = now
      if (pausedRef.current || half <= 0) return
      offset -= SPEED * dt
      if (offset <= -half) offset += half
      track.style.transform = `translate3d(${offset}px, 0, 0)`
    }
    raf = requestAnimationFrame(step)

    return () => {
      cancelAnimationFrame(raf)
      ro.disconnect()
    }
  }, [itemCount])

  if (itemCount === 0) return null

  const pause = () => {
    pausedRef.current = true
  }
  const resume = () => {
    pausedRef.current = false
  }

  return (
    <div
      className="qa-marquee-viewport overflow-hidden pb-1"
      onPointerEnter={pause}
      onPointerLeave={resume}
      onFocus={pause}
      onBlur={resume}
    >
      <div ref={trackRef} className="qa-marquee-track flex w-max">
        {children(false)}
        {children(true)}
      </div>
    </div>
  )
}

interface QuickActionsProps {
  /** Emits the resolved (localized) injection payload for the picked action. */
  onSelect: (payload: ComposerInjectContent) => void
  /** The agent the new conversation will use. Drives per-agent skill-enabled
   *  detection: a card whose skill isn't linked to this agent is locked, and
   *  clicking it shows a hint instead of injecting an unusable badge. */
  agentType: AgentType | null
}

export function QuickActions({ onSelect, agentType }: QuickActionsProps) {
  const t = useTranslations("Folder.chat.welcomePanel.quickActions")
  const locale = useLocale()
  const experts = useBuiltInExperts()
  const { enabledIds, ready, supported } = useEnabledSkillIds(agentType)
  const { showWelcomeQuickActions } = useWelcomeQuickActions()
  const lockHint = t("notEnabled.hint")

  // A skill card is locked when we know which agent will run (welcome mode
  // always does) and — after the status snapshot has loaded — that skill is not
  // linked to it. Before `ready` we optimistically treat everything as usable
  // to avoid a flash of all-locked cards on first paint.
  const isLocked = useCallback(
    (id: string) => !!agentType && ready && !enabledIds.has(id),
    [agentType, ready, enabledIds]
  )

  // Clicking a locked card: warn (with the agent's name) and offer a one-click
  // jump to the settings page that manages this skill family — coding cards
  // open Experts, office cards open Office Tools — rather than injecting a
  // badge the agent can't act on.
  const notifyNotEnabled = useCallback(
    (skillLabel: string, section: SettingsSection) => {
      const agentLabel = agentType ? AGENT_LABELS[agentType] : ""
      toast.warning(
        t("notEnabled.title", { skill: skillLabel, agent: agentLabel }),
        {
          description: t("notEnabled.description"),
          action: {
            label: t("notEnabled.action"),
            onClick: () => {
              void openSettingsWindow(section).catch((err) =>
                console.error("[QuickActions] failed to open settings:", err)
              )
            },
          },
        }
      )
    },
    [agentType, t]
  )

  // Restore the last-picked tab; persist on change. The lazy initializer reads
  // localStorage on each mount (QuickActions only renders client-side in
  // welcome mode, so there is no SSR hydration mismatch), so reopening a new
  // conversation shows the previous choice.
  const [tab, setTab] = useState<QuickActionsTab>(() => loadQuickActionsTab())
  const handleTabChange = useCallback((value: string) => {
    const next: QuickActionsTab =
      value === "office"
        ? "office"
        : value === "research"
          ? "research"
          : "coding"
    setTab(next)
    saveQuickActionsTab(next)
  }, [])

  const handleOffice = useCallback(
    (action: OfficeAction) => {
      const label = t(action.id as Parameters<typeof t>[0])
      if (isLocked(action.skillId)) {
        notifyNotEnabled(label, "office-tools")
        return
      }
      onSelect({
        text: t(action.promptKey as Parameters<typeof t>[0]),
        skill: { id: action.skillId, label },
      })
    },
    [onSelect, t, isLocked, notifyNotEnabled]
  )

  const handleResearch = useCallback(
    (action: ResearchAction) => {
      const label = t(action.id as Parameters<typeof t>[0])
      if (isLocked(action.skillId)) {
        notifyNotEnabled(label, "science")
        return
      }
      onSelect({
        text: t(action.promptKey as Parameters<typeof t>[0]),
        skill: { id: action.skillId, label },
      })
    },
    [onSelect, t, isLocked, notifyNotEnabled]
  )

  const handleExpert = useCallback(
    (item: ExpertListItem) => {
      const label =
        pickLocalized(item.metadata.display_name, locale) || item.metadata.id
      if (isLocked(item.metadata.id)) {
        notifyNotEnabled(label, "experts")
        return
      }
      // Experts are open-ended coding skills: inject just the leading
      // invocation badge (`$id` on Codex, `/id` elsewhere) and let the user
      // describe the task (no canned template like office docs).
      onSelect({ text: "", skill: { id: item.metadata.id, label } })
    },
    [onSelect, locale, isLocked, notifyNotEnabled]
  )

  const { codingFeatured, codingRest } = useMemo(() => {
    const byId = new Map(experts.map((e) => [e.metadata.id, e]))
    const featured = CODING_FEATURED.map((f) => {
      const item = byId.get(f.id)
      return item ? { ...f, item } : null
    }).filter(
      (
        v
      ): v is (typeof CODING_FEATURED)[number] & {
        item: ExpertListItem
      } => v !== null
    )
    const rest = experts
      .filter((e) => !CODING_FEATURED_IDS.has(e.metadata.id))
      .sort(
        (a, b) =>
          (a.metadata.sort_order ?? 0) - (b.metadata.sort_order ?? 0) ||
          a.metadata.id.localeCompare(b.metadata.id)
      )
    return { codingFeatured: featured, codingRest: rest }
  }, [experts])

  // Hidden when the user has turned off the welcome-page mode selection area
  // (Settings › Appearance), or when a custom-dir pi can't have skills managed
  // by codeg's default-dir store — in the latter case we hide the shortcut
  // cards rather than show ones that lock with a Settings path the
  // Experts/Office matrices also hide for this agent.
  if (!supported || !showWelcomeQuickActions) return null

  return (
    <Tabs value={tab} onValueChange={handleTabChange}>
      {/* Mirror the AgentSelector pill (agent-selector.tsx): a translucent
          `bg-muted/50` surface with the selected trigger a raised `bg-background`
          pill (shadow-sm + a subtle `ring-border/50` edge), so the active tab
          reads clearly against the bar instead of blending into it — a `bg-card/50`
          bar sat too close to the active `bg-background` fill to tell them apart.
          tailwind-merge drops the list's default `bg-muted`; with a workspace
          background image the bar reveals it at 50%, off (no image) it's a muted
          surface over the page. */}
      <TabsList className="mx-auto border bg-muted/50">
        <TabsTrigger
          value="coding"
          // Selected pill mirrors the AgentSelector droplet: bg + shadow + a
          // subtle ring edge. The `data-[state=active]:focus-visible:*` pair
          // re-asserts the 3px accent focus ring at higher specificity — without
          // it the 1px selected ring (emitted after the base `focus-visible`
          // rule) would swallow the keyboard focus indicator on the active tab.
          className="data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm data-[state=active]:ring-1 data-[state=active]:ring-border/50 data-[state=active]:focus-visible:ring-[3px] data-[state=active]:focus-visible:ring-ring/50"
        >
          {t("tabs.coding")}
        </TabsTrigger>
        <TabsTrigger
          value="office"
          // Selected pill mirrors the AgentSelector droplet: bg + shadow + a
          // subtle ring edge. The `data-[state=active]:focus-visible:*` pair
          // re-asserts the 3px accent focus ring at higher specificity — without
          // it the 1px selected ring (emitted after the base `focus-visible`
          // rule) would swallow the keyboard focus indicator on the active tab.
          className="data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm data-[state=active]:ring-1 data-[state=active]:ring-border/50 data-[state=active]:focus-visible:ring-[3px] data-[state=active]:focus-visible:ring-ring/50"
        >
          {t("tabs.office")}
        </TabsTrigger>
        <TabsTrigger
          value="research"
          // Selected pill mirrors the AgentSelector droplet: bg + shadow + a
          // subtle ring edge. The `data-[state=active]:focus-visible:*` pair
          // re-asserts the 3px accent focus ring at higher specificity — without
          // it the 1px selected ring (emitted after the base `focus-visible`
          // rule) would swallow the keyboard focus indicator on the active tab.
          className="data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm data-[state=active]:ring-1 data-[state=active]:ring-border/50 data-[state=active]:focus-visible:ring-[3px] data-[state=active]:focus-visible:ring-ring/50"
        >
          {t("tabs.research")}
        </TabsTrigger>
      </TabsList>

      <TabsContent value="coding" className="flex flex-col gap-2">
        {codingFeatured.length > 0 && (
          <div className="grid grid-cols-3 gap-2">
            {codingFeatured.map((f) => (
              <BigCard
                key={f.id}
                icon={getExpertIcon(f.item.metadata.icon)}
                accent={f.accent}
                title={
                  pickLocalized(f.item.metadata.display_name, locale) || f.id
                }
                description={t(f.descKey as Parameters<typeof t>[0])}
                onClick={() => handleExpert(f.item)}
                locked={isLocked(f.item.metadata.id)}
                lockHint={lockHint}
              />
            ))}
          </div>
        )}
        <Marquee itemCount={codingRest.length}>
          {(clone) =>
            codingRest.map((item) => {
              const label =
                pickLocalized(item.metadata.display_name, locale) ||
                item.metadata.id
              return (
                <SkillBar
                  key={`${item.metadata.id}-${clone ? "c" : "r"}`}
                  clone={clone}
                  icon={getExpertIcon(item.metadata.icon)}
                  label={label}
                  title={pickLocalized(item.metadata.description, locale)}
                  onClick={() => handleExpert(item)}
                  locked={isLocked(item.metadata.id)}
                  lockHint={lockHint}
                />
              )
            })
          }
        </Marquee>
      </TabsContent>

      <TabsContent value="office" className="flex flex-col gap-2">
        <div className="grid grid-cols-3 gap-2">
          {OFFICE_FIXED.map((action) => (
            <BigCard
              key={action.id}
              icon={action.icon}
              accent={action.accent}
              title={t(action.id as Parameters<typeof t>[0])}
              description={t(`${action.id}Desc` as Parameters<typeof t>[0])}
              onClick={() => handleOffice(action)}
              locked={isLocked(action.skillId)}
              lockHint={lockHint}
            />
          ))}
        </div>
        <Marquee itemCount={OFFICE_SCROLL.length}>
          {(clone) =>
            OFFICE_SCROLL.map((action) => (
              <SkillBar
                key={`${action.id}-${clone ? "c" : "r"}`}
                clone={clone}
                icon={action.icon}
                label={t(action.id as Parameters<typeof t>[0])}
                title={t(`${action.id}Desc` as Parameters<typeof t>[0])}
                onClick={() => handleOffice(action)}
                locked={isLocked(action.skillId)}
                lockHint={lockHint}
              />
            ))
          }
        </Marquee>
      </TabsContent>

      <TabsContent value="research" className="flex flex-col gap-2">
        <div className="grid grid-cols-3 gap-2">
          {RESEARCH_FIXED.map((action) => (
            <BigCard
              key={action.id}
              icon={action.icon}
              accent={action.accent}
              title={t(action.id as Parameters<typeof t>[0])}
              description={t(`${action.id}Desc` as Parameters<typeof t>[0])}
              onClick={() => handleResearch(action)}
              locked={isLocked(action.skillId)}
              lockHint={lockHint}
            />
          ))}
        </div>
        <Marquee itemCount={RESEARCH_SCROLL.length}>
          {(clone) =>
            RESEARCH_SCROLL.map((action) => (
              <SkillBar
                key={`${action.id}-${clone ? "c" : "r"}`}
                clone={clone}
                icon={action.icon}
                label={t(action.id as Parameters<typeof t>[0])}
                title={t(`${action.id}Desc` as Parameters<typeof t>[0])}
                onClick={() => handleResearch(action)}
                locked={isLocked(action.skillId)}
                lockHint={lockHint}
              />
            ))
          }
        </Marquee>
      </TabsContent>
    </Tabs>
  )
}
