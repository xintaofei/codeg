"use client"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import { browserSetVisible } from "@/lib/browser/browser-api"
import { useBrowserTabState } from "@/lib/browser/browser-tab-store"
import { browserTabBackendId } from "@/lib/file-tab-id"

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
  const url = state?.url || state?.requestedUrl || tab.browser.initialUrl
  const error = state?.error ?? null
  const ownedWindow = state?.surface === "window"

  return (
    <div className="flex h-full min-h-0 flex-col">
      <BrowserToolbar tab={tab} state={state} />
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
