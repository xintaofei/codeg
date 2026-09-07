"use client"

import { useRef, useState, type KeyboardEvent } from "react"
import {
  ArrowLeft,
  ArrowRight,
  Copy,
  ExternalLink,
  RotateCw,
  X,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import {
  browserGoBack,
  browserGoForward,
  browserNavigate,
  browserReload,
  browserStop,
} from "@/lib/browser/browser-api"
import type { BrowserTabState } from "@/lib/browser/types"
import { browserTabBackendId } from "@/lib/file-tab-id"
import { openUrl } from "@/lib/platform"
import { cn, copyTextToClipboard } from "@/lib/utils"

const ICON_BTN =
  "flex h-7 w-7 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/8 hover:text-foreground disabled:pointer-events-none disabled:opacity-40"

/**
 * Turn what a user typed into something the tab can load: a full URL as is,
 * a bare host / host:port / IP with an https (or http for loopback) prefix.
 * No search-engine fallback — the address bar is an address bar.
 */
export function normalizeTypedAddress(raw: string): string | null {
  const text = raw.trim()
  if (!text) return null
  if (/^https?:\/\//i.test(text)) {
    try {
      return new URL(text).toString()
    } catch {
      return null
    }
  }
  // A scheme other than http(s) is refused — but `localhost:3000/app` is a
  // host with a port, not a scheme, so a colon followed by digits is fine.
  if (/^[a-z][a-z\d+\-.]*:(?!\d)/i.test(text)) return null
  if (/\s/.test(text)) return null
  const hostPart = text.split(/[/?#]/)[0]
  if (
    !hostPart.includes(".") &&
    !/^(localhost|\[?[\d:.]+\]?)(:\d+)?$/i.test(hostPart)
  ) {
    return null
  }
  const isLocal = /^(localhost|127\.|\[::1\]|0\.0\.0\.0|10\.|192\.168\.)/i.test(
    hostPart
  )
  try {
    return new URL(`${isLocal ? "http" : "https"}://${text}`).toString()
  } catch {
    return null
  }
}

export function BrowserToolbar({
  tab,
  state,
}: {
  tab: BrowserWorkspaceTab
  state: BrowserTabState | null
}) {
  const t = useTranslations("Browser.toolbar")
  const backendId = browserTabBackendId(tab.id)
  const currentUrl = state?.url || state?.requestedUrl || tab.browser.initialUrl
  const [draft, setDraft] = useState(currentUrl)
  const [editing, setEditing] = useState(false)
  const [mirroredUrl, setMirroredUrl] = useState(currentUrl)
  const inputRef = useRef<HTMLInputElement | null>(null)

  // Mirror the live URL unless the user is typing. Adjusted during render
  // (the "state from previous renders" pattern) rather than in an effect, so
  // the address bar never paints a stale URL for a frame.
  if (mirroredUrl !== currentUrl) {
    setMirroredUrl(currentUrl)
    if (!editing) setDraft(currentUrl)
  }

  const loading = state?.loading ?? true

  const submit = () => {
    if (!backendId) return
    const url = normalizeTypedAddress(draft)
    if (!url) {
      toast.error(t("invalidUrl"))
      return
    }
    setEditing(false)
    inputRef.current?.blur()
    void browserNavigate(backendId, url).catch((error: unknown) => {
      toast.error(t("invalidUrl"), { description: String(error) })
    })
  }

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault()
      submit()
    } else if (event.key === "Escape") {
      event.preventDefault()
      setDraft(currentUrl)
      setEditing(false)
      inputRef.current?.blur()
    }
  }

  return (
    <div className="relative flex h-9 shrink-0 items-center gap-1 border-b border-border/60 bg-muted/40 px-1.5">
      <button
        type="button"
        className={ICON_BTN}
        title={t("back")}
        aria-label={t("back")}
        disabled={!backendId || !state?.canGoBack}
        onClick={() => backendId && void browserGoBack(backendId)}
      >
        <ArrowLeft className="h-4 w-4" />
      </button>
      <button
        type="button"
        className={ICON_BTN}
        title={t("forward")}
        aria-label={t("forward")}
        disabled={!backendId || !state?.canGoForward}
        onClick={() => backendId && void browserGoForward(backendId)}
      >
        <ArrowRight className="h-4 w-4" />
      </button>
      <button
        type="button"
        className={ICON_BTN}
        title={loading ? t("stop") : t("reload")}
        aria-label={loading ? t("stop") : t("reload")}
        disabled={!backendId}
        onClick={() =>
          backendId &&
          void (loading ? browserStop(backendId) : browserReload(backendId))
        }
      >
        {loading ? <X className="h-4 w-4" /> : <RotateCw className="h-4 w-4" />}
      </button>
      <input
        ref={inputRef}
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onFocus={(event) => {
          setEditing(true)
          event.currentTarget.select()
        }}
        onBlur={() => setEditing(false)}
        onKeyDown={onKeyDown}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        placeholder={t("addressPlaceholder")}
        aria-label={t("addressPlaceholder")}
        className={cn(
          "mx-1 h-7 min-w-0 flex-1 rounded-md border border-transparent bg-background px-2.5 text-xs text-foreground outline-none",
          "focus:border-ring/50 focus:ring-2 focus:ring-ring/20"
        )}
      />
      <button
        type="button"
        className={ICON_BTN}
        title={t("copyUrl")}
        aria-label={t("copyUrl")}
        onClick={() => {
          void copyTextToClipboard(currentUrl).then(() =>
            toast.success(t("copied"))
          )
        }}
      >
        <Copy className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        className={ICON_BTN}
        title={t("openInSystem")}
        aria-label={t("openInSystem")}
        onClick={() => void openUrl(currentUrl)}
      >
        <ExternalLink className="h-3.5 w-3.5" />
      </button>
      {loading ? (
        <div
          aria-hidden
          className="pointer-events-none absolute inset-x-0 bottom-0 h-0.5 overflow-hidden"
        >
          <div className="h-full w-1/3 animate-[browser-loading_1.2s_ease-in-out_infinite] bg-primary" />
        </div>
      ) : null}
    </div>
  )
}
