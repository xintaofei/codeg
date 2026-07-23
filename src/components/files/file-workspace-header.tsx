"use client"

import { Code, ExternalLink, Eye } from "lucide-react"
import { useTranslations } from "next-intl"
import { openPath } from "@/lib/platform"
import { isHtmlPreviewable } from "@/lib/language-detect"
import {
  useWorkspaceActions,
  useWorkspaceFileTabs,
} from "@/contexts/workspace-context"
import { FilePathBreadcrumb } from "@/components/files/file-path-breadcrumb"
import { cn } from "@/lib/utils"

/**
 * Desktop file-detail header: the active file's name on the left, its file-type
 * actions on the right — the markdown/html preview⇄source toggle and
 * open-in-browser (html). Maximize/restore lives in the file tab strip
 * (`FileWorkspaceTabBar`, embedded) instead, flush right of the tabs. Rendered
 * only on desktop (`WorkspaceContent`); the mobile panel row keeps these
 * buttons in its own tab bar. Sits above every `FileWorkspacePanel` render
 * branch (editor / preview / diff / image / office), so it wraps them all
 * uniformly.
 */
export function FileWorkspaceHeader() {
  const t = useTranslations("Folder.fileWorkspace")
  const { activeFileTab, activeFileTabId, previewFileTabIds } =
    useWorkspaceFileTabs()
  const { toggleFileTabPreview } = useWorkspaceActions()

  if (!activeFileTab) return null

  const isDiff =
    activeFileTab.kind === "diff" || activeFileTab.kind === "rich-diff"
  const isDirty =
    activeFileTab.kind === "file" && Boolean(activeFileTab.isDirty)
  // Mirror the gating the file tab strip used (file-workspace-tab-bar.tsx):
  // preview toggle for markdown/html, browser-open for html.
  const canPreview =
    activeFileTab.kind === "file" &&
    (activeFileTab.language === "markdown" ||
      isHtmlPreviewable(activeFileTab.path))
  const canOpenInBrowser =
    activeFileTab.kind === "file" && isHtmlPreviewable(activeFileTab.path)
  const isPreviewActive =
    canPreview && activeFileTabId
      ? previewFileTabIds.has(activeFileTabId)
      : false

  const actionBtn =
    "flex h-7 w-7 shrink-0 items-center justify-center rounded hover:bg-primary/8 transition-colors"

  return (
    // Transparent like the conversation detail header
    // (conversation-detail-header.tsx): the title header merges with the canvas
    // below it (editor / preview) instead of being a frosted band. With a
    // workspace background image on, the tab strip above it is transparent too,
    // so the whole top of the column reveals the canvas.
    <div className="flex h-10 shrink-0 items-center gap-2 border-b border-border/50 px-3">
      {/* No leading file-type icon — the folder name leads the breadcrumb, and
          the text matches the conversation detail header's `text-sm` title. */}
      <div className="flex min-w-0 flex-1 items-center gap-1.5 text-sm">
        {/* Diff tabs have no single navigable path — keep them as a plain
            title. Plain files render a clickable path breadcrumb. */}
        {isDiff || !activeFileTab.path ? (
          <span
            className="truncate text-foreground/90"
            title={activeFileTab.description ?? activeFileTab.title}
          >
            {activeFileTab.title}
            {isDirty ? " *" : ""}
          </span>
        ) : (
          <FilePathBreadcrumb
            path={activeFileTab.path}
            fileName={activeFileTab.title}
            isDirty={isDirty}
          />
        )}
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        {canPreview && activeFileTabId && (
          <button
            type="button"
            onClick={() => toggleFileTabPreview(activeFileTabId)}
            className={cn(actionBtn, isPreviewActive && "text-primary")}
            aria-label={isPreviewActive ? t("editSource") : t("preview")}
            title={isPreviewActive ? t("editSource") : t("preview")}
          >
            {isPreviewActive ? (
              <Code className="h-4 w-4" />
            ) : (
              <Eye className="h-4 w-4" />
            )}
          </button>
        )}
        {canOpenInBrowser && activeFileTab.path && (
          <button
            type="button"
            onClick={() => {
              // File tab paths are absolute — hand the path straight to the OS.
              openPath(activeFileTab.path as string).catch(() => {})
            }}
            className={actionBtn}
            aria-label={t("preview")}
            title={t("preview")}
          >
            <ExternalLink className="h-4 w-4" />
          </button>
        )}
      </div>
    </div>
  )
}
