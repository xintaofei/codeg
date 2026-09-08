"use client"

import { useEffect, useState } from "react"
import { Copy, ExternalLink, Loader2, RotateCw } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import {
  bridgeClose,
  bridgeEntryUrl,
  bridgeOpen,
  bridgeOrigin,
  probeBridge,
  type BridgeGrant,
} from "@/lib/browser/browser-bridge"
import { browserTabBackendId } from "@/lib/file-tab-id"
import { openExternalTab } from "@/lib/link-open"
import { copyTextToClipboard } from "@/lib/utils"

const ICON_BTN =
  "flex h-7 w-7 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/8 hover:text-foreground disabled:pointer-events-none disabled:opacity-40"

/** The frame keeps its origin (its own bridge port, shared with nothing) and
 *  never navigates the workbench. */
export const BRIDGE_FRAME_SANDBOX =
  "allow-scripts allow-forms allow-same-origin allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads"

type Phase =
  | { kind: "opening" }
  | { kind: "ready"; grant: BridgeGrant; src: string }
  | { kind: "unreachable"; grant: BridgeGrant; origin: string; src: string }
  | { kind: "error"; message: string }

function messageOf(error: unknown): string {
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message
    if (typeof message === "string" && message) return message
  }
  return String(error)
}

/**
 * The web-mode body of a browser tab: a dev server on the codeg host shown
 * through the port bridge in an iframe. Each mount takes a fresh grant (a
 * reload too), releases it on unmount, and probes the bridge port from this
 * browser before showing the frame so an unreachable port is explained
 * instead of left blank. "Open in a new tab" stays available throughout:
 * whatever the frame cannot show, a top-level tab on the same bridge origin
 * can.
 */
export function BrowserBridgeView({ tab }: { tab: BrowserWorkspaceTab }) {
  const t = useTranslations("Browser.bridge")
  const url = tab.browser.initialUrl
  const tabId = browserTabBackendId(tab.id) ?? tab.id
  const [attempt, setAttempt] = useState(0)
  // The outcome is stamped with the attempt it belongs to; a new attempt
  // (a reload, another address) reads as "opening" until its own outcome
  // lands, without a synchronous reset in the effect.
  const [outcome, setOutcome] = useState<{
    attempt: number
    url: string
    phase: Phase
  } | null>(null)
  const phase: Phase =
    outcome && outcome.attempt === attempt && outcome.url === url
      ? outcome.phase
      : { kind: "opening" }

  useEffect(() => {
    let cancelled = false
    const settle = (phase: Phase) => {
      if (!cancelled) setOutcome({ attempt, url, phase })
    }
    void (async () => {
      let grant: BridgeGrant
      try {
        grant = await bridgeOpen(url, tabId)
      } catch (error) {
        settle({ kind: "error", message: messageOf(error) })
        return
      }
      const page = window.location
      const origin = bridgeOrigin(grant, page)
      const src = bridgeEntryUrl(grant, page)
      const reachable = await probeBridge(origin)
      settle(
        reachable
          ? { kind: "ready", grant, src }
          : { kind: "unreachable", grant, origin, src }
      )
    })()
    return () => {
      cancelled = true
    }
  }, [url, tabId, attempt])

  // The hold ends with the view: the listener closes a minute later unless
  // another tab uses it. Coming back mints a new grant and reloads the page.
  useEffect(
    () => () => {
      void bridgeClose(tabId).catch(() => {})
    },
    [tabId]
  )

  const src =
    phase.kind === "ready" || phase.kind === "unreachable" ? phase.src : null

  const copyAddress = async () => {
    const ok = await copyTextToClipboard(url)
    if (ok) toast.success(t("copied"))
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border/60 px-2 text-xs">
        <span
          className="min-w-0 flex-1 truncate px-1 text-muted-foreground"
          title={url}
        >
          {url}
        </span>
        <button
          type="button"
          className={ICON_BTN}
          onClick={() => setAttempt((n) => n + 1)}
          title={t("reload")}
          aria-label={t("reload")}
        >
          <RotateCw className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          className={ICON_BTN}
          disabled={src === null}
          onClick={() => {
            if (src) openExternalTab(src)
          }}
          title={t("openExternal")}
          aria-label={t("openExternal")}
        >
          <ExternalLink className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          className={ICON_BTN}
          onClick={() => void copyAddress()}
          title={t("copyAddress")}
          aria-label={t("copyAddress")}
        >
          <Copy className="h-3.5 w-3.5" />
        </button>
      </div>
      <div className="relative min-h-0 flex-1">
        {phase.kind === "opening" ? (
          <div
            role="status"
            className="flex h-full items-center justify-center gap-2 text-xs text-muted-foreground"
          >
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("opening")}
          </div>
        ) : null}
        {phase.kind === "ready" ? (
          <iframe
            key={phase.src}
            title={t("frameTitle")}
            src={phase.src}
            sandbox={BRIDGE_FRAME_SANDBOX}
            referrerPolicy="no-referrer"
            className="absolute inset-0 h-full w-full border-0 bg-white"
          />
        ) : null}
        {phase.kind === "unreachable" ? (
          <Notice
            title={t("unreachableTitle")}
            body={t("unreachableHint", { origin: phase.origin })}
            action={{
              label: t("openExternal"),
              onClick: () => openExternalTab(phase.src),
            }}
          />
        ) : null}
        {phase.kind === "error" ? (
          <Notice title={t("errorTitle")} body={phase.message} />
        ) : null}
      </div>
    </div>
  )
}

function Notice({
  title,
  body,
  action,
}: {
  title: string
  body: string
  action?: { label: string; onClick: () => void }
}) {
  return (
    <div
      role="status"
      className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center"
    >
      <p className="text-sm font-medium text-foreground">{title}</p>
      <p className="max-w-md text-xs text-muted-foreground">{body}</p>
      {action ? (
        <button
          type="button"
          className="mt-1 inline-flex h-7 items-center gap-1.5 rounded-md border border-border px-2 text-xs text-foreground hover:bg-primary/8"
          onClick={action.onClick}
        >
          <ExternalLink className="h-3.5 w-3.5" />
          {action.label}
        </button>
      ) : null}
    </div>
  )
}
