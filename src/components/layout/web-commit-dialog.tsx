"use client"

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type SyntheticEvent,
} from "react"
import { LoaderCircle } from "lucide-react"
import { useTranslations } from "next-intl"
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog"
import {
  CLOSE_COMMIT_DIALOG_EVENT,
  OPEN_COMMIT_DIALOG_EVENT,
  type OpenCommitDialogDetail,
} from "@/lib/commit-dialog-events"

function isCommitPath(path: string): boolean {
  try {
    const url = new URL(path, window.location.origin)
    const pathname = url.pathname
      .replace(/\/index\.html$/, "")
      .replace(/\.html$/, "")
      .replace(/\/+$/, "")
    return url.origin === window.location.origin && pathname === "/commit"
  } catch {
    return false
  }
}

export function WebCommitDialog() {
  const t = useTranslations("CommitPage")
  const frameSequence = useRef(0)
  const [frame, setFrame] = useState<{
    path: string
    sequence: number
  } | null>(null)
  const [loaded, setLoaded] = useState(false)

  const closeDialog = useCallback(() => {
    setFrame(null)
    setLoaded(false)
  }, [])

  useEffect(() => {
    const handleOpenCommit = (event: Event) => {
      const nextPath = (event as CustomEvent<OpenCommitDialogDetail>).detail
        ?.path
      if (!nextPath || !isCommitPath(nextPath)) return

      setLoaded(false)
      frameSequence.current += 1
      setFrame({ path: nextPath, sequence: frameSequence.current })
    }

    window.addEventListener(OPEN_COMMIT_DIALOG_EVENT, handleOpenCommit)
    window.addEventListener(CLOSE_COMMIT_DIALOG_EVENT, closeDialog)
    return () => {
      window.removeEventListener(OPEN_COMMIT_DIALOG_EVENT, handleOpenCommit)
      window.removeEventListener(CLOSE_COMMIT_DIALOG_EVENT, closeDialog)
    }
  }, [closeDialog])

  const handleOpenChange = useCallback(
    (open: boolean) => {
      if (!open) closeDialog()
    },
    [closeDialog]
  )

  const handleFrameKeyDown = useCallback(
    (event: KeyboardEvent) => {
      if (event.key === "Escape") closeDialog()
    },
    [closeDialog]
  )

  const handleFrameLoad = useCallback(
    (event: SyntheticEvent<HTMLIFrameElement>) => {
      setLoaded(true)
      try {
        event.currentTarget.contentWindow?.addEventListener(
          "keydown",
          handleFrameKeyDown
        )
      } catch {
        // The commit route is expected to be same-origin. The close button
        // remains available if a deployment rewrites it to another origin.
      }
    },
    [handleFrameKeyDown]
  )

  return (
    <Dialog open={frame !== null} onOpenChange={handleOpenChange}>
      <DialogContent
        className="block h-[min(820px,calc(100dvh-2rem))] w-[min(1220px,calc(100vw-2rem))] max-w-none overflow-hidden rounded-2xl p-0 sm:max-w-none"
        closeButtonClassName="top-2 right-2 z-20 size-6 bg-background/80 backdrop-blur-sm [&_svg]:size-3"
      >
        <DialogTitle className="sr-only">{t("title")}</DialogTitle>

        {!loaded && (
          <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center bg-background">
            <LoaderCircle
              className="h-6 w-6 animate-spin text-muted-foreground"
              aria-hidden="true"
            />
          </div>
        )}

        {frame && (
          <iframe
            key={frame.sequence}
            src={frame.path}
            title={t("title")}
            className="h-full w-full border-0 bg-background"
            onLoad={handleFrameLoad}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}
