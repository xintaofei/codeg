"use client"

/**
 * Notification-sound preferences: which audible cue (if any) each agent event
 * plays, plus the master switch, volume and focus gate.
 *
 * The event catalogue deliberately mirrors the chat-channel Events tab
 * (`channel-events-tab.tsx` / `chat_channel/event_subscriber.rs`), so the same
 * five triggers a user already knows from IM/webhook pushes also drive the
 * local sound. Only the sink differs: a channel push leaves the machine, a
 * sound does not.
 *
 * Stored in localStorage rather than the backend because audio output is a
 * per-device concern — a phone browser attached to the same server should not
 * start beeping because the desktop was configured to. Same reactive shape as
 * `office-preview-prefs.ts`: a custom event for the current window plus the
 * native `storage` event for other windows/tabs, so flipping a switch in the
 * Settings window takes effect in the workspace immediately.
 */

import { useSyncExternalStore } from "react"

const PREFS_KEY = "settings:notification-sound:v1"
const PREFS_EVENT = "codeg:notification-sound-changed"

/** Events that can trigger a sound, in display order. */
export const SOUND_EVENT_IDS = [
  "turn_complete",
  "permission_request",
  "question_request",
  "error",
  "user_prompt_sent",
] as const

export type SoundEventId = (typeof SOUND_EVENT_IDS)[number]

/** Built-in tones, in display order. `none` means "this event is silent". */
export const SOUND_TONE_IDS = [
  "none",
  "chime",
  "ding",
  "blip",
  "pop",
  "alert",
  "descend",
] as const

export type SoundToneId = (typeof SOUND_TONE_IDS)[number]

export interface NotificationSoundPrefs {
  /** Master switch for visual system notifications. */
  systemNotificationsEnabled: boolean
  /** Only show system notifications while the workspace is unfocused. */
  systemNotificationsOnlyWhenUnfocused: boolean
  /** Master switch. Off until the user opts in — see DEFAULTS below. */
  enabled: boolean
  /** 0..1, applied as the peak gain of every tone. */
  volume: number
  /** Only play while the app window is unfocused (a background-only cue). */
  onlyWhenUnfocused: boolean
  /** Tone per event; `none` silences that event without touching the rest. */
  tones: Record<SoundEventId, SoundToneId>
}

/**
 * The master switch starts OFF: an upgrade must not make an install that never
 * asked for audio suddenly start beeping. The per-event tones below are still
 * pre-filled, so switching it on gives a usable set immediately instead of
 * silence the user has to go configure.
 *
 * `user_prompt_sent` defaults to `none` because it fires on the user's own
 * action — an echo of a message you just sent is the least useful cue of the
 * five. (Unrelated to the backend's default-off rule for the same event, which
 * is about not exporting prompt text off the machine.)
 */
export const DEFAULT_NOTIFICATION_SOUND_PREFS: NotificationSoundPrefs = {
  systemNotificationsEnabled: true,
  systemNotificationsOnlyWhenUnfocused: true,
  enabled: false,
  volume: 0.6,
  onlyWhenUnfocused: false,
  tones: {
    turn_complete: "chime",
    permission_request: "alert",
    question_request: "ding",
    error: "descend",
    user_prompt_sent: "none",
  },
}

function isSoundToneId(value: unknown): value is SoundToneId {
  return (
    typeof value === "string" &&
    (SOUND_TONE_IDS as readonly string[]).includes(value)
  )
}

/**
 * Merge a stored blob over the defaults, field by field. Every field is
 * validated independently so a partial write from an older build (or a
 * hand-edited value) degrades to the default for that one field instead of
 * discarding the whole preference set.
 */
