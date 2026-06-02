"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2 } from "lucide-react"

interface ShadcnPreviewProps {
  previewUrl: string
}

// 合并连续的配置编辑，稳定后再让 iframe 重载一次，而非每次改动都重载。
const DEBOUNCE_MS = 500
// 兜底：若 iframe 始终不触发 load（离线/被拦/webview 异常），到时强制收起遮罩，
// 避免永久停在「加载预览…」。
const LOAD_TIMEOUT_MS = 15000

export function ShadcnPreview({ previewUrl }: ShadcnPreviewProps) {
  const t = useTranslations("ProjectBoot")
  // 已提交到 iframe 的 URL：比 previewUrl 滞后一个 debounce 窗口；
  // 挂载时与 previewUrl 相等，首帧即指向正确页面。
  const [committedUrl, setCommittedUrl] = useState(previewUrl)
  const [loading, setLoading] = useState(true)

  // 仅当 URL 真正变化时才在 debounce 后提交并重置 loading。
  // 守卫确保挂载/无变化时是 no-op —— 否则会在不重载 iframe 的情况下顶起
  // loading，导致遮罩永久卡住。
  useEffect(() => {
    if (previewUrl === committedUrl) return
    const id = setTimeout(() => {
      setCommittedUrl(previewUrl)
      setLoading(true)
    }, DEBOUNCE_MS)
    return () => clearTimeout(id)
  }, [previewUrl, committedUrl])

  // 每次提交加载的兜底：load 始终不来时收起遮罩。
  useEffect(() => {
    if (!loading) return
    const id = setTimeout(() => setLoading(false), LOAD_TIMEOUT_MS)
    return () => clearTimeout(id)
  }, [loading, committedUrl])

  return (
    <div className="relative h-full w-full">
      {loading && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-background/80">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          <span className="ml-2 text-sm text-muted-foreground">
            {t("preview.loading")}
          </span>
        </div>
      )}
      <iframe
        key={committedUrl}
        src={committedUrl}
        className="h-full w-full border-0"
        onLoad={() => setLoading(false)}
        onError={() => setLoading(false)}
        sandbox="allow-scripts allow-same-origin allow-popups"
      />
    </div>
  )
}
