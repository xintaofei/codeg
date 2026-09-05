"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  ChevronLeft,
  ChevronRight,
  Download,
  ImageOff,
  Loader2,
  RefreshCw,
  Store,
} from "lucide-react"
import { toast } from "sonner"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useProxiedBackgroundThumb } from "@/hooks/use-proxied-background-thumb"
import {
  MARKET_CATEGORIES,
  searchWorkspaceBgMarket,
  type MarketCategory,
  type MarketWallpaper,
} from "@/lib/workspace-background-market"
import { cn } from "@/lib/utils"

const SEARCH_DEBOUNCE_MS = 300

interface WorkspaceBackgroundMarketDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** 当前背景的市场来源页（本地图/未设置为 null），用于「使用中」标记。 */
  appliedSourceUrl: string | null
  /** 下载并应用（provider 的 downloadMarketWorkspaceBackground）。 */
  onApply: (url: string, sourceUrl: string) => Promise<void>
}

function MarketCard({
  wallpaper,
  applied,
  downloading,
  onApply,
}: {
  wallpaper: MarketWallpaper
  applied: boolean
  downloading: boolean
  onApply: (wallpaper: MarketWallpaper) => void
}) {
  const t = useTranslations("AppearanceSettings.workspaceBackground.market")
  const thumb = useProxiedBackgroundThumb(wallpaper.thumbUrl)

  return (
    <button
      type="button"
      aria-label={wallpaper.id}
      title={wallpaper.resolution || wallpaper.id}
      disabled={downloading}
      onClick={() => onApply(wallpaper)}
      className="group relative aspect-[3/2] overflow-hidden rounded-md border bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60"
    >
      {thumb.src ? (
        // 预览是后端代理的 blob URL，next/image 不适用；用原生 img。
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={thumb.src}
          alt={wallpaper.id}
          className="h-full w-full object-cover"
          loading="lazy"
        />
      ) : (
        <span className="flex h-full w-full items-center justify-center">
          {thumb.failed ? (
            <ImageOff className="h-5 w-5 text-muted-foreground/50" />
          ) : (
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground/50" />
          )}
        </span>
      )}
      {wallpaper.resolution && (
        <span className="absolute bottom-1 left-1 rounded bg-background/80 px-1 text-2xs tabular-nums text-foreground/80">
          {wallpaper.resolution}
        </span>
      )}
      {applied && (
        <Badge className="absolute right-1 top-1 h-4 px-1 text-2xs">
          {t("applied")}
        </Badge>
      )}
      <span className="absolute inset-0 flex items-center justify-center bg-background/40 opacity-0 transition-opacity group-hover:opacity-100">
        {downloading ? (
          <Loader2 className="h-5 w-5 animate-spin" />
        ) : (
          <Download className="h-5 w-5" />
        )}
      </span>
    </button>
  )
}

