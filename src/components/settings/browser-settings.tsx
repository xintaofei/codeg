"use client"

/**
 * Built-in browser settings: where links open by default (per source), whether
 * browser tabs get the web inspector, which native surface hosts them, whether
 * background tabs are unloaded after a while, where downloads land, the
 * browser profiles (create, clear, delete; which one new tabs open in) and the
 * sign-in user-agent switch.
 *
 * Preferences live in localStorage (`browser-prefs.ts`): written immediately,
 * mirrored across windows through the storage event, so there is no Save
 * button. The inspector and surface choices are read when a tab is created,
 * which is why their hints say "from now on". Desktop only — in web mode every
 * link goes to the system browser and none of this exists.
 */

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import {
  AppWindow,
  Download,
  Eraser,
  FileCode2,
  Globe,
  KeyRound,
  Link2,
  ListFilter,
  Lock,
  MoonStar,
  MousePointerClick,
  Network,
  Plus,
  Trash2,
  UserRound,
  Wrench,
} from "lucide-react"
import { toast } from "sonner"

import { SettingCard, SettingRow } from "@/components/shared/setting-card"
import { SettingsSection } from "@/components/shared/settings-section"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { toErrorMessage } from "@/lib/app-error"
import {
  browserCapabilitiesNow,
  browserClearData,
  browserRemoveProfile,
} from "@/lib/browser/browser-api"
import {
  DEFAULT_BROWSER_PROFILE_ID,
  LINK_SOURCES,
  addBrowserProfile,
  removeBrowserProfile,
  setBrowserDevtools,
  setBrowserHostRules,
  setBrowserHtmlPreviewEngine,
  setBrowserNewTabProfile,
  setBrowserSignInUserAgent,
  setBrowserSurfaceOverride,
  setBrowserSuspendBackgroundTabs,
  setBrowserTerminalClickMenu,
  setDefaultLinkTarget,
  useBrowserPrefs,
  type BrowserProfile,
  type LinkSource,
  type LinkTarget,
  type SurfaceOverride,
} from "@/lib/browser/browser-prefs"
import {
  HOST_RULE_ACTIONS,
  hostRulePatternKey,
  normalizeHostRulePattern,
  validateHostRulePattern,
  type HostRule,
  type HostRuleAction,
} from "@/lib/browser/host-rules"
import type {
  BrowserPolicyStatus,
  BrowserProxyStatus,
  WireHostRule,
} from "@/lib/browser/types"
import { isDesktop } from "@/lib/platform"

// Literal message keys per id — next-intl only resolves literal keys, so the
// lookup tables keep the rows data-driven without losing key checking.
const SOURCE_LABEL_KEYS = {
  transcript: "sourceTranscript",
  toolCard: "sourceToolCard",
  terminal: "sourceTerminal",
  editor: "sourceEditor",
  notification: "sourceNotification",
} as const satisfies Record<LinkSource, string>

const TARGETS: readonly LinkTarget[] = ["builtin", "system"]
const TARGET_LABEL_KEYS = {
  builtin: "targetBuiltin",
  system: "targetSystem",
} as const satisfies Record<LinkTarget, string>

const ACTION_LABEL_KEYS = {
  builtin: "targetBuiltin",
  system: "targetSystem",
  block: "ruleActionBlock",
} as const satisfies Record<HostRuleAction, string>

const SURFACES: readonly SurfaceOverride[] = ["auto", "child", "window"]
const SURFACE_LABEL_KEYS = {
  auto: "surfaceAuto",
  child: "surfaceChild",
  window: "surfaceWindow",
} as const satisfies Record<SurfaceOverride, string>

/**
 * One line for the proxy row: what browser tabs use right now and, where the
 * platform cannot switch live, what a change needs. Read-only — the proxy is
 * set in System settings, not here.
 */
