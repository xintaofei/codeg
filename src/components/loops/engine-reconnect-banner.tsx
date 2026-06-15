"use client"

import { useSyncExternalStore } from "react"
import { useTranslations } from "next-intl"
import { Loader2 } from "lucide-react"

import {
  getWebConnectionServerSnapshot,
  getWebConnectionSnapshot,
  subscribeWebConnection,
} from "@/lib/transport/web-connection-store"

/** A quiet inline banner shown inside the loop workbench while the web transport
 *  is reconnecting — loop events are dropped during a disconnect window, so this
 *  tells the operator the view may be briefly stale (it auto-resyncs on
 *  reconnect via the realtime provider's `null` batch). Inert on desktop/SSR
 *  (the store reports a stable "connected" there). */
export function EngineReconnectBanner() {
  const t = useTranslations("Loops.engineHealth")
  const state = useSyncExternalStore(
    subscribeWebConnection,
    getWebConnectionSnapshot,
    getWebConnectionServerSnapshot
  )
  if (state !== "reconnecting") return null
  return (
    <div className="flex shrink-0 items-center justify-center gap-2 bg-amber-500/10 px-3 py-1 text-xs text-amber-600">
      <Loader2 className="h-3 w-3 animate-spin" />
      {t("reconnecting")}
    </div>
  )
}
