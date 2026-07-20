"use client"

import { useEffect, useRef, useState } from "react"
import { isDesktop } from "@/lib/platform"
import { useTranslations } from "next-intl"
import { usePlatform } from "@/hooks/use-platform"
import { cn } from "@/lib/utils"

async function getTauriWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  return getCurrentWindow()
}

/** Combined width of the three control buttons (3 × 46px). Exported so the
 *  Linux resize grips can carve this region out of the top edge. */
export const WINDOW_CONTROLS_WIDTH = 138

export function WindowControls() {
  const t = useTranslations("Folder.windowControls")
  const { isWindows, isLinux } = usePlatform()
  const showControls = isWindows || isLinux
  const [isMaximized, setIsMaximized] = useState(false)
  const appWindowRef = useRef<Awaited<
    ReturnType<typeof getTauriWindow>
  > | null>(null)

  useEffect(() => {
    if (!showControls || !isDesktop()) return

    let disposed = false
    let unlistenResize: (() => void) | null = null
    let resizeFrame: number | null = null

    getTauriWindow().then((appWindow) => {
      if (disposed) return
      appWindowRef.current = appWindow

      const syncMaximized = async () => {
        try {
          const maximized = await appWindow.isMaximized()
          if (!disposed) setIsMaximized(maximized)
        } catch {
          if (!disposed) setIsMaximized(false)
        }
      }

      const scheduleSync = () => {
        if (resizeFrame !== null) return
        resizeFrame = window.requestAnimationFrame(() => {
          resizeFrame = null
          void syncMaximized()
        })
      }

      void syncMaximized()

      appWindow
        .onResized(() => scheduleSync())
        .then((unlisten) => {
          unlistenResize = unlisten
        })
        .catch(() => {
          unlistenResize = null
        })
    })

    return () => {
      disposed = true
      if (resizeFrame !== null) {
        window.cancelAnimationFrame(resizeFrame)
      }
      unlistenResize?.()
    }
  }, [showControls])

  if (!showControls || !isDesktop()) return null

  return (
    <div className="flex h-full items-stretch [-webkit-app-region:no-drag]">
      <button
        type="button"
        className={buttonClass}
        onClick={() => {
          appWindowRef.current?.minimize().catch((err: unknown) => {
            console.error("[WindowControls] failed to minimize:", err)
          })
        }}
        aria-label={t("minimizeWindow")}
        title={t("minimize")}
      >
        <MinimizeIcon />
      </button>
      <button
        type="button"
        className={buttonClass}
        onClick={() => {
          appWindowRef.current?.toggleMaximize().catch((err: unknown) => {
            console.error("[WindowControls] failed to toggle maximize:", err)
          })
        }}
        aria-label={t(isMaximized ? "restoreWindow" : "maximizeWindow")}
        title={t(isMaximized ? "restore" : "maximize")}
      >
        {isMaximized ? <RestoreIcon /> : <MaximizeIcon />}
      </button>
      <button
        type="button"
        className={cn(
          buttonClass,
          "hover:bg-[#e81123] hover:text-white active:bg-[#c50f1f] active:text-white"
        )}
        onClick={() => {
          appWindowRef.current?.close().catch((err: unknown) => {
            console.error("[WindowControls] failed to close:", err)
          })
        }}
        aria-label={t("closeWindow")}
        title={t("close")}
      >
        <CloseIcon />
      </button>
    </div>
  )
}

const buttonClass =
  "flex h-full w-[46px] items-center justify-center text-foreground/85 transition-colors duration-75 hover:bg-foreground/10 active:bg-foreground/15"

function MinimizeIcon() {
  return (
    <span
      aria-hidden
      className="inline-block h-px w-[10px] translate-y-[2px] bg-current"
    />
  )
}

function MaximizeIcon() {
  return (
    <span
      aria-hidden
      className="inline-block h-[10px] w-[10px] border border-current"
    />
  )
}

function RestoreIcon() {
  return (
    <span aria-hidden className="relative inline-block h-[10px] w-[10px]">
      <span className="absolute right-0 top-0 h-[7px] w-[7px] border border-current" />
      <span className="absolute bottom-0 left-0 h-[7px] w-[7px] border border-current" />
    </span>
  )
}

function CloseIcon() {
  return (
    <span aria-hidden className="relative inline-block h-[10px] w-[10px]">
      <span className="absolute left-1/2 top-0 h-[10px] w-px -translate-x-1/2 rotate-45 bg-current" />
      <span className="absolute left-1/2 top-0 h-[10px] w-px -translate-x-1/2 -rotate-45 bg-current" />
    </span>
  )
}