export function WorkspaceBackgroundMarketDialog({
  open,
  onOpenChange,
  appliedSourceUrl,
  onApply,
}: WorkspaceBackgroundMarketDialogProps) {
  const t = useTranslations("AppearanceSettings.workspaceBackground.market")
  const [searchInput, setSearchInput] = useState("")
  const [query, setQuery] = useState("")
  const [category, setCategory] = useState<MarketCategory>("all")
  const [page, setPage] = useState(1)
  const [items, setItems] = useState<MarketWallpaper[]>([])
  const [lastPage, setLastPage] = useState(1)
  const [loading, setLoading] = useState(false)
  // 失败标记（文案在渲染期翻译，避免把 t 引进 load 的依赖）。
  const [error, setError] = useState(false)
  const [downloadingId, setDownloadingId] = useState<string | null>(null)
  // 代次守卫：慢请求晚归不覆盖新请求的结果（与宠物市场同款问题）。
  const requestSeq = useRef(0)

  // 搜索框 debounce；输入变化回到第 1 页。
  useEffect(() => {
    const handle = setTimeout(() => {
      setQuery(searchInput.trim())
      setPage(1)
    }, SEARCH_DEBOUNCE_MS)
    return () => clearTimeout(handle)
  }, [searchInput])

  const load = useCallback(async (q: string, c: MarketCategory, p: number) => {
    const seq = ++requestSeq.current
    setLoading(true)
    setError(false)
    try {
      const result = await searchWorkspaceBgMarket({
        query: q,
        category: c,
        page: p,
      })
      if (seq !== requestSeq.current) return
      setItems(result.items)
      setLastPage(result.lastPage)
    } catch {
      if (seq !== requestSeq.current) return
      setItems([])
      setLastPage(1)
      setError(true)
    } finally {
      if (seq === requestSeq.current) setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (!open) return
    void load(query, category, page)
  }, [open, query, category, page, load])

  const onCardApply = async (wallpaper: MarketWallpaper) => {
    setError(false)
    setDownloadingId(wallpaper.id)
    try {
      await onApply(wallpaper.fullUrl, wallpaper.sourceUrl)
      toast.success(t("appliedToast"))
    } catch {
      toast.error(t("downloadFailed"))
    } finally {
      setDownloadingId(null)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-3xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-sm">
            <Store className="h-4 w-4" />
            {t("title")}
          </DialogTitle>
          <p className="text-2xs text-muted-foreground">
            {t("description")} · {t("credit")}
          </p>
        </DialogHeader>

        {/* 搜索 + 分类 */}
        <div className="flex flex-wrap items-center gap-2">
          <Input
            value={searchInput}
            onChange={(e) => setSearchInput(e.target.value)}
            placeholder={t("searchPlaceholder")}
            className="h-8 w-56"
          />
          <div className="flex items-center gap-1">
            {MARKET_CATEGORIES.map((c) => (
              <Button
                key={c}
                type="button"
                variant={category === c ? "default" : "ghost"}
                size="sm"
                className="h-8 px-2 text-xs"
                onClick={() => {
                  setCategory(c)
                  setPage(1)
                }}
              >
                {t(`categories.${c}`)}
              </Button>
            ))}
          </div>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-8"
            disabled={loading}
            onClick={() => void load(query, category, page)}
            aria-label={t("retry")}
          >
            {loading ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <RefreshCw className="h-4 w-4" />
            )}
          </Button>
        </div>

        {/* 网格 / 三态 */}
        <ScrollArea className="h-[55vh] pr-3">
          {error ? (
            <div className="flex flex-col items-center gap-3 py-12">
              <p className="text-xs text-destructive">{t("error")}</p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void load(query, category, page)}
              >
                <RefreshCw className="h-3.5 w-3.5" />
                {t("retry")}
              </Button>
            </div>
          ) : loading && items.length === 0 ? (
            <div className="flex items-center justify-center py-12">
              <Loader2 className="h-5 w-5 animate-spin text-muted-foreground/50" />
            </div>
          ) : items.length === 0 ? (
            <p className="py-12 text-center text-xs text-muted-foreground">
              {t("empty")}
            </p>
          ) : (
            <div
              className={cn(
                "grid grid-cols-2 gap-2 transition-opacity sm:grid-cols-3 md:grid-cols-4",
                loading && "opacity-50"
              )}
            >
              {items.map((w) => (
                <MarketCard
                  key={w.id}
                  wallpaper={w}
                  applied={appliedSourceUrl === w.sourceUrl}
                  downloading={downloadingId === w.id}
                  onApply={(item) => void onCardApply(item)}
                />
              ))}
            </div>
          )}
        </ScrollArea>

        {/* 分页 */}
        <div className="flex items-center justify-between">
          <span className="text-2xs tabular-nums text-muted-foreground">
            {t("pageInfo", { page, lastPage })}
          </span>
          <div className="flex items-center gap-1">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-8"
              disabled={page <= 1 || loading}
              onClick={() => setPage((p) => Math.max(1, p - 1))}
            >
              <ChevronLeft className="h-4 w-4" />
              {t("prevPage")}
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-8"
              disabled={page >= lastPage || loading}
              onClick={() => setPage((p) => p + 1)}
            >
              {t("nextPage")}
              <ChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
