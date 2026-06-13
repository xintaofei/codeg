"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"

import { getLoopArtifact } from "@/lib/loops-api"
import type { LoopArtifactDetail } from "@/lib/types"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"

/**
 * Read-only drawer for a single artifact: its kind/status and the latest
 * revision's content. The M2.1 simple view — richer history/criteria/links
 * arrive with the dedicated artifact tooling later.
 */
export function ArtifactDrawer({
  artifactId,
  onClose,
}: {
  artifactId: number | null
  onClose: () => void
}) {
  const t = useTranslations("Loops.artifactDrawer")
  const tKind = useTranslations("Loops.artifactKind")
  const tStatus = useTranslations("Loops.artifactStatus")

  const [detail, setDetail] = useState<LoopArtifactDetail | null>(null)
  const [loading, setLoading] = useState(false)
  // Monotonic request id: a slower earlier fetch must not overwrite a newer one.
  const reqRef = useRef(0)

  const load = useCallback(async (id: number) => {
    const req = ++reqRef.current
    setLoading(true)
    setDetail(null)
    try {
      const d = await getLoopArtifact(id)
      if (reqRef.current === req) setDetail(d)
    } finally {
      if (reqRef.current === req) setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (artifactId != null) void load(artifactId)
  }, [artifactId, load])

  // Newest revision first.
  const latest = detail
    ? [...detail.revisions].sort((a, b) => b.seq - a.seq)[0]
    : undefined

  return (
    <Sheet open={artifactId != null} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="w-full sm:max-w-md">
        <SheetHeader>
          <SheetTitle className="truncate">
            {detail?.title ?? t("loading")}
          </SheetTitle>
          <SheetDescription asChild>
            <div className="flex flex-wrap items-center gap-1.5">
              {detail && (
                <>
                  <Badge variant="outline">{tKind(detail.kind)}</Badge>
                  <Badge variant="secondary">{tStatus(detail.status)}</Badge>
                  {detail.revisions.length > 0 && (
                    <span className="text-xs text-muted-foreground">
                      {t("revisionCount", { count: detail.revisions.length })}
                    </span>
                  )}
                </>
              )}
            </div>
          </SheetDescription>
        </SheetHeader>

        <ScrollArea className="min-h-0 flex-1 px-4 pb-4">
          {loading ? (
            <div className="space-y-2 pt-1">
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-2/3" />
            </div>
          ) : latest && latest.content.trim().length > 0 ? (
            <p className="whitespace-pre-wrap break-words pt-1 text-sm">
              {latest.content}
            </p>
          ) : (
            <p className="pt-1 text-sm text-muted-foreground">
              {t("noContent")}
            </p>
          )}
        </ScrollArea>
      </SheetContent>
    </Sheet>
  )
}
