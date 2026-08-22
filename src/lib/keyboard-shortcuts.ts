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
  | "toggle_custom_style"
  | "zoom_in"
  | "zoom_out"
  | "zoom_reset"

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
  {
    id: "toggle_custom_style",
  },
  {
    id: "zoom_in",
  },
  {
    id: "zoom_out",
  },
  {
    id: "zoom_reset",
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
  // 自定义样式的逃生舱：用户把界面改到不可用时，这一路必须仍然按得动，所以选一个
  // 三修饰键组合（不会与任何常用操作撞车），并在捕获阶段监听。
  toggle_custom_style: "mod+alt+shift+s",
  // Same rungs as Settings → Window zoom. `=` is what US keyboards fire for
  // Ctrl/+ without Shift; `+` is Shift+= and the numpad.
  zoom_in: "mod+=",
  zoom_out: "mod+-",
  zoom_reset: "mod+0",
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

/**
 * `=`/`+` and `-`/`_` are the same physical key (unshifted vs shifted).
 * Bindings on one should also fire for the other; extra Shift is ignored
 * only for these pairs, because Shift is how you type the sibling.
 */
const PHYSICAL_KEY_SIBLINGS: Record<string, string> = {
  "=": "+",
  "+": "=",
  "-": "_",
  _: "-",
}

export interface ParsedShortcut {
  mod: boolean
  alt: boolean
  shift: boolean
  key: string
}

/**
 * Recording a shortcut in Settings and the global zoom listener are both
 * capture handlers on `window`. `stopPropagation()` does not stop a sibling
 * listener on the same target, so the recorder arms this flag instead.
 */
let shortcutRecorderArmed = false

export function setShortcutRecorderArmed(armed: boolean): void {
  shortcutRecorderArmed = armed
}

export function isShortcutRecorderArmed(): boolean {
  return shortcutRecorderArmed
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

/**
 * Split a shortcut string without treating a trailing `+` key as a delimiter.
 * `mod++` and `mod+shift++` are how Ctrl/Cmd+Shift+= serializes.
 */
export function parseShortcut(rawShortcut: string): ParsedShortcut | null {
  const lowered = rawShortcut.toLowerCase().trim()
  if (!lowered) return null

  let keyRaw: string
  let prefix: string
  if (lowered === "+" || lowered.endsWith("++")) {
    keyRaw = "+"
    prefix = lowered === "+" ? "" : lowered.slice(0, -2)
  } else {
    const lastPlus = lowered.lastIndexOf("+")
    if (lastPlus === -1) {
      keyRaw = lowered
      prefix = ""
    } else {
      keyRaw = lowered.slice(lastPlus + 1)
      prefix = lowered.slice(0, lastPlus)
    }
  }

  const keyToken = normalizeKeyToken(keyRaw.trim())
  if (!keyToken || MODIFIER_KEY_SET.has(keyToken)) return null

  let mod = false
  let alt = false
  let shift = false
  if (prefix) {
    const parts = prefix
      .split("+")
      .map((part) => part.trim())
      .filter(Boolean)
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
      return null
    }
  }

  return { mod, alt, shift, key: keyToken }
}

export function normalizeShortcut(rawShortcut: string): string | null {
  const parsed = parseShortcut(rawShortcut)
  if (!parsed) return null

  const normalizedParts: string[] = []
  if (parsed.mod) normalizedParts.push("mod")
  if (parsed.alt) normalizedParts.push("alt")
  if (parsed.shift) normalizedParts.push("shift")
  normalizedParts.push(parsed.key)
  return normalizedParts.join("+")
}

/**
 * 键盘事件里我们真正读的那几个字段。`code` 是可选的，因为部分调用点（含测试）
 * 构造的是精简对象。
 */
type ShortcutEventLike = Pick<
  KeyboardEvent,
  "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"
> & { code?: string }

/**
 * Alt/Option 会改写 `event.key`：macOS 上 ⌥S 报的是 "ß" 而不是 "s"（其它布局同理）。
 * 按住 Alt 时改用与键盘布局无关的 `event.code`，否则**任何含 alt 的组合在 macOS 上
 * 都按不出来**，录制时记下的也会是那个变音字符。
 *
 * 只在 altKey 为真时改道，所以不含 alt 的既有快捷键行为完全不变。
 */
function eventKeyToken(event: ShortcutEventLike): string | null {
  if (event.altKey && typeof event.code === "string") {
    const letter = /^Key([A-Z])$/.exec(event.code)
    if (letter) return letter[1].toLowerCase()
    const digit = /^Digit([0-9])$/.exec(event.code)
    if (digit) return digit[1]
  }
  return normalizeKeyToken(event.key)
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
  event: ShortcutEventLike,
  /** When true, allow shortcuts without modifier keys (e.g. plain Enter). */
  allowNoModifier = false
): string | null {
  const keyToken = eventKeyToken(event)
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

function siblingKeys(keyToken: string): Set<string> {
  const sibling = PHYSICAL_KEY_SIBLINGS[keyToken]
  return sibling ? new Set([keyToken, sibling]) : new Set([keyToken])
}

function matchesNumpadCode(
  event: ShortcutEventLike,
  boundKey: string
): boolean {
  if (boundKey === "=" || boundKey === "+") {
    return event.code === "NumpadAdd"
  }
  if (boundKey === "-" || boundKey === "_") {
    return event.code === "NumpadSubtract"
  }
  return false
}

export function matchShortcutEvent(
  event: ShortcutEventLike,
  shortcut: string
): boolean {
  const parsed = parseShortcut(shortcut)
  if (!parsed) return false

  const keys = siblingKeys(parsed.key)
  const actualKey = eventKeyToken(event)
  const matchesKey = actualKey !== null && keys.has(actualKey)
  if (!matchesKey && !matchesNumpadCode(event, parsed.key)) return false

  const hasMod = event.metaKey || event.ctrlKey
  if (hasMod !== parsed.mod) return false
  if (event.altKey !== parsed.alt) return false

  // Extra Shift is how `=` becomes `+` (and `-` becomes `_`). Require Shift
  // when the binding asked for it; ignore a surplus Shift only on those pairs.
  if (parsed.shift) {
    if (!event.shiftKey) return false
  } else if (event.shiftKey && keys.size === 1) {
    return false
  }

  return true
}

export function resolveWindowZoomAction(
  event: ShortcutEventLike,
  shortcuts: Pick<ShortcutSettings, "zoom_in" | "zoom_out" | "zoom_reset">
): "in" | "out" | "reset" | null {
  if (matchShortcutEvent(event, shortcuts.zoom_in)) return "in"
  if (matchShortcutEvent(event, shortcuts.zoom_out)) return "out"
  if (matchShortcutEvent(event, shortcuts.zoom_reset)) return "reset"
  return null
}

function toKeyLabel(keyToken: string): string {
  const common = KEY_LABELS[keyToken]
  if (common) return common

  if (keyToken.length === 1) return keyToken.toUpperCase()
  if (FUNCTION_KEY_PATTERN.test(keyToken)) return keyToken.toUpperCase()

  return keyToken
}

export function formatShortcutLabel(shortcut: string, isMac: boolean): string {
  const parsed = parseShortcut(shortcut)
  if (!parsed) return shortcut

  const modifiers: string[] = []
  if (parsed.mod) modifiers.push(isMac ? "⌘" : "Ctrl")
  if (parsed.alt) modifiers.push(isMac ? "⌥" : "Alt")
  if (parsed.shift) modifiers.push(isMac ? "⇧" : "Shift")

  const keyLabel = toKeyLabel(parsed.key)

  if (isMac) {
    return `${modifiers.join("")}${keyLabel}`
  }

  if (modifiers.length === 0) return keyLabel
  return `${modifiers.join("+")}+${keyLabel}`
}
