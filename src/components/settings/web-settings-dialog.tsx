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
  OPEN_SETTINGS_DIALOG_EVENT,
  type OpenSettingsDialogDetail,
} from "@/lib/settings-dialog-events"

export function WebSettingsDialog() {
  const t = useTranslations("SettingsShell")
  const frameSequence = useRef(0)
  const [frame, setFrame] = useState<{
    path: string
    sequence: number
  } | null>(null)
  const [loaded, setLoaded] = useState(false)

  useEffect(() => {
    const handleOpenSettings = (event: Event) => {
      const nextPath = (event as CustomEvent<OpenSettingsDialogDetail>).detail
        ?.path
      if (!nextPath?.startsWith("/settings/")) return

      setLoaded(false)
      frameSequence.current += 1
      setFrame({ path: nextPath, sequence: frameSequence.current })
    }

    window.addEventListener(OPEN_SETTINGS_DIALOG_EVENT, handleOpenSettings)
    return () => {
      window.removeEventListener(OPEN_SETTINGS_DIALOG_EVENT, handleOpenSettings)
    }
  }, [])

  const handleOpenChange = useCallback((open: boolean) => {
    if (open) return
    setFrame(null)
    setLoaded(false)
  }, [])

  const handleFrameKeyDown = useCallback(
    (event: KeyboardEvent) => {
      if (event.key === "Escape") handleOpenChange(false)
    },
    [handleOpenChange]
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
        // The settings route is expected to be same-origin. Keep the close
        // button usable if a deployment rewrites it to another origin.
      }
    },
    [handleFrameKeyDown]
  )

  return (
    <Dialog open={frame !== null} onOpenChange={handleOpenChange}>
      <DialogContent
        className="block h-[min(700px,calc(100dvh-2rem))] w-[min(1080px,calc(100vw-2rem))] max-w-none overflow-hidden rounded-2xl p-0 sm:max-w-none"
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
