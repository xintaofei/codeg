"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2 } from "lucide-react"
import { getPet, getPetSettings, readPetSpritesheet } from "@/lib/pet/api"
import type { PetDetail, PetSpriteAsset } from "@/lib/pet/types"
import { disposeTauriListener } from "@/lib/tauri-listener"
import { isDesktop } from "@/lib/transport"
import { PET_FRAME_DURATIONS_MS, type PetState } from "@/lib/pet/animation"
import { usePetState } from "../_hooks/usePetState"
import { usePetDrag } from "../_hooks/usePetDrag"
import { PetSprite } from "./PetSprite"
import { PetMenu } from "./PetMenu"

export interface PetWindowProps {
  petId: string
}

// Hover/click animations loop this many times before resolving back to the
// agent state. The animator naturally chains non-idle states back to col 0,
// so we just hold the state for N × single-cycle duration. The +80ms slack
// covers tick-rounding in the JS animator so we don't cut the last frame.
const INTERACTION_LOOPS = 3
const INTERACTION_SLACK_MS = 80
const JUMPING_DURATION_MS =
  sumDurations("jumping") * INTERACTION_LOOPS + INTERACTION_SLACK_MS
const WAVING_DURATION_MS =
  sumDurations("waving") * INTERACTION_LOOPS + INTERACTION_SLACK_MS
const PET_HOVER_ENTER_EVENT = "pet://hover-enter"
const PET_HOVER_LEAVE_EVENT = "pet://hover-leave"

function sumDurations(state: PetState): number {
  return PET_FRAME_DURATIONS_MS[state].reduce((acc, d) => acc + d, 0)
}

