"use client"

import { useState, type KeyboardEvent } from "react"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import { browserSetVisible } from "@/lib/browser/browser-api"
import {
  useBrowserFindRequest,
  useBrowserTabState,
} from "@/lib/browser/browser-tab-store"
import { browserTabBackendId } from "@/lib/file-tab-id"

import { BrowserFindBar } from "./browser-find-bar"
import {
  BrowserDownloadBar,
  BrowserErrorPage,
  BrowserNoticeBar,
  BrowserOwnedWindowCard,
} from "./browser-status-layer"
import { BrowserSurfaceHost } from "./browser-surface-host"
import { BrowserToolbar } from "./browser-toolbar"

/**
 * The file-pane content of a browser tab: toolbar, notices, and the native
 * surface (or, when the page could not load, a DOM error page in its place).
 */
export function BrowserTabView({ tab }: { tab: BrowserWorkspaceTab }) {
  const state = useBrowserTabState(tab.id)
  const backendId = browserTabBackendId(tab.id)
  const [findOpen, setFindOpen] = useState(false)
  // ⌘F pressed while the PAGE had keyboard focus: the app's DOM never sees
  // that keystroke, so the host relays it and the store counts it. Read as a
  // counter, not a flag, so pressing it again on an open bar still counts —
  // and applied during render (not in an effect) so the bar is there in the
  // same paint.
  const findRequest = useBrowserFindRequest(tab.id)
  const [seenFindRequest, setSeenFindRequest] = useState(findRequest)
  if (seenFindRequest !== findRequest) {
    setSeenFindRequest(findRequest)
    setFindOpen(true)
  }

  // ⌘F pressed while the focus is in this view's own DOM (address bar, find
  // bar). Bound to the container rather than the window so the shortcut only
  // belongs to the browser when the browser is what the user is in.
  // ⌘F from this view's own DOM counts the same way, so pressing it twice
  // re-focuses the bar instead of doing nothing.
  const [localFindRequest, setLocalFindRequest] = useState(0)
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.key.toLowerCase() !== "f") return
    if (!event.metaKey && !event.ctrlKey) return
    event.preventDefault()
    setFindOpen(true)
    setLocalFindRequest((n) => n + 1)
  }
  const url = state?.url || state?.requestedUrl || tab.browser.initialUrl
  const error = state?.error ?? null
  const ownedWindow = state?.surface === "window"

  return (
    <div className="flex h-full min-h-0 flex-col" onKeyDown={onKeyDown}>
      <BrowserToolbar tab={tab} state={state} />
      <BrowserFindBar
        tab={tab}
        open={findOpen && !ownedWindow}
        focusToken={findRequest + localFindRequest}
        onClose={() => setFindOpen(false)}
      />
      <BrowserNoticeBar tab={tab} state={state} />
      <BrowserDownloadBar tab={tab} />
      <div className="relative min-h-0 flex-1">
        {/* Always mounted so the native surface keeps its bounds; the DOM
            layers below only show when the surface is hidden (error) or
            never embedded (owned window). */}
        <BrowserSurfaceHost
          tab={tab}
          hidden={error !== null}
          className={error || ownedWindow ? "invisible" : undefined}
        />
        {error ? (
          <div className="absolute inset-0 bg-background">
            <BrowserErrorPage tab={tab} error={error} url={url} />
          </div>
        ) : ownedWindow ? (
          <div className="absolute inset-0 bg-background">
            <BrowserOwnedWindowCard
              url={url}
              onShow={() =>
                backendId && void browserSetVisible(backendId, true, false)
              }
            />
          </div>
        ) : null}
      </div>
    </div>
  )
}
