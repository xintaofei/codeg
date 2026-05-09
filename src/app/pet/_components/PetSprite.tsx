"use client"

import {
  SPRITE_BACKGROUND_SIZE,
  backgroundPositionFor,
  type PetState,
} from "@/lib/pet/animation"
import { usePetAnimator } from "../_hooks/usePetAnimator"

export interface PetSpriteProps {
  spritesheetDataUrl: string
  state: PetState
  scale: number
  /** Aria-label for screen readers. */
  label: string
}

const FRAME_WIDTH = 192
const FRAME_HEIGHT = 208

export function PetSprite({
  spritesheetDataUrl,
  state,
  scale,
  label,
}: PetSpriteProps) {
  const { row, col } = usePetAnimator(state)

  return (
    <div
      role="img"
      aria-label={label}
      style={{
        width: `${FRAME_WIDTH * scale}px`,
        height: `${FRAME_HEIGHT * scale}px`,
        backgroundImage: `url("${spritesheetDataUrl}")`,
        backgroundRepeat: "no-repeat",
        backgroundSize: SPRITE_BACKGROUND_SIZE,
        backgroundPosition: backgroundPositionFor(row, col),
        imageRendering: "pixelated",
      }}
    />
  )
}