export function parseNotificationSoundPrefs(
  raw: unknown
): NotificationSoundPrefs {
  const defaults = DEFAULT_NOTIFICATION_SOUND_PREFS
  if (!raw || typeof raw !== "object") {
    return { ...defaults, tones: { ...defaults.tones } }
  }
  const source = raw as Record<string, unknown>

  const tones = { ...defaults.tones }
  const storedTones = source.tones
  if (storedTones && typeof storedTones === "object") {
    const entries = storedTones as Record<string, unknown>
    for (const id of SOUND_EVENT_IDS) {
      const tone = entries[id]
      if (isSoundToneId(tone)) tones[id] = tone
    }
  }

  return {
    systemNotificationsEnabled:
      typeof source.systemNotificationsEnabled === "boolean"
        ? source.systemNotificationsEnabled
        : defaults.systemNotificationsEnabled,
    systemNotificationsOnlyWhenUnfocused:
      typeof source.systemNotificationsOnlyWhenUnfocused === "boolean"
        ? source.systemNotificationsOnlyWhenUnfocused
        : defaults.systemNotificationsOnlyWhenUnfocused,
    enabled:
      typeof source.enabled === "boolean" ? source.enabled : defaults.enabled,
    // Clamp rather than reject: a slider that once wrote 1.2 should land on
    // full volume, not silently reset to the default.
    volume:
      typeof source.volume === "number" && Number.isFinite(source.volume)
        ? Math.min(1, Math.max(0, source.volume))
        : defaults.volume,
    onlyWhenUnfocused:
      typeof source.onlyWhenUnfocused === "boolean"
        ? source.onlyWhenUnfocused
        : defaults.onlyWhenUnfocused,
    tones,
  }
}

export function loadNotificationSoundPrefs(): NotificationSoundPrefs {
  const defaults = DEFAULT_NOTIFICATION_SOUND_PREFS
  if (typeof window === "undefined") {
    return { ...defaults, tones: { ...defaults.tones } }
  }
  try {
    const raw = localStorage.getItem(PREFS_KEY)
    if (!raw) return { ...defaults, tones: { ...defaults.tones } }
    return parseNotificationSoundPrefs(JSON.parse(raw))
  } catch {
    return { ...defaults, tones: { ...defaults.tones } }
  }
}

export function saveNotificationSoundPrefs(
  prefs: NotificationSoundPrefs
): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(prefs))
  } catch {
    /* ignore */
  }
  // Same-window listeners (the player lives in the workspace window, which may
  // also host the settings route); other windows/tabs get the native `storage`
  // event.
  window.dispatchEvent(new CustomEvent(PREFS_EVENT, { detail: prefs }))
}

// ── Shared snapshot ──
//
// One memoized copy per window, invalidated by a write here or in another
// window/tab. Both consumers read through it: the settings panel (via
// `useSyncExternalStore`, which needs a stable identity between renders) and
// the player, which sits on the ACP event path and must not re-parse JSON per
// event.

let snapshot: NotificationSoundPrefs | null = null
const listeners = new Set<() => void>()
let windowBound = false

function bindWindow(): void {
  if (windowBound || typeof window === "undefined") return
  windowBound = true
  const invalidate = () => {
    snapshot = null
    for (const listener of listeners) listener()
  }
  window.addEventListener(PREFS_EVENT, invalidate)
  window.addEventListener("storage", invalidate)
}

/**
 * Current preferences, memoized. Identity only changes when the stored value
 * does, so it is safe as a `useSyncExternalStore` snapshot.
 */
export function getNotificationSoundPrefs(): NotificationSoundPrefs {
  // Reading is enough to start tracking changes — a window that only plays
  // sounds never subscribes, and would otherwise hold a stale copy forever
  // after the settings window wrote a new one.
  bindWindow()
  if (typeof window === "undefined") return DEFAULT_NOTIFICATION_SOUND_PREFS
  snapshot ??= loadNotificationSoundPrefs()
  return snapshot
}

/** Subscribe to preference changes from this window or any other. */
export function subscribeNotificationSoundPrefs(
  onChange: () => void
): () => void {
  bindWindow()
  listeners.add(onChange)
  return () => {
    listeners.delete(onChange)
  }
}

/**
 * Stable pre-hydration value. The exported HTML is built without a browser, so
 * React renders the defaults on the server pass and swaps in the stored
 * preferences once hydrated.
 */
function getServerNotificationSoundPrefs(): NotificationSoundPrefs {
  return DEFAULT_NOTIFICATION_SOUND_PREFS
}

/** Reactive read of the preferences; live across windows. */
export function useNotificationSoundPrefs(): NotificationSoundPrefs {
  return useSyncExternalStore(
    subscribeNotificationSoundPrefs,
    getNotificationSoundPrefs,
    getServerNotificationSoundPrefs
  )
}

/** Test seam: forget the memoized snapshot so the next read hits storage. */
export function resetNotificationSoundPrefsCacheForTests(): void {
  snapshot = null
}
