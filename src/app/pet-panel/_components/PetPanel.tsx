"use client"

import { useEffect, useMemo, useRef } from "react"
import { useTranslations } from "next-intl"
import { AnimatePresence, motion } from "motion/react"
import { Inbox, X } from "lucide-react"
import { closePetPanel, resizePetPanel } from "@/lib/pet/api"
import { isDesktop } from "@/lib/transport"
import { usePetSessions } from "../../pet/_hooks/usePetSessions"
import { sessionSortRank } from "@/lib/pet/session-display"
import { SessionRow } from "./SessionRow"

// Cap the scrollable list so a long session list doesn't grow the window past a
// sane height; beyond this the list scrolls. The window itself is sized to fit
// content (header + this list), so few/no sessions leave no dead space.
const LIST_MAX_HEIGHT_PX = 320

export function PetPanel() {
  const t = useTranslations("Pet")
  const { sessions } = usePetSessions()
  const wrapperRef = useRef<HTMLDivElement>(null)

  const sorted = useMemo(
    () => [...sessions].sort((a, b) => sessionSortRank(a) - sessionSortRank(b)),
    [sessions]
  )

  // Esc dismisses, matching the click-away (blur) behaviour.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        void closePetPanel().catch(() => {})
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])

  // Fit the OS window to the rendered content. The wrapper sizes to its content
  // (no forced fill), so its box height — including the `p-2` chrome — is the
  // height the window should be. A ResizeObserver catches every content change
  // (sessions loading, a permission card expanding/collapsing), rAF-coalesced
  // and integer-deduped so a burst nets one resize and we never re-fire on our
  // own resize (which only changes window height, not the content's). 1 CSS px
  // == 1 logical px, exactly what the backend `set_size` expects — so we send
  // the measured value directly, with NO devicePixelRatio multiply.
  useEffect(() => {
    if (!isDesktop()) return
    const el = wrapperRef.current
    if (!el) return

    let raf = 0
    let lastSent = -1

    const measure = () => {
      raf = 0
      const h = Math.ceil(el.getBoundingClientRect().height)
      if (h <= 0 || h === lastSent) return
      lastSent = h
      void resizePetPanel(h).catch(() => {})
    }
    const schedule = () => {
      if (raf) return
      raf = requestAnimationFrame(measure)
    }

    schedule()
    const ro = new ResizeObserver(schedule)
    ro.observe(el)

    return () => {
      ro.disconnect()
      if (raf) cancelAnimationFrame(raf)
    }
  }, [])

  return (
    <div
      ref={wrapperRef}
      className="flex w-screen flex-col border border-border"
    >
      <header className="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
        <div className="text-sm font-semibold">
          {t("panel.title")}
          {sorted.length > 0 ? (
            <span className="ml-1 font-normal text-muted-foreground">
              ({sorted.length})
            </span>
          ) : null}
        </div>
        <button
          type="button"
          aria-label={t("menu.close")}
          onClick={() => void closePetPanel().catch(() => {})}
          className="rounded p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </header>

      {sorted.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-2 px-6 py-8 text-center">
          <Inbox className="h-8 w-8 text-muted-foreground/50" />
          <div className="text-sm font-medium">{t("panel.empty")}</div>
          <div className="text-xs text-muted-foreground">
            {t("panel.emptyHint")}
          </div>
        </div>
      ) : (
        <ul
          className="overflow-y-auto py-1"
          style={{ maxHeight: LIST_MAX_HEIGHT_PX }}
        >
          <AnimatePresence initial={false}>
            {sorted.map((session) => (
              <motion.li
                key={session.connectionId}
                layout
                exit={{ opacity: 0, height: 0 }}
                transition={{ duration: 0.2, ease: "easeOut" }}
                style={{ overflow: "hidden" }}
              >
                <SessionRow session={session} />
              </motion.li>
            ))}
          </AnimatePresence>
        </ul>
      )}
    </div>
  )
}
