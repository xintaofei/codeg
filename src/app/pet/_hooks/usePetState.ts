"use client"

import { useEffect, useState } from "react"
import { isDesktop } from "@/lib/transport"
import type { PetState } from "@/lib/pet/animation"

const PET_STATE_EVENT = "pet://state"

export function usePetState(initial: PetState = "idle"): PetState {
  const [state, setState] = useState<PetState>(initial)

  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false

    async function subscribe() {
      try {
        if (isDesktop()) {
          const { listen } = await import("@tauri-apps/api/event")
          const off = await listen<PetState>(PET_STATE_EVENT, (event) => {
            if (cancelled) return
            const next = normalize(event.payload)
            if (next) setState(next)
          })
          if (cancelled) {
            off()
          } else {
            unlisten = off
          }
        } else {
          const { getTransport } = await import("@/lib/transport")
          const off = await getTransport().subscribe<PetState>(
            PET_STATE_EVENT,
            (payload) => {
              if (cancelled) return
              const next = normalize(payload)
              if (next) setState(next)
            }
          )
          if (cancelled) {
            off()
          } else {
            unlisten = off
          }
        }
      } catch (err) {
        // Subscription failures are non-fatal — pet just stays in `idle`.
        console.warn("[Pet] state subscription failed:", err)
      }
    }

    void subscribe()

    return () => {
      cancelled = true
      if (unlisten) {
        unlisten()
      }
    }
  }, [])

  return state
}

function normalize(payload: unknown): PetState | null {
  if (typeof payload === "string") {
    return payload as PetState
  }
  if (
    payload &&
    typeof payload === "object" &&
    "payload" in (payload as Record<string, unknown>)
  ) {
    const inner = (payload as { payload: unknown }).payload
    if (typeof inner === "string") return inner as PetState
  }
  return null
}
