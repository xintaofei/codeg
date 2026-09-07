"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  type ReactNode,
} from "react"

type CloseSource = "manual" | "abandon"

type Entry = {
  key: string
  close: () => boolean | void
  /** Set when the browser back button (not the UI) dismissed this entry. */
  popped?: boolean
}

type HistoryContextValue = {
  open: (entry: Entry) => Entry
  close: (entry: Entry, source: CloseSource) => void
  attach: (entry: Entry) => Entry | null
}

const HistoryContext = createContext<HistoryContextValue | null>(null)
const STATE_KEY = "codegWorkspaceWindow"

function pushWindowState(key: string) {
  if (typeof window === "undefined") return
  // Spread the current state so entries pushed through Next.js' patched
  // pushState keep `__NA` and the internal router tree — without them the
  // app router reloads the page when it sees our entries on popstate.
  window.history.pushState(
    { ...(window.history.state ?? {}), [STATE_KEY]: key },
    "",
    window.location.href
  )
}

function stateKeyOf(state: unknown): string | null {
  if (!state || typeof state !== "object") return null
  const key = (state as Record<string, unknown>)[STATE_KEY]
  return typeof key === "string" ? key : null
}

/**
 * A LIFO stack of "windows" (drawers, in-memory routes, the file workspace)
 * mirrored into the browser history so the back button (mobile gesture,
 * hardware key, or desktop browser chrome) dismisses the topmost window
 * instead of leaving the workspace. Applies on every platform — web and
 * desktop client, any viewport.
 *
 * Every open window pushes one same-URL history entry tagged with its key.
 * The popstate handler decides what to do from the state it LANDS on:
 *
 * - a key that is still registered → the user backed over the windows above
 *   it, so close everything above that key;
 * - a key that is no longer registered → a phantom entry whose owner was
 *   unmounted without a matching back(); skip it silently with one more
 *   back() so a dead entry never swallows a back press;
 * - no key (the workspace base entry or another page) → close the topmost
 *   window. A close handler may veto by returning `false` (e.g. unsaved
 *   files), in which case the entry is re-pushed to undo the traversal.
 *
 * Manual closes call history.back() themselves; the traversals those cause
 * are swallowed via a counter rather than a boolean so rapid consecutive
 * closes cannot leak a synthetic popstate into the user path.
 */
export function WorkspaceWindowHistoryProvider({
  children,
}: {
  children: ReactNode
}) {
  const entriesRef = useRef<Entry[]>([])
  const byKeyRef = useRef(new Map<string, Entry>())
  const ignoreNextPopCountRef = useRef(0)

  const open = useCallback((entry: Entry) => {
    const existing = byKeyRef.current.get(entry.key)
    if (existing) {
      existing.close = entry.close
      return existing
    }
    entriesRef.current.push(entry)
    byKeyRef.current.set(entry.key, entry)
    pushWindowState(entry.key)
    return entry
  }, [])

  const close = useCallback((entry: Entry, source: CloseSource) => {
    if (!byKeyRef.current.has(entry.key)) return
    byKeyRef.current.delete(entry.key)
    entriesRef.current = entriesRef.current.filter(
      (candidate) => candidate.key !== entry.key
    )
    if (source === "manual") {
      // Defer the traversal and re-check before backing out: a window
      // opening in the same commit (e.g. tapping a file in the aux drawer
      // closes the drawer and opens the file workspace together) pushes
      // its own entry in a later effect of the same flush. history.back()
      // is asynchronous — issued now, the traversal would run AFTER that
      // push and land below the new entry, leaving the browser pointer
      // misaligned with the live stack (the next real back press would
      // then leave the page instead of closing that window). When this
      // entry is no longer the current one, skip back(); its history
      // entry becomes a phantom the popstate handler skips.
      const key = entry.key
      setTimeout(() => {
        if (stateKeyOf(window.history.state) !== key) return
        ignoreNextPopCountRef.current += 1
        window.history.back()
      }, 0)
    }
    // "abandon" (owner unmounted) only cleans the memory stack. The browser
    // entry becomes a phantom and is skipped by the popstate handler.
  }, [])

  const attach = useCallback((entry: Entry) => {
    const existing = byKeyRef.current.get(entry.key)
    if (!existing) return null
    existing.close = entry.close
    return existing
  }, [])

  useEffect(() => {
    const onPopState = (event: PopStateEvent) => {
      if (ignoreNextPopCountRef.current > 0) {
        ignoreNextPopCountRef.current -= 1
        return
      }
      const landedKey = stateKeyOf(event.state)
      if (landedKey !== null) {
        if (!byKeyRef.current.has(landedKey)) {
          // Phantom: the window owning this entry is gone. Skip the dead
          // entry so back still does something visible.
          window.history.back()
          return
        }
        // Close every window above the one we landed on.
        while (entriesRef.current.length > 0) {
          const top = entriesRef.current[entriesRef.current.length - 1]
          if (top.key === landedKey) break
          const closed = top.close()
          if (closed === false) {
            pushWindowState(top.key)
            break
          }
          top.popped = true
          entriesRef.current.pop()
          byKeyRef.current.delete(top.key)
        }
        return
      }
      // Landed on the workspace base entry or a page outside the workspace.
      const entry = entriesRef.current[entriesRef.current.length - 1]
      if (!entry) return
      const closed = entry.close()
      if (closed === false) {
        pushWindowState(entry.key)
        return
      }
      entry.popped = true
      entriesRef.current.pop()
      byKeyRef.current.delete(entry.key)
    }
    window.addEventListener("popstate", onPopState)
    return () => window.removeEventListener("popstate", onPopState)
  }, [])

  const value = useMemo(
    () => ({ open, close, attach }),
    [attach, close, open]
  )
  return (
    <HistoryContext.Provider value={value}>
      {children}
    </HistoryContext.Provider>
  )
}

export function useBrowserBackWindow({
  open,
  onClose,
  key,
}: {
  open: boolean
  onClose: () => boolean | void
  key?: string
}) {
  const context = useContext(HistoryContext)
  const autoKey = useId()
  const entryKey = key ?? autoKey
  const closeRef = useRef(onClose)
  const entryRef = useRef<Entry | null>(null)

  useEffect(() => {
    closeRef.current = onClose
  }, [onClose])

  // Register while open. Runs on every onClose change too, but `attach`
  // finds the existing entry then and only refreshes its close callback —
  // it never pushes a second history entry. Re-registering here also
  // recovers the entry after a StrictMode simulated remount, which the
  // abandon cleanup below removes.
  useEffect(() => {
    if (!context || !open) return
    // The browser already dismissed this window; the parent just hasn't
    // applied the closed state yet. Re-registering here would resurrect
    // its history entry.
    if (entryRef.current?.popped) return
    const candidate: Entry = {
      key: entryKey,
      close: () => closeRef.current(),
    }
    entryRef.current = context.attach(candidate) ?? context.open(candidate)
  }, [context, entryKey, onClose, open])

  // Closed through the UI: consume the matching history entry.
  useEffect(() => {
    if (!context || open || !entryRef.current) return
    const entry = entryRef.current
    entryRef.current = null
    context.close(entry, "manual")
  }, [context, open])

  // Unmounted while open: drop the memory entry. Its history entry stays
  // behind as a phantom and is skipped on the next back press.
  useEffect(() => {
    return () => {
      if (entryRef.current && context) {
        context.close(entryRef.current, "abandon")
      }
    }
  }, [context])
}
