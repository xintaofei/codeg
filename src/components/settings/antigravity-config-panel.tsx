"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Eye, EyeOff, Loader2, Save } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  acpSyncAntigravitySettings,
  type AntigravitySyncReport,
} from "@/lib/api"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import type { AcpAgentInfo } from "@/lib/types"

/** codeg-side knob recording the chosen auth method. The agent reads NOTHING
 * from it — its auth intent comes from `auth.type` in
 * `<GEMINI_HOME>/antigravity-acp/settings.json` — so the launch path uses it
 * twice: to write that file, and to scrub the credential vars the other
 * methods use (`apply_antigravity_env_policy` in `acp/connection.rs`). */
const AGY_AUTH_METHOD_ENV = "AGY_AUTH_METHOD"
/** The Gemini Developer API key, read when `auth.type = gemini-api-key`. */
const GEMINI_API_KEY_ENV = "GEMINI_API_KEY"
/** Agent Platform's own key. Mutually exclusive with the project/location
 * pair: when it is set the server suppresses both (its `_vertex_config` logs
 * "project and location suppressed by the key"). */
const GOOGLE_API_KEY_ENV = "GOOGLE_API_KEY"
const GOOGLE_CLOUD_PROJECT_ENV = "GOOGLE_CLOUD_PROJECT"
const GOOGLE_CLOUD_LOCATION_ENV = "GOOGLE_CLOUD_LOCATION"

/** Every key this panel owns, so the settings page can fold the whole set into
 * the raw env draft (see `persistEnv`'s `draftEnvPatch`). */
export const ANTIGRAVITY_ENV_KEYS = [
  AGY_AUTH_METHOD_ENV,
  GEMINI_API_KEY_ENV,
  GOOGLE_API_KEY_ENV,
  GOOGLE_CLOUD_PROJECT_ENV,
  GOOGLE_CLOUD_LOCATION_ENV,
] as const

/**
 * The four `auth.type` values Antigravity's ACP server accepts, in the order it
 * advertises them at `initialize`. The pre-rebrand `vertex-ai` spelling is
 * still accepted by the server but never written by codeg — a second name for
 * one method would render as two buttons.
 */
export type AntigravityAuthMethod =
  | "oauth-personal"
  | "oauth-business"
  | "gemini-api-key"
  | "agent-platform"

const AUTH_METHODS: AntigravityAuthMethod[] = [
  "oauth-personal",
  "oauth-business",
  "gemini-api-key",
  "agent-platform",
]

function isAuthMethod(value: string): value is AntigravityAuthMethod {
  return (AUTH_METHODS as string[]).includes(value)
}

/** The persisted method, defaulting to the browser login — the one path that
 * needs no credential of its own, and the one most users are on. */
export function inferAntigravityMethod(
  env: Record<string, string>
): AntigravityAuthMethod {
  const explicit = (env[AGY_AUTH_METHOD_ENV] ?? "").trim()
  return isAuthMethod(explicit) ? explicit : "oauth-personal"
}

export interface AntigravityFormValues {
  method: AntigravityAuthMethod
  geminiApiKey: string
  googleApiKey: string
  gcpProject: string
  gcpLocation: string
}

/**
 * Build the env map to persist.
 *
 * Only the credentials the chosen method actually consumes are written; every
 * other one is DELETED rather than left behind. That matters because the
 * method and the credential are not independent here — a `GEMINI_API_KEY` left
 * over from a previous choice would authenticate the user as something they
 * did not pick, and would contradict the `auth.type` the same launch writes to
 * `settings.json`. (The launch path scrubs them again for the same reason; this
 * is the half that keeps the stored row honest.)
 *
 * `GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_LOCATION` are kept for BOTH
 * `agent-platform` (which reads them from the environment) and `oauth-business`
 * (which reads project/location from `settings.json` only — the launch path
 * copies them into the `gcp` block from these same values).
 *
 * Unrelated keys are preserved untouched.
 */
