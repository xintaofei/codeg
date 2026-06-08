import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * Legacy clipboard copy: a hidden `<textarea>` + `document.execCommand("copy")`.
 * Works in non-secure contexts (HTTP over LAN) where the async Clipboard API is
 * unavailable. Returns whether the copy succeeded.
 */
function legacyCopyText(text: string): boolean {
  try {
    const textarea = document.createElement("textarea")
    textarea.value = text
    textarea.setAttribute("readonly", "")
    textarea.setAttribute("aria-hidden", "true")
    textarea.style.position = "absolute"
    textarea.style.left = "-9999px"
    textarea.style.top = `${window.pageYOffset || document.documentElement.scrollTop}px`
    textarea.style.opacity = "0"
    textarea.style.pointerEvents = "none"
    const selectionApi = document.getSelection()
    const previousRange =
      selectionApi && selectionApi.rangeCount > 0
        ? selectionApi.getRangeAt(0).cloneRange()
        : null
    document.body.appendChild(textarea)
    textarea.focus({ preventScroll: true })
    textarea.select()
    textarea.setSelectionRange(0, text.length)
    let ok = false
    try {
      ok = document.execCommand("copy")
    } catch {
      ok = false
    }
    textarea.remove()
    if (previousRange) {
      try {
        const selection = document.getSelection()
        selection?.removeAllRanges()
        selection?.addRange(previousRange)
      } catch {
        // restoring the prior page selection is best-effort
      }
    }
    return ok
  } catch {
    return false
  }
}

/**
 * Write text to the clipboard. Falls back to `legacyCopyText` when the async
 * Clipboard API is unavailable (non-secure contexts such as HTTP over LAN).
 */
export async function copyTextToClipboard(text: string): Promise<boolean> {
  if (typeof window === "undefined") return false
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // fall through to legacy path
    }
  }
  return legacyCopyText(text)
}

/**
 * Copy text to the clipboard from inside a Radix menu item
 * (`ContextMenuItem` / `DropdownMenuItem`).
 *
 * Radix menus trap focus while open. `copyTextToClipboard`'s fallback (hidden
 * textarea + `execCommand`, used in non-secure contexts such as the web build
 * served over HTTP/LAN where `navigator.clipboard` is unavailable) must focus
 * that textarea, which the focus trap steals back — so a copy fired straight
 * from `onSelect`/`onClick` silently fails in the web build. Deferring to the
 * next tick lets the menu close and release focus first, then the write
 * succeeds. Plain buttons aren't focus-trapped — call `copyTextToClipboard`
 * directly there. Resolves once the deferred write completes.
 */
export function copyTextFromMenu(text: string): Promise<boolean> {
  return new Promise((resolve) => {
    setTimeout(() => {
      void copyTextToClipboard(text).then(resolve)
    }, 0)
  })
}

/**
 * Install a `navigator.clipboard.writeText` fallback for non-secure contexts.
 *
 * The web build served over plain HTTP/LAN is not a secure context, so
 * `navigator.clipboard` is undefined. Third-party components that call
 * `navigator.clipboard.writeText` directly with no fallback of their own —
 * notably Streamdown's code-block copy button (`data-streamdown="code-block"`)
 * and its link-safety dialog — then silently fail. Backing `writeText` with the
 * legacy `execCommand` path makes those copies work. Idempotent, and a no-op
 * when the native async Clipboard API is present (secure contexts, desktop
 * Tauri) or when run off the client. Call once on client startup.
 */
export function installClipboardFallback(): void {
  if (typeof navigator === "undefined" || typeof document === "undefined") {
    return
  }
  // navigator.clipboard is undefined at runtime in non-secure contexts even
  // though the DOM types claim it is always present, so guard with typeof.
  if (typeof navigator.clipboard?.writeText === "function") return

  const writeText = (text: string): Promise<void> =>
    legacyCopyText(text)
      ? Promise.resolve()
      : Promise.reject(new Error("Copy command failed"))

  // A clipboard object exists but lacks writeText: augment it in place so any
  // other methods it carries (e.g. readText) are preserved.
  if (navigator.clipboard) {
    try {
      Object.defineProperty(navigator.clipboard, "writeText", {
        configurable: true,
        value: writeText,
      })
      return
    } catch {
      // fall through to defining a fresh clipboard object
    }
  }

  try {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    })
  } catch {
    // navigator.clipboard is a non-configurable getter here; nothing we can do.
  }
}

/**
 * Generate a UUID v4. Uses `crypto.randomUUID()` when available (secure
 * contexts), otherwise falls back to `crypto.getRandomValues()`.
 */
export function randomUUID(): string {
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID()
  }
  // Fallback for non-secure contexts (HTTP over LAN)
  const bytes = new Uint8Array(16)
  crypto.getRandomValues(bytes)
  // Set version 4 and variant bits
  bytes[6] = (bytes[6] & 0x0f) | 0x40
  bytes[8] = (bytes[8] & 0x3f) | 0x80
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}
