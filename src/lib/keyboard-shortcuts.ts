export type ShortcutActionId =
  | "toggle_search"
  | "toggle_sidebar"
  | "toggle_terminal"
  | "new_terminal_tab"
  | "close_current_terminal_tab"
  | "toggle_aux_panel"
  | "new_conversation"
  | "open_folder"
  | "open_settings"
  | "close_current_tab"
  | "close_all_file_tabs"
  | "next_tab"
  | "prev_tab"
  | "send_message"
  | "newline_in_message"

export interface ShortcutDefinition {
  id: ShortcutActionId
}

export const SHORTCUT_DEFINITIONS: ShortcutDefinition[] = [
  {
    id: "toggle_search",
  },
  {
    id: "toggle_sidebar",
  },
  {
    id: "toggle_terminal",
  },
  {
    id: "new_terminal_tab",
  },
  {
    id: "close_current_terminal_tab",
  },
  {
    id: "toggle_aux_panel",
  },
  {
    id: "new_conversation",
  },
  {
    id: "open_folder",
  },
  {
    id: "open_settings",
  },
  {
    id: "close_current_tab",
  },
  {
    id: "close_all_file_tabs",
  },
  {
    id: "next_tab",
  },
  {
    id: "prev_tab",
  },
  {
    id: "send_message",
  },
  {
    id: "newline_in_message",
  },
]

/** Actions that allow shortcuts without modifier keys (e.g. plain Enter). */
export const INPUT_SHORTCUT_IDS = new Set<ShortcutActionId>([
  "send_message",
  "newline_in_message",
])

export type ShortcutSettings = Record<ShortcutActionId, string>

export const DEFAULT_SHORTCUTS: ShortcutSettings = {
  toggle_search: "mod+k",
  toggle_sidebar: "mod+b",
  toggle_terminal: "mod+j",
  new_terminal_tab: "mod+t",
  close_current_terminal_tab: "mod+w",
  toggle_aux_panel: "mod+e",
  new_conversation: "mod+t",
  open_folder: "mod+o",
  open_settings: "mod+,",
  close_current_tab: "mod+w",
  close_all_file_tabs: "mod+shift+w",
  next_tab: "mod+tab",
  prev_tab: "mod+shift+tab",
  send_message: "enter",
  newline_in_message: "shift+enter",
}

export const SHORTCUTS_STORAGE_KEY = "settings:shortcuts:v1"
export const SHORTCUTS_UPDATED_EVENT = "codeg:shortcuts-updated"

const FUNCTION_KEY_PATTERN = /^f\d{1,2}$/
const MODIFIER_KEY_SET = new Set(["shift", "meta", "control", "alt"])

const SPECIAL_KEY_ALIASES: Record<string, string> = {
  " ": "space",
  spacebar: "space",
  esc: "escape",
  return: "enter",
  up: "arrowup",
  down: "arrowdown",
  left: "arrowleft",
  right: "arrowright",
}

const KEY_LABELS: Record<string, string> = {
  space: "Space",
  escape: "Esc",
  enter: "Enter",
  tab: "Tab",
  arrowup: "Up",
  arrowdown: "Down",
  arrowleft: "Left",
  arrowright: "Right",
  backspace: "Backspace",
  delete: "Delete",
}

function normalizeKeyToken(rawKey: string): string | null {
  const key = rawKey.toLowerCase()
  if (!key) return null

  if (key.length === 1) return key
  if (FUNCTION_KEY_PATTERN.test(key)) return key

  const aliased = SPECIAL_KEY_ALIASES[key] ?? key
  if (aliased.length === 1 || FUNCTION_KEY_PATTERN.test(aliased)) return aliased

  if (
    aliased === "space" ||
    aliased === "escape" ||
    aliased === "enter" ||
    aliased === "tab" ||
    aliased === "backspace" ||
    aliased === "delete" ||
    aliased === "arrowup" ||
    aliased === "arrowdown" ||
    aliased === "arrowleft" ||
    aliased === "arrowright"
  ) {
    return aliased
  }

  return null
}

function normalizeSettings(input: unknown): ShortcutSettings {
  const next: ShortcutSettings = { ...DEFAULT_SHORTCUTS }
  if (!input || typeof input !== "object") return next

  const record = input as Record<string, unknown>
  for (const definition of SHORTCUT_DEFINITIONS) {
    const rawValue = record[definition.id]
    if (typeof rawValue !== "string") continue

    const normalized = normalizeShortcut(rawValue)
    if (normalized) next[definition.id] = normalized
  }

  return next
}