export function proxyStatusLines(
  t: ReturnType<typeof useTranslations<"BrowserSettings">>,
  status: BrowserProxyStatus | null
): string[] {
  if (!status) return []
  if (status.applies === "unsupported") return [t("proxyUnsupported")]
  const lines: string[] = []
  if (status.url) lines.push(t("proxyOn", { url: status.url }))
  else if (status.reason) lines.push(t("proxyUnusable"))
  else lines.push(t("proxyOff"))
  if (status.applies === "restart" && status.reason)
    lines.push(t("proxyRestart"))
  if (status.applies === "next-tab") lines.push(t("proxyNextTab"))
  return lines
}

/**
 * The site-rule table: the administrator's rows first (read-only, with a
 * lock), then the user's, then a line to add one. Every change is written at
 * once, like the rest of the section. There is no ordering to manage: the
 * most specific pattern wins, which the hint says.
 */
function HostRulesEditor({
  rules,
  managed,
}: {
  rules: readonly HostRule[]
  managed: readonly WireHostRule[]
}) {
  const t = useTranslations("BrowserSettings")
  const [draft, setDraft] = useState("")
  const [draftAction, setDraftAction] = useState<HostRuleAction>("system")
  const [problem, setProblem] = useState<"invalid" | "duplicate" | null>(null)

  const add = () => {
    if (validateHostRulePattern(draft)) {
      setProblem("invalid")
      return
    }
    const pattern = normalizeHostRulePattern(draft)
    // Two spellings of one rule are one rule (`[::1]` and
    // `[0:0:0:0:0:0:0:1]`, case, whitespace): compare what they match, not
    // the text. Otherwise both would sit in the table and only one could
    // ever apply.
    const key = hostRulePatternKey(pattern)
    if (rules.some((rule) => hostRulePatternKey(rule.pattern) === key)) {
      setProblem("duplicate")
      return
    }
    setBrowserHostRules([...rules, { pattern, action: draftAction }])
    setDraft("")
    setProblem(null)
  }
  const setAction = (index: number, action: HostRuleAction) => {
    setBrowserHostRules(
      rules.map((rule, i) => (i === index ? { ...rule, action } : rule))
    )
  }
  const remove = (index: number) => {
    setBrowserHostRules(rules.filter((_, i) => i !== index))
    // A "duplicate" complaint may have been about this very row.
    if (problem === "duplicate") setProblem(null)
  }

  return (
    <div className="space-y-1.5">
      {managed.map((rule, index) => (
        <div
          key={`managed:${index}:${rule.pattern}`}
          className="flex items-center justify-between gap-3 rounded-lg border border-border/70 bg-muted/40 px-3 py-2"
          title={t("ruleManaged")}
        >
          <span className="flex min-w-0 items-center gap-2">
            <Lock
              className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
              aria-label={t("ruleManaged")}
            />
            <span className="truncate font-mono text-xs">{rule.pattern}</span>
          </span>
          <span className="shrink-0 text-xs text-muted-foreground">
            {t(ACTION_LABEL_KEYS[rule.action])}
          </span>
        </div>
      ))}
      {rules.map((rule, index) => (
        <div
          // Position plus pattern: two rows can carry the same pattern when
          // the table was written by hand, and the key must still be unique.
          key={`${index}:${rule.pattern}`}
          className="flex items-center justify-between gap-3 rounded-lg border border-border/70 bg-background px-3 py-2"
        >
          <span className="min-w-0 truncate font-mono text-xs">
            {rule.pattern}
          </span>
          <div className="flex shrink-0 items-center gap-1">
            <Select
              value={rule.action}
              onValueChange={(value) =>
                setAction(index, value as HostRuleAction)
              }
            >
              <SelectTrigger
                size="sm"
                className="w-40 bg-background text-xs"
                aria-label={t("ruleActionFor", { pattern: rule.pattern })}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                {HOST_RULE_ACTIONS.map((action) => (
                  <SelectItem key={action} value={action}>
                    {t(ACTION_LABEL_KEYS[action])}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              title={t("ruleRemove")}
              aria-label={t("ruleRemove")}
              onClick={() => remove(index)}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      ))}
      {rules.length === 0 && managed.length === 0 ? (
        <p className="text-xs text-muted-foreground">{t("rulesEmpty")}</p>
      ) : null}
      <form
        className="flex items-start gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          add()
        }}
      >
        <div className="min-w-0 flex-1">
          <Input
            value={draft}
            onChange={(event) => {
              setDraft(event.target.value)
              if (problem) setProblem(null)
            }}
            placeholder={t("rulePatternPlaceholder")}
            aria-label={t("rulePatternLabel")}
            aria-invalid={problem ? true : undefined}
            className="h-8 bg-background font-mono text-xs"
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
          />
          {problem ? (
            <p className="mt-1 text-xs text-destructive">
              {t(problem === "duplicate" ? "ruleDuplicate" : "ruleInvalid")}
            </p>
          ) : null}
        </div>
        <Select
          value={draftAction}
          onValueChange={(value) => setDraftAction(value as HostRuleAction)}
        >
          <SelectTrigger
            size="sm"
            className="w-40 bg-background text-xs"
            aria-label={t("ruleActionLabel")}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent align="end">
            {HOST_RULE_ACTIONS.map((action) => (
              <SelectItem key={action} value={action}>
                {t(ACTION_LABEL_KEYS[action])}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          type="submit"
          variant="outline"
          size="sm"
          className="bg-background"
        >
          <Plus className="h-3.5 w-3.5" />
          {t("ruleAdd")}
        </Button>
      </form>
    </div>
  )
}

/** A profile as the settings list shows it: the default one with its
 *  localized name, or one the user created. */
interface ProfileRow {
  id: string
  name: string
  isDefault: boolean
}

/**
 * The profile list: the default profile first, then the user's, each with
 * "clear data" and (except the default) "delete", and a line to add one.
 * Adding writes the preference at once — the backend needs nothing until a
 * tab is opened in the profile, which is when its store is created. Deleting
 * goes to the backend first (it closes the profile's tabs and removes the
 * store) and drops the entry only once that succeeded, so a failure never
 * leaves data behind that the settings no longer show.
 */
function ProfilesEditor({
  rows,
  onClear,
  onDelete,
}: {
  rows: readonly ProfileRow[]
  onClear: (row: ProfileRow) => void
  onDelete: (row: ProfileRow) => void
}) {
  const t = useTranslations("BrowserSettings")
  const [draft, setDraft] = useState("")
  const [problem, setProblem] = useState<"required" | "duplicate" | null>(null)

  const add = () => {
    const name = draft.trim()
    if (!name) {
      setProblem("required")
      return
    }
    // Two profiles with one name would be told apart by nothing the user can
    // see; case and surrounding spaces are not a difference either.
    const key = name.toLocaleLowerCase()
    if (rows.some((row) => row.name.trim().toLocaleLowerCase() === key)) {
      setProblem("duplicate")
      return
    }
    addBrowserProfile(name)
    setDraft("")
    setProblem(null)
  }

  return (
    <div className="space-y-1.5">
      {rows.map((row) => (
        <div
          key={row.id}
          className="flex items-center justify-between gap-3 rounded-lg border border-border/70 bg-background px-3 py-2"
        >
          <span className="flex min-w-0 items-center gap-2">
            <UserRound
              className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <span className="truncate text-sm">{row.name}</span>
          </span>
          <div className="flex shrink-0 items-center gap-1">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7"
              title={t("profileClear", { name: row.name })}
              aria-label={t("profileClear", { name: row.name })}
              onClick={() => onClear(row)}
            >
              <Eraser className="h-3.5 w-3.5" />
            </Button>
            {row.isDefault ? null : (
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-7 w-7"
                title={t("profileDelete", { name: row.name })}
                aria-label={t("profileDelete", { name: row.name })}
                onClick={() => onDelete(row)}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            )}
          </div>
        </div>
      ))}
      <form
        className="flex items-start gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          add()
        }}
      >
        <div className="min-w-0 flex-1">
          <Input
            value={draft}
            onChange={(event) => {
              setDraft(event.target.value)
              if (problem) setProblem(null)
            }}
            placeholder={t("profileNamePlaceholder")}
            aria-label={t("profileNameLabel")}
            aria-invalid={problem ? true : undefined}
            className="h-8 bg-background text-xs"
            autoComplete="off"
          />
          {problem ? (
            <p className="mt-1 text-xs text-destructive">
              {t(
                problem === "duplicate"
                  ? "profileNameDuplicate"
                  : "profileNameRequired"
              )}
            </p>
          ) : null}
        </div>
        <Button
          type="submit"
          variant="outline"
          size="sm"
          className="bg-background"
        >
          <Plus className="h-3.5 w-3.5" />
          {t("profileAdd")}
        </Button>
      </form>
    </div>
  )
}

export function BrowserSettingsSection() {
  const t = useTranslations("BrowserSettings")
  const prefs = useBrowserPrefs()
  // Folded on arrival like its neighbours: the General tab is a stack of
  // sections, and this one is five pickers tall.
  const [expanded, setExpanded] = useState(false)
  // Which profile the clear / delete dialog is about (null = closed). Ids,
  // not rows: the row is derived from the current list on every render, so
  // a profile another window deletes while the dialog is open closes it
  // rather than being cleared or deleted again under a stale name.
  const [confirmClearId, setConfirmClearId] = useState<string | null>(null)
  const [clearing, setClearing] = useState(false)
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const [deleting, setDeleting] = useState(false)
  const [proxy, setProxy] = useState<BrowserProxyStatus | null>(null)
  const [downloadsDir, setDownloadsDir] = useState<string | null>(null)
  const [policy, setPolicy] = useState<BrowserPolicyStatus | null>(null)
  // Whether this build hosts document guests (null until known); the HTML
  // preview switch is inert where there is none to switch to.
  const [docGuest, setDocGuest] = useState<boolean | null>(null)
  // Whether more than the default profile can exist here (macOS 14+,
  // Windows, Linux). Until known, or where not, the section shows the one
  // "clear browsing data" row instead of the profile list.
  const [profilesSupported, setProfilesSupported] = useState<boolean | null>(
    null
  )
  // Whether the sign-in user agent is applied on this platform at all.
  const [signInUaSupported, setSignInUaSupported] = useState<boolean | null>(
    null
  )

  // Fetched when the section opens (not once per app run): the answer follows
  // the proxy setting, which lives on another settings page.
  useEffect(() => {
    if (!expanded) return
    let cancelled = false
    browserCapabilitiesNow()
      .then((caps) => {
        if (cancelled) return
        setProxy(caps.proxy)
        setDownloadsDir(caps.downloadsDir || null)
        setPolicy(caps.policy ?? null)
        setDocGuest(caps.docGuest ?? false)
        setProfilesSupported(caps.profiles ?? false)
        setSignInUaSupported(caps.signInUserAgent ?? false)
      })
      .catch(() => {
        if (cancelled) return
        setProxy(null)
        setDownloadsDir(null)
        setPolicy(null)
        setDocGuest(null)
        setProfilesSupported(null)
        setSignInUaSupported(null)
      })
    return () => {
      cancelled = true
    }
  }, [expanded])

  if (!isDesktop()) return null

  const profileRows: ProfileRow[] = [
    {
      id: DEFAULT_BROWSER_PROFILE_ID,
      name: t("profileDefault"),
      isDefault: true,
    },
    ...prefs.profiles.map((profile: BrowserProfile) => ({
      id: profile.id,
      name: profile.name,
      isDefault: false,
    })),
  ]

  const confirmClear =
    profileRows.find((row) => row.id === confirmClearId) ?? null
  const confirmDelete =
    profileRows.find((row) => row.id === confirmDeleteId) ?? null

  const clear = async (row: ProfileRow) => {
    setClearing(true)
    try {
      await browserClearData(row.id)
      toast.success(t("cleared"))
      setConfirmClearId(null)
    } catch (error) {
      toast.error(t("clearFailed", { message: toErrorMessage(error) }))
    } finally {
      setClearing(false)
    }
  }

  const remove = async (row: ProfileRow) => {
    setDeleting(true)
    try {
      await browserRemoveProfile(row.id)
      // Only now: a profile the backend could not delete keeps its entry, so
      // its data is never orphaned behind a list that no longer names it.
      removeBrowserProfile(row.id)
      toast.success(t("profileDeleted"))
      setConfirmDeleteId(null)
    } catch (error) {
      toast.error(t("profileDeleteFailed", { message: toErrorMessage(error) }))
    } finally {
      setDeleting(false)
    }
  }

  return (
    <SettingsSection
      icon={Globe}
      title={t("title")}
      description={t("description")}
      collapsible
      open={expanded}
      onOpenChange={setExpanded}
    >
      {policy && !policy.enabled ? (
        <p className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-foreground">
          {t("managedDisabled")}
        </p>
      ) : null}
      <SettingCard>
        {/* One setting with five values, so one row whose control is the
            list — not five rows repeating the same explanation. */}
        <SettingRow
          icon={Link2}
          title={t("defaultTargetTitle")}
          description={t("defaultTargetHint")}
        >
          <div className="space-y-1.5">
            {LINK_SOURCES.map((source) => {
              const label = t(SOURCE_LABEL_KEYS[source])
              return (
                <div
                  key={source}
                  className="flex items-center justify-between gap-3 rounded-lg border border-border/70 bg-background px-3 py-2"
                >
                  <span className="min-w-0 truncate text-sm">{label}</span>
                  <Select
                    value={prefs.defaultTarget[source]}
                    onValueChange={(value) =>
                      setDefaultLinkTarget(source, value as LinkTarget)
                    }
                  >
                    <SelectTrigger
                      size="sm"
                      className="w-44 bg-background text-xs"
                      aria-label={label}
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent align="end">
                      {TARGETS.map((target) => (
                        <SelectItem key={target} value={target}>
                          {t(TARGET_LABEL_KEYS[target])}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              )
            })}
          </div>
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow
          icon={ListFilter}
          title={t("rulesTitle")}
          description={t("rulesHint")}
        >
          <HostRulesEditor
            rules={prefs.hostRules}
            managed={policy?.managedRules ?? []}
          />
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow
          icon={MousePointerClick}
          title={t("terminalMenuTitle")}
          description={t("terminalMenuHint")}
          htmlFor="browser-terminal-menu"
          control={
            <Switch
              id="browser-terminal-menu"
              checked={prefs.terminalClickMenu}
              onCheckedChange={(enabled) =>
                setBrowserTerminalClickMenu(enabled)
              }
            />
          }
        />
        <SettingRow
          icon={FileCode2}
          title={t("htmlPreviewTitle")}
          description={t("htmlPreviewHint")}
          htmlFor="browser-html-preview"
          control={
            <Switch
              id="browser-html-preview"
              checked={prefs.htmlPreviewEngine === "guest"}
              disabled={docGuest === false}
              onCheckedChange={(enabled) =>
                setBrowserHtmlPreviewEngine(enabled ? "guest" : "inline")
              }
            />
          }
        />
        <SettingRow
          icon={Wrench}
          title={t("devtoolsTitle")}
          description={t("devtoolsHint")}
          htmlFor="browser-devtools"
          control={
            <Switch
              id="browser-devtools"
              checked={prefs.devtools}
              onCheckedChange={(enabled) => setBrowserDevtools(enabled)}
            />
          }
        />
        <SettingRow
          icon={AppWindow}
          title={t("surfaceTitle")}
          description={t("surfaceHint")}
          control={
            <Select
              value={prefs.surfaceOverride}
              onValueChange={(value) =>
                setBrowserSurfaceOverride(value as SurfaceOverride)
              }
            >
              <SelectTrigger
                size="sm"
                className="w-44 bg-background text-xs"
                aria-label={t("surfaceTitle")}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                {SURFACES.map((surface) => (
                  <SelectItem key={surface} value={surface}>
                    {t(SURFACE_LABEL_KEYS[surface])}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          }
        />
        <SettingRow
          icon={MoonStar}
          title={t("suspendTitle")}
          description={t("suspendHint")}
          htmlFor="browser-suspend"
          control={
            <Switch
              id="browser-suspend"
              checked={prefs.suspendBackgroundTabs}
              onCheckedChange={(enabled) =>
                setBrowserSuspendBackgroundTabs(enabled)
              }
            />
          }
        />
        <SettingRow
          icon={Network}
          title={t("proxyTitle")}
          description={t("proxyHint")}
        >
          <div className="space-y-0.5 text-xs text-muted-foreground">
            {proxyStatusLines(t, proxy).map((line) => (
              <p key={line} className="break-all">
                {line}
              </p>
            ))}
          </div>
        </SettingRow>
        <SettingRow
          icon={Download}
          title={t("downloadsTitle")}
          description={t("downloadsHint", { dir: downloadsDir ?? "…" })}
        />
        {signInUaSupported ? (
          <SettingRow
            icon={KeyRound}
            title={t("signInUaTitle")}
            description={t("signInUaHint")}
            htmlFor="browser-sign-in-ua"
            control={
              <Switch
                id="browser-sign-in-ua"
                checked={prefs.signInUserAgent}
                onCheckedChange={(enabled) =>
                  setBrowserSignInUserAgent(enabled)
                }
              />
            }
          />
        ) : null}
        {profilesSupported ? null : (
          <SettingRow
            icon={Eraser}
            title={t("clearTitle")}
            description={t("clearHint")}
            control={
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="bg-background"
                onClick={() => setConfirmClearId(profileRows[0].id)}
              >
                {t("clearAction")}
              </Button>
            }
          />
        )}
      </SettingCard>

      {profilesSupported ? (
        <SettingCard>
          <SettingRow
            icon={UserRound}
            title={t("profilesTitle")}
            description={t("profilesHint")}
          >
            <ProfilesEditor
              rows={profileRows}
              onClear={(row) => setConfirmClearId(row.id)}
              onDelete={(row) => setConfirmDeleteId(row.id)}
            />
          </SettingRow>
          <SettingRow
            icon={Plus}
            title={t("profileNewTabsTitle")}
            control={
              <Select
                value={prefs.newTabProfile}
                onValueChange={(value) => setBrowserNewTabProfile(value)}
              >
                <SelectTrigger
                  size="sm"
                  className="w-44 bg-background text-xs"
                  aria-label={t("profileNewTabsTitle")}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent align="end">
                  {profileRows.map((row) => (
                    <SelectItem key={row.id} value={row.id}>
                      {row.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            }
          />
        </SettingCard>
      ) : null}

      <AlertDialog
        open={confirmClear !== null}
        onOpenChange={(open) => {
          if (!clearing && !open) setConfirmClearId(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("clearConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {profilesSupported
                ? t("clearConfirmDescriptionProfile", {
                    name: confirmClear?.name ?? "",
                  })
                : t("clearConfirmDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={clearing}>
              {t("cancel")}
            </AlertDialogCancel>
            {/* Stays open until the backend answers, so a failure toast has
                the dialog it belongs to still on screen. */}
            <AlertDialogAction
              disabled={clearing}
              onClick={(event) => {
                event.preventDefault()
                if (confirmClear) void clear(confirmClear)
              }}
            >
              {t("clearConfirmAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={confirmDelete !== null}
        onOpenChange={(open) => {
          if (!deleting && !open) setConfirmDeleteId(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("profileDeleteConfirmTitle", {
                name: confirmDelete?.name ?? "",
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("profileDeleteConfirmDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>
              {t("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={deleting}
              onClick={(event) => {
                event.preventDefault()
                if (confirmDelete) void remove(confirmDelete)
              }}
            >
              {t("profileDeleteConfirmAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsSection>
  )
}