export function buildAntigravityEnv(
  prevEnv: Record<string, string>,
  values: AntigravityFormValues
): Record<string, string> {
  const env: Record<string, string> = { ...prevEnv }
  const set = (key: string, value: string) => {
    const trimmed = value.trim()
    if (trimmed) {
      env[key] = trimmed
    } else {
      delete env[key]
    }
  }

  env[AGY_AUTH_METHOD_ENV] = values.method
  for (const key of [
    GEMINI_API_KEY_ENV,
    GOOGLE_API_KEY_ENV,
    GOOGLE_CLOUD_PROJECT_ENV,
    GOOGLE_CLOUD_LOCATION_ENV,
  ]) {
    delete env[key]
  }

  if (values.method === "gemini-api-key") {
    set(GEMINI_API_KEY_ENV, values.geminiApiKey)
  }
  if (values.method === "agent-platform") {
    set(GOOGLE_API_KEY_ENV, values.googleApiKey)
    // The API key suppresses project/location server-side, so writing all
    // three would silently ignore two of them. Keep the row matching what the
    // server will actually use.
    if (!values.googleApiKey.trim()) {
      set(GOOGLE_CLOUD_PROJECT_ENV, values.gcpProject)
      set(GOOGLE_CLOUD_LOCATION_ENV, values.gcpLocation)
    }
  }
  if (values.method === "oauth-business") {
    set(GOOGLE_CLOUD_PROJECT_ENV, values.gcpProject)
    set(GOOGLE_CLOUD_LOCATION_ENV, values.gcpLocation)
  }

  return env
}

/**
 * Whether the form can produce a working session, and if not, which
 * translation key explains why.
 *
 * These mirror the server's own `auth_required` messages: `gemini-api-key`
 * needs `GEMINI_API_KEY`; `agent-platform` needs `GOOGLE_API_KEY` **or** both a
 * project and a location; Gemini Enterprise needs both `gcp.project` and
 * `gcp.location` (a partial config falls through to its no-config path).
 * `oauth-personal` needs nothing — the server opens a browser.
 */
export function antigravityIncompleteReason(
  values: AntigravityFormValues
):
  | "missingGeminiApiKey"
  | "missingAgentPlatformConfig"
  | "missingEnterpriseConfig"
  | null {
  const has = (value: string) => value.trim().length > 0
  switch (values.method) {
    case "gemini-api-key":
      return has(values.geminiApiKey) ? null : "missingGeminiApiKey"
    case "agent-platform":
      if (has(values.googleApiKey)) return null
      return has(values.gcpProject) && has(values.gcpLocation)
        ? null
        : "missingAgentPlatformConfig"
    case "oauth-business":
      return has(values.gcpProject) && has(values.gcpLocation)
        ? null
        : "missingEnterpriseConfig"
    default:
      return null
  }
}

/**
 * Dedicated settings panel for Google Antigravity (`agy_acp_server`).
 *
 * This panel is load-bearing, not cosmetic. Antigravity's `session/new` fails
 * outright with `Authentication required` unless
 * `<GEMINI_HOME>/antigravity-acp/settings.json` declares an `auth.type`
 * (upstream removed environment-based auth selection), and codeg does not
 * implement the ACP `authenticate` request that would otherwise set it. The
 * choice made here is what the launch path writes to that file — so without it
 * the agent cannot start a single session.
 *
 * Model and session mode are deliberately absent: the server reports both as
 * standard ACP config options, so the composer's own selectors own them per
 * session. A second copy here would only create two places to disagree about
 * what a session runs on.
 */