export function normalizeShortcut(rawShortcut: string): string | null {
  const parts = rawShortcut
    .toLowerCase()
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)

  if (parts.length === 0) return null

  let mod = false
  let alt = false
  let shift = false
  let keyToken: string | null = null

  for (const part of parts) {
    if (
      part === "mod" ||
      part === "cmd" ||
      part === "command" ||
      part === "meta" ||
      part === "ctrl" ||
      part === "control"
    ) {
      mod = true
      continue
    }

    if (part === "alt" || part === "option") {
      alt = true
      continue
    }

    if (part === "shift") {
      shift = true
      continue
    }

    if (keyToken) return null

    const normalizedKey = normalizeKeyToken(part)
    if (!normalizedKey || MODIFIER_KEY_SET.has(normalizedKey)) return null

    keyToken = normalizedKey
  }

  if (!keyToken) return null

  const normalizedParts: string[] = []
  if (mod) normalizedParts.push("mod")
  if (alt) normalizedParts.push("alt")
  if (shift) normalizedParts.push("shift")
  normalizedParts.push(keyToken)

  return normalizedParts.join("+")
}

export function readShortcutSettings(): ShortcutSettings {
  if (typeof window === "undefined") return { ...DEFAULT_SHORTCUTS }

  try {
    const raw = window.localStorage.getItem(SHORTCUTS_STORAGE_KEY)
    if (!raw) return { ...DEFAULT_SHORTCUTS }
    const parsed: unknown = JSON.parse(raw)
    return normalizeSettings(parsed)
  } catch {
    return { ...DEFAULT_SHORTCUTS }
  }
}

export function writeShortcutSettings(settings: ShortcutSettings): void {
  if (typeof window === "undefined") return

  const normalized = normalizeSettings(settings)

  try {
    window.localStorage.setItem(
      SHORTCUTS_STORAGE_KEY,
      JSON.stringify(normalized)
    )
    window.dispatchEvent(new Event(SHORTCUTS_UPDATED_EVENT))
  } catch {
    // Ignore storage failures so UI shortcuts still work in memory.
  }
}

export function shortcutFromKeyboardEvent(
  event: Pick<
    KeyboardEvent,
    "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"
  >,
  /** When true, allow shortcuts without modifier keys (e.g. plain Enter). */
  allowNoModifier = false
): string | null {
  const keyToken = normalizeKeyToken(event.key)
  if (!keyToken || MODIFIER_KEY_SET.has(keyToken)) return null

  if (!allowNoModifier && !event.metaKey && !event.ctrlKey && !event.altKey) {
    return null
  }

  const parts: string[] = []
  if (event.metaKey || event.ctrlKey) parts.push("mod")
  if (event.altKey) parts.push("alt")
  if (event.shiftKey) parts.push("shift")
  parts.push(keyToken)

  return parts.join("+")
}

export function matchShortcutEvent(
  event: Pick<
    KeyboardEvent,
    "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"
  >,
  shortcut: string
): boolean {
  const normalized = normalizeShortcut(shortcut)
  if (!normalized) return false

  const parts = normalized.split("+")
  const keyToken = parts[parts.length - 1]
  const needsMod = parts.includes("mod")
  const needsAlt = parts.includes("alt")
  const needsShift = parts.includes("shift")

  const actualKey = normalizeKeyToken(event.key)
  if (!actualKey) return false
  if (actualKey !== keyToken) return false

  const hasMod = event.metaKey || event.ctrlKey
  if (hasMod !== needsMod) return false
  if (event.altKey !== needsAlt) return false
  if (event.shiftKey !== needsShift) return false

  return true
}

function toKeyLabel(keyToken: string): string {
  const common = KEY_LABELS[keyToken]
  if (common) return common

  if (keyToken.length === 1) return keyToken.toUpperCase()
  if (FUNCTION_KEY_PATTERN.test(keyToken)) return keyToken.toUpperCase()

  return keyToken
}

export function formatShortcutLabel(shortcut: string, isMac: boolean): string {
  const normalized = normalizeShortcut(shortcut)
  if (!normalized) return shortcut

  const parts = normalized.split("+")
  const keyToken = parts[parts.length - 1]

  const modifiers: string[] = []
  if (parts.includes("mod")) modifiers.push(isMac ? "⌘" : "Ctrl")
  if (parts.includes("alt")) modifiers.push(isMac ? "⌥" : "Alt")
  if (parts.includes("shift")) modifiers.push(isMac ? "⇧" : "Shift")

  const keyLabel = toKeyLabel(keyToken)

  if (isMac) {
    return `${modifiers.join("")}${keyLabel}`
  }

  if (modifiers.length === 0) return keyLabel
  return `${modifiers.join("+")}+${keyLabel}`
}
