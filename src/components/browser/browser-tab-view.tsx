"use client"

import { useState, type KeyboardEvent } from "react"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import { browserSetVisible } from "@/lib/browser/browser-api"
import {
  useBrowserFindRequest,
  useBrowserTabState,
} from "@/lib/browser/browser-tab-store"
import { useBrowserCapabilities } from "@/lib/browser/use-browser-capabilities"
import { browserTabBackendId } from "@/lib/file-tab-id"

import { isDesktop } from "@/lib/transport"
import { cn } from "@/lib/utils"

import { BrowserAgentStrip, useBrowserAgentGlow } from "./browser-agent-access"
import { BrowserBridgeView } from "./browser-bridge-view"
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
 * The file-pane content of a browser tab. On the desktop: toolbar, notices,
 * and the native surface (or, when the page could not load, a DOM error page
 * in its place). In a browser there is no native surface; the tab shows a
 * dev server on the codeg host through the port bridge instead.
 */
export function BrowserTabView({ tab }: { tab: BrowserWorkspaceTab }) {
  if (!isDesktop()) return <BrowserBridgeView tab={tab} />
  return <NativeBrowserTabView tab={tab} />
}

function NativeBrowserTabView({ tab }: { tab: BrowserWorkspaceTab }) {
  const state = useBrowserTabState(tab.id)
  const capabilities = useBrowserCapabilities()
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
  const glow = useBrowserAgentGlow(tab, state)
  const url = state?.url || state?.requestedUrl || tab.browser.initialUrl
  const error = state?.error ?? null
  const ownedWindow = state?.surface === "window"
  // Find searches the page, which is in the other window — worth offering
  // when that window answers, and not when the host has no hold on it.
  const canFind = !ownedWindow || (capabilities?.ownedWindowControls ?? false)

  return (
    <div className="flex h-full min-h-0 flex-col" onKeyDown={onKeyDown}>
      <BrowserToolbar tab={tab} state={state} />
      <BrowserFindBar
        tab={tab}
        open={findOpen && canFind}
        focusToken={findRequest + localFindRequest}
        onClose={() => setFindOpen(false)}
      />
      <BrowserNoticeBar tab={tab} state={state} />
      <BrowserAgentStrip tab={tab} />
      <BrowserDownloadBar tab={tab} />
      {/* The border of the page area, not of anything inside it: a native
          webview paints over any DOM at its rect, so the only mark that can
          be seen on a shared page is one drawn around the hole it sits in.
          The width never changes — the surface is fitted to this element's
          content box, and a border that came and went would reflow the page.
          Only the colour moves: dim while the page is merely shared, bright
          for a moment each time an agent touches it. */}
      <div
        className={cn(
          "relative min-h-0 flex-1 border-2 transition-colors",
          glow === "active"
            ? "border-violet-500"
            : glow === "steady"
              ? "border-violet-500/45"
              : "border-transparent"
        )}
      >
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