export function AntigravityConfigPanel({
  agent,
  saving,
  onSaveEnv,
  onSaved,
}: {
  agent: AcpAgentInfo
  saving: boolean
  onSaveEnv: (env: Record<string, string>, enabled: boolean) => Promise<unknown>
  onSaved: () => void
}) {
  const t = useTranslations("AcpAgentSettings")

  const [method, setMethod] = useState<AntigravityAuthMethod>(() =>
    inferAntigravityMethod(agent.env)
  )
  const [geminiApiKey, setGeminiApiKey] = useState(
    () => agent.env[GEMINI_API_KEY_ENV] ?? ""
  )
  const [googleApiKey, setGoogleApiKey] = useState(
    () => agent.env[GOOGLE_API_KEY_ENV] ?? ""
  )
  const [gcpProject, setGcpProject] = useState(
    () => agent.env[GOOGLE_CLOUD_PROJECT_ENV] ?? ""
  )
  const [gcpLocation, setGcpLocation] = useState(
    () => agent.env[GOOGLE_CLOUD_LOCATION_ENV] ?? ""
  )
  const [showKey, setShowKey] = useState(false)
  const [savingForm, setSavingForm] = useState(false)
  /** The last save whose settings.json write was REFUSED, or null. Held rather
   *  than left to the toast: this is a standing disagreement between the form
   *  and the file the agent reads, and it stays true until someone edits that
   *  file — a notice that scrolls away after four seconds would not say so. */
  const [syncSkip, setSyncSkip] = useState<AntigravitySyncReport | null>(null)

  const mountedRef = useRef(true)
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  // Re-seed when the persisted row changes UNDER the panel (the settings page
  // refetches after every save, and another window can rewrite it). Edits in
  // progress win: `dirty` is a ref set by the change handlers rather than a
  // comparison against current state, because that state must NOT be a
  // dependency — between a save completing and the refresh it triggers, the
  // form already holds what was stored while `agent` still holds the pre-save
  // values, and a re-run would revert the save on screen.
  const persisted = useMemo(
    () => ({
      method: inferAntigravityMethod(agent.env),
      geminiApiKey: agent.env[GEMINI_API_KEY_ENV] ?? "",
      googleApiKey: agent.env[GOOGLE_API_KEY_ENV] ?? "",
      gcpProject: agent.env[GOOGLE_CLOUD_PROJECT_ENV] ?? "",
      gcpLocation: agent.env[GOOGLE_CLOUD_LOCATION_ENV] ?? "",
    }),
    [agent.env]
  )
  const seeded = useRef(persisted)
  const dirty = useRef(false)
  useEffect(() => {
    const previous = seeded.current
    const changed =
      previous.method !== persisted.method ||
      previous.geminiApiKey !== persisted.geminiApiKey ||
      previous.googleApiKey !== persisted.googleApiKey ||
      previous.gcpProject !== persisted.gcpProject ||
      previous.gcpLocation !== persisted.gcpLocation
    if (!changed) return
    seeded.current = persisted
    if (dirty.current) return
    setMethod(persisted.method)
    setGeminiApiKey(persisted.geminiApiKey)
    setGoogleApiKey(persisted.googleApiKey)
    setGcpProject(persisted.gcpProject)
    setGcpLocation(persisted.gcpLocation)
  }, [persisted])

  const values: AntigravityFormValues = {
    method,
    geminiApiKey,
    googleApiKey,
    gcpProject,
    gcpLocation,
  }
  const valuesRef = useRef(values)
  useEffect(() => {
    valuesRef.current = values
  })

  const incomplete = antigravityIncompleteReason(values)

  const save = useCallback(async () => {
    setSavingForm(true)
    const submitted = valuesRef.current
    try {
      await onSaveEnv(buildAntigravityEnv(agent.env, submitted), agent.enabled)
      // Only the exact values that just landed stop counting as an unsaved
      // edit: the form stays editable while the request is in flight, so
      // anything changed since is NEWER than what was stored.
      if (valuesRef.current === submitted) dirty.current = false

      // Storing the row is only half of a save. What actually authenticates
      // Antigravity is `auth.type` in the server's settings.json, and that file
      // is the user's — it can legitimately be Hjson with comments, or hold an
      // `auth` key of a shape codeg refuses to overwrite — in which case the
      // launch leaves it alone and only writes a log line. Saying "saved"
      // regardless was claiming something that had not happened, and the
      // consequence lands later and elsewhere: switching methods scrubs the
      // credentials for the NEW method while the server still reads the OLD
      // auth.type, so the next session fails with no credential for the method
      // it believes it is using. Ask, and say so while the user is still here.
      let report: AntigravitySyncReport | null = null
      try {
        report = await acpSyncAntigravitySettings()
      } catch {
        // The sync is a report, not the save. A transport failure leaves the
        // row stored and the launch-time sync still to come, so it must not
        // turn a successful save into a failed one.
      }
      if (mountedRef.current)
        setSyncSkip(report?.status === "skipped" ? report : null)
      if (report?.status === "skipped") {
        toast.warning(t("toasts.antigravitySyncSkipped"))
      } else {
        toast.success(t("toasts.antigravitySaved"))
      }
      onSaved()
    } catch (e) {
      toast.error(
        `${t("toasts.saveAntigravityConfigFailed")}: ${
          e instanceof Error ? e.message : String(e)
        }`
      )
    } finally {
      if (mountedRef.current) setSavingForm(false)
    }
  }, [agent.enabled, agent.env, onSaveEnv, onSaved, t])

  const busy = saving || savingForm
  const markDirty = () => {
    dirty.current = true
  }

  return (
    <div className="space-y-3 rounded-md border bg-muted/10 p-3">
      <div>
        <label className="text-xs font-medium">{t("configManagement")}</label>
        <p className="mt-1 text-[11px] text-muted-foreground">
          {t("antigravity.configDescription")}
        </p>
      </div>

      <div className="space-y-2 rounded-md border bg-background/60 p-2.5">
        <div className="space-y-1">
          <label
            className="text-[11px] text-muted-foreground"
            htmlFor="antigravity-auth-method"
          >
            {t("antigravity.methodLabel")}
          </label>
          <Select
            onValueChange={(value) => {
              if (!isAuthMethod(value)) return
              markDirty()
              setMethod(value)
            }}
            value={method}
          >
            <SelectTrigger className="h-7 text-xs" id="antigravity-auth-method">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {AUTH_METHODS.map((value) => (
                <SelectItem key={value} value={value}>
                  {t(`antigravity.methods.${value}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className="text-[10px] text-muted-foreground">
            {t(`antigravity.methodHints.${method}`)}
          </p>
        </div>

        {method === "gemini-api-key" ? (
          <div className="space-y-1">
            <label
              className="text-[11px] text-muted-foreground"
              htmlFor="antigravity-gemini-key"
            >
              {t("antigravity.geminiApiKeyLabel")}
            </label>
            <div className="flex items-center gap-1.5">
              <Input
                className="h-7 flex-1 text-xs"
                id="antigravity-gemini-key"
                onChange={(e) => {
                  markDirty()
                  setGeminiApiKey(e.target.value)
                }}
                placeholder="AIza..."
                type={showKey ? "text" : "password"}
                value={geminiApiKey}
              />
              <Button
                className="h-7 w-7 shrink-0"
                onClick={() => setShowKey((v) => !v)}
                size="icon"
                title={
                  showKey ? t("actions.hideApiKey") : t("actions.showApiKey")
                }
                type="button"
                variant="ghost"
              >
                {showKey ? (
                  <EyeOff className="h-3.5 w-3.5" />
                ) : (
                  <Eye className="h-3.5 w-3.5" />
                )}
              </Button>
            </div>
          </div>
        ) : null}

        {method === "agent-platform" ? (
          <div className="space-y-1">
            <label
              className="text-[11px] text-muted-foreground"
              htmlFor="antigravity-google-key"
            >
              {t("antigravity.googleApiKeyLabel")}
            </label>
            <div className="flex items-center gap-1.5">
              <Input
                className="h-7 flex-1 text-xs"
                id="antigravity-google-key"
                onChange={(e) => {
                  markDirty()
                  setGoogleApiKey(e.target.value)
                }}
                placeholder="AIza..."
                type={showKey ? "text" : "password"}
                value={googleApiKey}
              />
              <Button
                className="h-7 w-7 shrink-0"
                onClick={() => setShowKey((v) => !v)}
                size="icon"
                title={
                  showKey ? t("actions.hideApiKey") : t("actions.showApiKey")
                }
                type="button"
                variant="ghost"
              >
                {showKey ? (
                  <EyeOff className="h-3.5 w-3.5" />
                ) : (
                  <Eye className="h-3.5 w-3.5" />
                )}
              </Button>
            </div>
            <p className="text-[10px] text-muted-foreground">
              {t("antigravity.googleApiKeyHint")}
            </p>
          </div>
        ) : null}

        {method === "oauth-business" ||
        (method === "agent-platform" && !googleApiKey.trim()) ? (
          <div className="grid grid-cols-2 gap-2">
            <div className="space-y-1">
              <label
                className="text-[11px] text-muted-foreground"
                htmlFor="antigravity-gcp-project"
              >
                {t("antigravity.gcpProjectLabel")}
              </label>
              <Input
                className="h-7 text-xs"
                id="antigravity-gcp-project"
                onChange={(e) => {
                  markDirty()
                  setGcpProject(e.target.value)
                }}
                placeholder="my-project"
                value={gcpProject}
              />
            </div>
            <div className="space-y-1">
              <label
                className="text-[11px] text-muted-foreground"
                htmlFor="antigravity-gcp-location"
              >
                {t("antigravity.gcpLocationLabel")}
              </label>
              <Input
                className="h-7 text-xs"
                id="antigravity-gcp-location"
                onChange={(e) => {
                  markDirty()
                  setGcpLocation(e.target.value)
                }}
                placeholder="global"
                value={gcpLocation}
              />
            </div>
          </div>
        ) : null}

        {incomplete ? (
          <p className="text-[11px] text-amber-600 dark:text-amber-400">
            {t(`antigravity.${incomplete}`)}
          </p>
        ) : null}

        {/* The save landed in the database but not in the file the server
            reads, so the choice above is NOT what the next session will
            authenticate with. Names the file and the reason, because the fix
            is to edit that file by hand — codeg will not touch it again. */}
        {syncSkip ? (
          <div className="space-y-1 rounded-md border border-amber-500/40 bg-amber-500/5 p-2">
            <p className="text-[11px] text-amber-600 dark:text-amber-400">
              {t("antigravity.syncSkipped")}
            </p>
            {syncSkip.reason ? (
              <p className="text-[10px] text-muted-foreground">
                {syncSkip.reason}
              </p>
            ) : null}
            <code className="block overflow-x-auto rounded bg-muted px-2 py-1 font-mono text-[10px] whitespace-nowrap text-muted-foreground">
              {syncSkip.path}
            </code>
          </div>
        ) : null}

        {agent.config_file_path ? (
          <div className="space-y-1">
            <p className="text-[10px] text-muted-foreground">
              {t("antigravity.settingsFileHint")}
            </p>
            <code className="block overflow-x-auto rounded bg-muted px-2 py-1 font-mono text-[10px] whitespace-nowrap text-muted-foreground">
              {agent.config_file_path}
            </code>
          </div>
        ) : null}

        <div className="flex justify-end">
          <Button
            className="h-7 gap-1.5 px-2.5 text-xs"
            disabled={busy}
            onClick={() => void save()}
            size="sm"
            type="button"
          >
            {busy ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Save className="h-3.5 w-3.5" />
            )}
            {t("antigravity.save")}
          </Button>
        </div>
      </div>
    </div>
  )
}
