"use client"

/**
 * Built-in browser settings: where links open by default (per source), whether
 * browser tabs get the web inspector, which native surface hosts them, and a
 * one-shot "clear browsing data".
 *
 * Preferences live in localStorage (`browser-prefs.ts`): written immediately,
 * mirrored across windows through the storage event, so there is no Save
 * button. The inspector and surface choices are read when a tab is created,
 * which is why their hints say "from now on". Desktop only — in web mode every
 * link goes to the system browser and none of this exists.
 */

import { useState } from "react"
import { useTranslations } from "next-intl"
import { AppWindow, Eraser, Globe, Link2, Wrench } from "lucide-react"
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { toErrorMessage } from "@/lib/app-error"
import { browserClearData } from "@/lib/browser/browser-api"
import {
  LINK_SOURCES,
  setBrowserDevtools,
  setBrowserSurfaceOverride,
  setDefaultLinkTarget,
  useBrowserPrefs,
  type LinkSource,
  type LinkTarget,
  type SurfaceOverride,
} from "@/lib/browser/browser-prefs"
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

const SURFACES: readonly SurfaceOverride[] = ["auto", "child", "window"]
const SURFACE_LABEL_KEYS = {
  auto: "surfaceAuto",
  child: "surfaceChild",
  window: "surfaceWindow",
} as const satisfies Record<SurfaceOverride, string>

export function BrowserSettingsSection() {
  const t = useTranslations("BrowserSettings")
  const prefs = useBrowserPrefs()
  // Folded on arrival like its neighbours: the General tab is a stack of
  // sections, and this one is five pickers tall.
  const [expanded, setExpanded] = useState(false)
  const [confirmClear, setConfirmClear] = useState(false)
  const [clearing, setClearing] = useState(false)

  if (!isDesktop()) return null

  const clear = async () => {
    setClearing(true)
    try {
      await browserClearData()
      toast.success(t("cleared"))
      setConfirmClear(false)
    } catch (error) {
      toast.error(t("clearFailed", { message: toErrorMessage(error) }))
    } finally {
      setClearing(false)
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
          icon={Eraser}
          title={t("clearTitle")}
          description={t("clearHint")}
          control={
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="bg-background"
              onClick={() => setConfirmClear(true)}
            >
              {t("clearAction")}
            </Button>
          }
        />
      </SettingCard>

      <AlertDialog
        open={confirmClear}
        onOpenChange={(open) => {
          if (!clearing) setConfirmClear(open)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("clearConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("clearConfirmDescription")}
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
                void clear()
              }}
            >
              {t("clearConfirmAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsSection>
  )
}
