"use client"

/**
 * Persists user's selector preferences (mode & config option selections)
 * per agentType to localStorage, so they survive session restarts.
 *
 * Structure hash is stored alongside values — when the saved value no
 * longer exists in the current option set (item renamed / removed) the
 * backend's `set_session_config_option` will reject the application and
 * the stale value is naturally dropped on the next user pick.
 *
 * Preferences are shipped to the backend at `acp_connect` time (see
 * `getSavedPrefsForConnect`) which applies them to the agent BEFORE
 * the initial `session_modes` / `session_config_options` events are
 * emitted. Snapshots, replays, and live events therefore all carry the
 * user-preferred values uniformly — there is no client-side "intercept
 * incoming event and overwrite locally" path.
 */

import type { SessionModeStateInfo } from "@/lib/types"

const STORAGE_KEY = "codeg:selector-prefs"

interface SelectorPrefs {
  modeId?: string
  configValues?: Record<string, string>
}

type AllPrefs = Record<string, SelectorPrefs>

function readAll(): AllPrefs {
  if (typeof window === "undefined") return {}
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? (JSON.parse(raw) as AllPrefs) : {}
  } catch {
    return {}
  }
}

function writeAll(all: AllPrefs) {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(all))
  } catch {
    /* ignore */
  }
}

function updatePrefs(
  agentType: string,
  fn: (prefs: SelectorPrefs) => SelectorPrefs
) {
  const all = readAll()
  const existing = all[agentType]
  // Re-project onto the current schema so legacy fields (`modesHash` /
  // `configHash` from before the backend took ownership of preference
  // application) don't survive across writes. Without this an upgrade
  // user's first save would re-persist the stale hash bytes forever.
  const normalized: SelectorPrefs = {
    modeId: existing?.modeId,
    configValues: existing?.configValues,
  }
  all[agentType] = fn(normalized)
  writeAll(all)
}

// ── Read ──

/** Read saved mode id for an agent (no validation, just the raw value). */
export function getSavedModeId(agentType: string): string | null {
  const all = readAll()
  return all[agentType]?.modeId ?? null
}

/**
 * Read all saved preferences for an agent. Returned shape mirrors what
 * the backend `acp_connect` command accepts (`preferred_mode_id` +
 * `preferred_config_values`). Null/empty fields are normalized so the
 * call site can pass the result through unchanged.
 *
 * The backend applies these on the freshly-attached session before any
 * `session_modes` / `session_config_options` event is emitted, so the
 * frontend never needs to "intercept event and overwrite, then sync back".
 */
export function getSavedPrefsForConnect(agentType: string): {
  modeId: string | null
  configValues: Record<string, string> | null
} {
  const all = readAll()
  const prefs = all[agentType]
  if (!prefs) return { modeId: null, configValues: null }
  const configValues =
    prefs.configValues && Object.keys(prefs.configValues).length > 0
      ? prefs.configValues
      : null
  return {
    modeId: prefs.modeId ?? null,
    configValues,
  }
}

// ── Save (user actions only) ──

export function saveModePreference(
  agentType: string,
  modes: SessionModeStateInfo
) {
  updatePrefs(agentType, (prefs) => ({
    ...prefs,
    modeId: modes.current_mode_id,
  }))
}

export function saveConfigPreference(
  agentType: string,
  configId: string,
  valueId: string
) {
  updatePrefs(agentType, (prefs) => ({
    ...prefs,
    configValues: { ...prefs.configValues, [configId]: valueId },
  }))
}

/** Drop one stale config preference after the agent rejects it. */
export function clearConfigPreference(agentType: string, configId: string) {
  updatePrefs(agentType, (prefs) => {
    const configValues = { ...prefs.configValues }
    delete configValues[configId]
    return {
      ...prefs,
      configValues:
        Object.keys(configValues).length > 0 ? configValues : undefined,
    }
  })
}
