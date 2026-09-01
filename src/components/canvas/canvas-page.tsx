"use client"

import dynamic from "next/dynamic"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { WorkbenchPageTitle } from "@/components/workbench/workbench-page-title"

/**
 * ReactFlow (and the canvas machinery) stays out of the first-paint chunk:
 * the page shell is registered in WORKBENCH_ROUTES, the heavy view loads on
 * first visit. `ssr: false` is moot under static export but explicit — the
 * view reads `window` for viewport math.
 */
const CanvasView = dynamic(() => import("./canvas-view"), {
  ssr: false,
  loading: () => (
    <div className="flex h-full items-center justify-center">
      <Loader2 className="size-5 animate-spin text-muted-foreground/60" />
    </div>
  ),
})

export function CanvasPage() {
  return (
    <div className="h-full min-h-0 w-full">
      <CanvasView />
    </div>
  )
}

export function CanvasPageTitle() {
  const t = useTranslations("Canvas")
  return <WorkbenchPageTitle title={t("title")} />
}
