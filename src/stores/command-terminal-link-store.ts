import { create } from "zustand"

/**
 * The command launcher (`CommandDropdown`) lives inside the aux panel's Session
 * Details tab, which the shell unmounts whenever the right sidebar is closed.
 * The command↔terminal linkage that drives the launcher's Run/Stop state used
 * to be local component state, so it was destroyed on that unmount and never
 * reconstructed — reopening the sidebar showed "Run" for a still-running
 * command. Holding the map at module scope here keeps the linkage alive across
 * that unmount; the terminals themselves already survive (their
 * `TerminalProvider` is a top-level provider).
 */
interface CommandTerminalLinkState {
  /**
   * Maps a folder command's id to the terminal it was launched into. Folder
   * command ids are globally-unique DB primary keys, so a flat map is safe
   * across folders.
   */
  links: Record<number, string>
  setLink: (commandId: number, terminalId: string) => void
  clearLink: (commandId: number) => void
  /** Drop every link whose terminal id matches the predicate (exited/closed). */
  pruneTerminals: (shouldRemove: (terminalId: string) => boolean) => void
}

export const useCommandTerminalLinkStore = create<CommandTerminalLinkState>(
  (set) => ({
    links: {},
    setLink: (commandId, terminalId) =>
      set((s) =>
        s.links[commandId] === terminalId
          ? s
          : { links: { ...s.links, [commandId]: terminalId } }
      ),
    clearLink: (commandId) =>
      set((s) => {
        if (!(commandId in s.links)) return s
        const next = { ...s.links }
        delete next[commandId]
        return { links: next }
      }),
    pruneTerminals: (shouldRemove) =>
      set((s) => {
        let changed = false
        const next: Record<number, string> = {}
        for (const [cmdId, termId] of Object.entries(s.links)) {
          if (shouldRemove(termId)) changed = true
          else next[Number(cmdId)] = termId
        }
        return changed ? { links: next } : s
      }),
  })
)

/**
 * The terminal a command is currently linked to *and* running in, or
 * `undefined`. A persisted link is only "live" when its terminal still exists
 * and hasn't exited — so a stale link left over from a terminal that ended
 * while the panel was closed reads as not-running (no false "Stop" state).
 * Pure + exported for unit testing.
 */
export function resolveLiveCommandTerminalId(
  links: Record<number, string>,
  commandId: number,
  isLive: (terminalId: string) => boolean
): string | undefined {
  const terminalId = links[commandId]
  return terminalId && isLive(terminalId) ? terminalId : undefined
}
