"use client"

import { useCallback, useMemo, useRef, useState } from "react"
import dynamic from "next/dynamic"
import { ChevronLeft, ChevronRight } from "lucide-react"
import type { DiffOnMount } from "@monaco-editor/react"
import type { editor as MonacoEditorNs } from "monaco-editor"
import { defineMonacoThemes, useMonacoThemeSync } from "@/lib/monaco-themes"
import { cn } from "@/lib/utils"
import { getCodeFontSize } from "@/lib/appearance-settings"

import "@/lib/monaco-local"

const MonacoDiffEditor = dynamic(
  async () => {
    const mod = await import("@monaco-editor/react")
    return { default: mod.DiffEditor }
  },
  { ssr: false }
)

export interface DiffViewerProps {
  original: string
  modified: string
  originalLabel?: string
  modifiedLabel?: string
  language?: string
  className?: string
}

export function DiffViewer({
  original,
  modified,
  originalLabel = "Original",
  modifiedLabel = "Modified",
  language = "plaintext",
  className,
}: DiffViewerProps) {
  const editorTheme = useMonacoThemeSync()
  const diffEditorRef = useRef<MonacoEditorNs.IStandaloneDiffEditor | null>(
    null
  )
  const [diffChanges, setDiffChanges] = useState<MonacoEditorNs.ILineChange[]>(
    []
  )
  const [currentChangeIndex, setCurrentChangeIndex] = useState(-1)

  const handleEditorMount: DiffOnMount = useCallback((editor) => {
    diffEditorRef.current = editor
    let scrolledToFirst = false

    const updateDiffs = () => {
      const changes = editor.getLineChanges()
      setDiffChanges(changes ?? [])
      if (changes && changes.length > 0) {
        setCurrentChangeIndex(0)
        // Auto-scroll to the first change only once
        if (!scrolledToFirst) {
          scrolledToFirst = true
          const first = changes[0]
          const lineNumber =
            first.modifiedStartLineNumber || first.originalStartLineNumber || 1
          const modifiedEditor = editor.getModifiedEditor()
          modifiedEditor.revealLineInCenter(lineNumber)
          modifiedEditor.setPosition({ lineNumber, column: 1 })
        }
      }
    }

    editor.onDidUpdateDiff(updateDiffs)
    setTimeout(updateDiffs, 300)
  }, [])

  const navigateToChange = useCallback(
    (index: number) => {
      const editor = diffEditorRef.current
      if (!editor || diffChanges.length === 0) return

      const clampedIndex = Math.max(0, Math.min(index, diffChanges.length - 1))
      setCurrentChangeIndex(clampedIndex)

      const change = diffChanges[clampedIndex]
      const lineNumber =
        change.modifiedStartLineNumber || change.originalStartLineNumber || 1

      const modifiedEditor = editor.getModifiedEditor()
      modifiedEditor.revealLineInCenter(lineNumber)
      modifiedEditor.setPosition({ lineNumber, column: 1 })
    },
    [diffChanges]
  )

  const handlePrevChange = useCallback(() => {
    if (currentChangeIndex > 0) {
      navigateToChange(currentChangeIndex - 1)
    }
  }, [currentChangeIndex, navigateToChange])

  const handleNextChange = useCallback(() => {
    if (currentChangeIndex < diffChanges.length - 1) {
      navigateToChange(currentChangeIndex + 1)
    }
  }, [currentChangeIndex, diffChanges.length, navigateToChange])

  const { additions, deletions } = useMemo(() => {
    let add = 0
    let del = 0
    for (const change of diffChanges) {
      // Monaco ILineChange: endLineNumber === 0 means no lines on that side
      // Pure insertion: originalEndLineNumber === 0
      // Pure deletion: modifiedEndLineNumber === 0
      const isInsertion = change.originalEndLineNumber === 0
      const isDeletion = change.modifiedEndLineNumber === 0

      if (isInsertion) {
        add += change.modifiedEndLineNumber - change.modifiedStartLineNumber + 1
      } else if (isDeletion) {
        del += change.originalEndLineNumber - change.originalStartLineNumber + 1
      } else {
        del += change.originalEndLineNumber - change.originalStartLineNumber + 1
        add += change.modifiedEndLineNumber - change.modifiedStartLineNumber + 1
      }
    }
    return { additions: add, deletions: del }
  }, [diffChanges])

  return (
    <div className={cn("flex h-full flex-col", className)}>
      <div className="flex items-center gap-3 border-b bg-muted/50 px-3 py-1.5 text-xs text-muted-foreground">
        <span className="font-medium">{originalLabel}</span>
        <span className="text-muted-foreground/60">↔</span>
        <span className="font-medium">{modifiedLabel}</span>
        {diffChanges.length > 0 && (
          <>
            <span className="ml-2 font-mono text-green-600 dark:text-green-400">
              +{additions}
            </span>
            <span className="font-mono text-red-600 dark:text-red-400">
              -{deletions}
            </span>
            <span>
              {diffChanges.length}{" "}
              {diffChanges.length === 1 ? "change" : "changes"}
            </span>
            <div className="ml-auto flex items-center gap-1">
              <button
                type="button"
                onClick={handlePrevChange}
                disabled={currentChangeIndex <= 0}
                className="rounded border border-border bg-background px-2 py-0.5 text-[10px] disabled:opacity-40 hover:bg-muted transition-colors inline-flex items-center gap-1"
              >
                <ChevronLeft className="h-3 w-3" />
                Prev
              </button>
              <span className="tabular-nums text-[10px]">
                {currentChangeIndex + 1} / {diffChanges.length}
              </span>
              <button
                type="button"
                onClick={handleNextChange}
                disabled={currentChangeIndex >= diffChanges.length - 1}
                className="rounded border border-border bg-background px-2 py-0.5 text-[10px] disabled:opacity-40 hover:bg-muted transition-colors inline-flex items-center gap-1"
              >
                Next
                <ChevronRight className="h-3 w-3" />
              </button>
            </div>
          </>
        )}
      </div>
      <div className="min-h-0 flex-1">
        <MonacoDiffEditor
          original={original}
          modified={modified}
          language={language}
          theme={editorTheme}
          keepCurrentOriginalModel
          keepCurrentModifiedModel
          beforeMount={defineMonacoThemes}
          onMount={handleEditorMount}
          loading={
            <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
              Loading diff viewer...
            </div>
          }
          options={{
            readOnly: true,
            renderSideBySide: true,
            renderSideBySideInlineBreakpoint: 0,
            automaticLayout: true,
            fontSize: getCodeFontSize(),
            minimap: { enabled: false },
            scrollBeyondLastLine: false,
            renderOverviewRuler: false,
            ignoreTrimWhitespace: true,
            renderIndicators: true,
            originalEditable: false,
          }}
        />
      </div>
    </div>
  )
}