export function PetWindow({ petId }: PetWindowProps) {
  const t = useTranslations("Pet")
  const [pet, setPet] = useState<PetDetail | null>(null)
  const [asset, setAsset] = useState<PetSpriteAsset | null>(null)
  const [scale, setScale] = useState<number>(1)
  const [error, setError] = useState<string | null>(null)
  const agentState = usePetState()

  // Interaction-driven state takes priority over the agent-driven state so
  // a drag, hover, or click immediately wins over the ambient ACP animation.
  // The override is cleared either by the drag-idle timer (held still during
  // drag) or by the post-action timeout (after waving/jumping finishes).
  const [interactionState, setInteractionState] = useState<PetState | null>(
    null
  )
  const interactionTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const pointerDownRef = useRef(false)

  const handleDragDirection = useCallback((s: PetState | null) => {
    if (interactionTimerRef.current) {
      clearTimeout(interactionTimerRef.current)
      interactionTimerRef.current = null
    }
    setInteractionState(s)
  }, [])

  const playOneShot = useCallback((state: PetState, durationMs: number) => {
    if (interactionTimerRef.current) clearTimeout(interactionTimerRef.current)
    setInteractionState(state)
    interactionTimerRef.current = setTimeout(() => {
      setInteractionState(null)
      interactionTimerRef.current = null
    }, durationMs)
  }, [])

  const cancelInteraction = useCallback(() => {
    handleDragDirection(null)
  }, [handleDragDirection])

  const handleClick = useCallback(() => {
    playOneShot("jumping", JUMPING_DURATION_MS)
  }, [playOneShot])

  // Track held-mouse-button state so hover-driven waving stays out of the
  // way of any active interaction (drag, click-and-hold). Listening on
  // `window` rather than the root div catches pointerup even when it
  // happens off-window mid-drag.
  useEffect(() => {
    const onDown = () => {
      pointerDownRef.current = true
    }
    const onUp = () => {
      pointerDownRef.current = false
    }
    window.addEventListener("pointerdown", onDown)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", onUp)
    return () => {
      window.removeEventListener("pointerdown", onDown)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", onUp)
    }
  }, [])

  // Hover detection runs in Rust (`spawn_pet_hover_watcher` polls the
  // global cursor position and emits enter/leave events). Going through
  // the OS window event system from JS is unreliable when the pet isn't
  // the key window, so we listen for the backend events instead. Leaving
  // the window cancels any in-flight one-shot so the pet returns to its
  // ambient state immediately.
  useEffect(() => {
    if (!isDesktop()) return
    let unlistenEnter: (() => void) | null = null
    let unlistenLeave: (() => void) | null = null
    let cancelled = false
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event")
        const [offEnter, offLeave] = await Promise.all([
          listen(PET_HOVER_ENTER_EVENT, () => {
            if (cancelled || pointerDownRef.current) return
            playOneShot("waving", WAVING_DURATION_MS)
          }),
          listen(PET_HOVER_LEAVE_EVENT, () => {
            if (cancelled || pointerDownRef.current) return
            cancelInteraction()
          }),
        ])
        if (cancelled) {
          disposeTauriListener(offEnter, "Pet")
          disposeTauriListener(offLeave, "Pet")
        } else {
          unlistenEnter = offEnter
          unlistenLeave = offLeave
        }
      } catch (err) {
        console.warn("[Pet] hover subscription failed:", err)
      }
    })()
    return () => {
      cancelled = true
      disposeTauriListener(unlistenEnter, "Pet")
      disposeTauriListener(unlistenLeave, "Pet")
    }
  }, [playOneShot, cancelInteraction])

  useEffect(() => {
    return () => {
      if (interactionTimerRef.current) clearTimeout(interactionTimerRef.current)
    }
  }, [])

  const drag = usePetDrag({
    onDragDirection: handleDragDirection,
    onClick: handleClick,
  })

  const renderState: PetState = interactionState ?? agentState

  useEffect(() => {
    let cancelled = false
    setError(null)

    async function load() {
      try {
        const [detail, sprite, config] = await Promise.all([
          getPet(petId),
          readPetSpritesheet(petId),
          getPetSettings(),
        ])
        if (cancelled) return
        setPet(detail)
        setAsset(sprite)
        setScale(config.scale ?? 1)
      } catch (err) {
        if (!cancelled) setError(toMessage(err))
      }
    }

    void load()
    return () => {
      cancelled = true
    }
  }, [petId])

  // Keep the document title clean. macOS hides it via title_bar_style anyway,
  // but server-mode preview shows it.
  useEffect(() => {
    document.title = pet ? `${pet.displayName} - codeg pet` : "codeg pet"
  }, [pet])

  // Fully transparent body so the OS chrome is invisible. Done in JS to keep
  // the global stylesheet untouched.
  useEffect(() => {
    const prevBg = document.body.style.background
    const prevHtmlBg = document.documentElement.style.background
    document.body.style.background = "transparent"
    document.documentElement.style.background = "transparent"
    document.body.classList.add("pet-body")
    return () => {
      document.body.style.background = prevBg
      document.documentElement.style.background = prevHtmlBg
      document.body.classList.remove("pet-body")
    }
  }, [])

  const openManager = () => {
    if (!isDesktop()) return
    void (async () => {
      try {
        const { getTransport } = await import("@/lib/transport")
        await getTransport().call("open_settings_window", {
          section: "appearance",
        })
      } catch (err) {
        console.warn("[Pet] failed to open manager:", err)
      }
    })()
  }

  if (error) {
    return (
      <div
        className="flex h-screen w-screen items-center justify-center text-xs text-destructive"
        style={{ background: "transparent" }}
        title={error}
      >
        {t("loadError")}
      </div>
    )
  }

  if (!pet || !asset) {
    return (
      <div
        className="flex h-screen w-screen items-center justify-center"
        style={{ background: "transparent" }}
      >
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
    )
  }

  const dataUrl = `data:${asset.mime};base64,${asset.dataBase64}`

  return (
    <div
      className="relative flex h-screen w-screen select-none items-center justify-center"
      style={{ background: "transparent" }}
      onPointerDown={drag.onPointerDown}
    >
      <PetSprite
        spritesheetDataUrl={dataUrl}
        state={renderState}
        scale={scale}
        label={pet.displayName}
      />
      <PetMenu
        scale={scale}
        onScaleChange={setScale}
        onOpenSettings={openManager}
      />
    </div>
  )
}

function toMessage(err: unknown): string {
  if (err instanceof Error) return err.message
  if (typeof err === "string") return err
  if (err && typeof err === "object" && "message" in err) {
    const m = (err as { message: unknown }).message
    if (typeof m === "string") return m
  }
  return String(err)
}
