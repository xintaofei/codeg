type TerminalShortcutEvent = Pick<
  KeyboardEvent,
  "code" | "altKey" | "metaKey" | "ctrlKey" | "shiftKey"
>

export function isTerminalCopyShortcut(
  event: TerminalShortcutEvent,
  isMac: boolean
): boolean {
  return (
    !isMac &&
    event.code === "KeyC" &&
    event.ctrlKey &&
    event.shiftKey &&
    !event.altKey &&
    !event.metaKey
  )
}
