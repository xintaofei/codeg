"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { closePetWindow, savePetWindowState } from "@/lib/pet/api"

export interface PetMenuProps {
  scale: number
  onScaleChange: (scale: number) => void
  onOpenSettings: () => void
}

const SCALE_STEPS: ReadonlyArray<number> = [0.5, 1, 1.5, 2]

export function PetMenu({
  scale,
  onScaleChange,
  onOpenSettings,
}: PetMenuProps) {
  const t = useTranslations("Pet")
  const [open, setOpen] = useState(false)
  const [position, setPosition] = useState<{ x: number; y: number } | null>(
    null
  )
  const containerRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    function onContextMenu(e: MouseEvent) {
      e.preventDefault()
      setPosition({ x: e.clientX, y: e.clientY })
      setOpen(true)
    }
    function onClickOutside(e: MouseEvent) {
      if (
        containerRef.current &&
        !containerRef.current.contains(e.target as Node)
      ) {
        setOpen(false)
      }
    }
    document.addEventListener("contextmenu", onContextMenu)
    document.addEventListener("mousedown", onClickOutside)
    return () => {
      document.removeEventListener("contextmenu", onContextMenu)
      document.removeEventListener("mousedown", onClickOutside)
    }
  }, [])

  if (!open || !position) return null

  return (
    <div
      ref={containerRef}
      className="pointer-events-auto fixed z-50 min-w-44 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-lg"
      style={{ left: position.x, top: position.y }}
    >
      <div className="px-2 pt-1 pb-1 text-xs text-muted-foreground">
        {t("menu.scale")}
      </div>
      <div className="flex gap-1 px-2 pb-2">
        {SCALE_STEPS.map((step) => (
          <button
            key={step}
            type="button"
            className={`rounded border px-2 py-0.5 text-xs ${
              Math.abs(step - scale) < 0.01
                ? "border-primary bg-primary/10 text-primary"
                : "border-border text-foreground hover:bg-accent"
            }`}
            onClick={async () => {
              try {
                await savePetWindowState({ scale: step })
                onScaleChange(step)
              } finally {
                setOpen(false)
              }
            }}
          >
            {step}×
          </button>
        ))}
      </div>
      <div className="my-1 h-px bg-border" />
      <button
        type="button"
        className="w-full rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
        onClick={() => {
          setOpen(false)
          onOpenSettings()
        }}
      >
        {t("menu.openManager")}
      </button>
      <button
        type="button"
        className="w-full rounded px-2 py-1.5 text-left text-sm text-destructive hover:bg-destructive/10"
        onClick={async () => {
          setOpen(false)
          try {
            await closePetWindow()
          } catch (err) {
            console.warn("[Pet] close failed:", err)
          }
        }}
      >
        {t("menu.close")}
      </button>
    </div>
  )
}
