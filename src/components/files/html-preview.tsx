"use client"

import { useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { ShieldCheck, ShieldOff } from "lucide-react"
import { readWorkspaceFileBase64 } from "@/lib/api"
import {
  extractHtmlTitle,
  inlineHtmlResources,
  withSandboxCsp,
} from "@/lib/html-preview-inline"
import type { FileWorkspaceTab } from "@/contexts/workspace-context"
import { cn } from "@/lib/utils"

// Trusted sandbox: scripts run, popups/forms/modals work, but the frame still
// has an opaque origin (no allow-same-origin) and cannot navigate the top
// window (no allow-top-navigation). The default untrusted mode uses an empty
// sandbox, which renders markup/CSS/images but blocks ALL script execution —
// the actual in-app security boundary for previewing untrusted HTML.
const SANDBOX_TRUSTED = "allow-scripts allow-popups allow-forms allow-modals"

export function HtmlPreview({
  tab,
  folderPath,
}: {
  tab: FileWorkspaceTab
  folderPath: string | null
}) {
  const t = useTranslations("Folder.fileWorkspacePanel")
  const [inlined, setInlined] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [trusted, setTrusted] = useState(false)

  const content = tab.content ?? ""
  const path = tab.path ?? ""

  // The document's own <title>, shown at the left of the header bar; falls back
  // to the file name when the document has none. Parsed from the raw source
  // (not the inlined output) so it updates immediately, without loading any
  // resource.
  const title = useMemo(() => extractHtmlTitle(content), [content])
  const heading = title || path.split("/").pop() || path

  useEffect(() => {
    let cancelled = false
    const root = folderPath ?? ""
    const absFilePath = root && path ? `${root}/${path}` : null
    const fileDir = absFilePath ? absFilePath.replace(/\/[^/]*$/, "") : root
    inlineHtmlResources(content, {
      fileDir,
      folderPath: root,
      // The inliner hands us absolute, lexically in-root paths; convert to a
      // workspace-relative path and read through the symlink-safe, confined
      // backend command (defense-in-depth over the lexical check).
      readFileBase64: (absPath) => {
        const r = root.replace(/\\/g, "/").replace(/\/+$/, "")
        const a = absPath.replace(/\\/g, "/")
        const rel =
          a === r ? "" : a.startsWith(r + "/") ? a.slice(r.length + 1) : a
        return readWorkspaceFileBase64(root, rel)
      },
    })
      .then((html) => {
        if (!cancelled) {
          setInlined(html)
          setError(null)
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setInlined(null)
          setError(String(err))
        }
      })
    return () => {
      cancelled = true
    }
  }, [content, path, folderPath])

  const srcDoc = inlined != null ? withSandboxCsp(inlined, { trusted }) : ""
  const loading = inlined == null && error == null

  return (
    <div className="h-full flex flex-col min-h-0">
      <div className="h-9 shrink-0 flex items-center justify-between gap-3 px-3 border-b border-border bg-muted/20">
        <span
          className="min-w-0 truncate text-xs font-medium text-foreground/80"
          title={heading || undefined}
        >
          {heading}
        </span>
        <button
          type="button"
          onClick={() => setTrusted((v) => !v)}
          aria-pressed={trusted}
          title={t("htmlPreviewTrustHint")}
          className={cn(
            "inline-flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-xs transition-colors",
            trusted
              ? "text-amber-600 dark:text-amber-500 hover:bg-amber-500/10"
              : "text-muted-foreground hover:bg-primary/8"
          )}
        >
          {trusted ? (
            <ShieldOff className="h-3.5 w-3.5" />
          ) : (
            <ShieldCheck className="h-3.5 w-3.5" />
          )}
          {t("htmlPreviewTrust")}
        </button>
      </div>
      <div className="relative flex-1 min-h-0">
        {loading && (
          <div className="absolute top-2 right-3 z-10 rounded-md bg-background/70 px-2 py-1 text-[11px] text-muted-foreground backdrop-blur-sm">
            {t("loading")}
          </div>
        )}
        {error ? (
          <div className="h-full flex items-center justify-center px-6 text-center text-xs text-muted-foreground">
            {error}
          </div>
        ) : (
          inlined != null && (
            <iframe
              key={trusted ? "trusted" : "strict"}
              title={t("htmlPreviewTitle")}
              sandbox={trusted ? SANDBOX_TRUSTED : ""}
              srcDoc={srcDoc}
              className="absolute inset-0 h-full w-full border-0 bg-white"
            />
          )
        )}
      </div>
    </div>
  )
}
