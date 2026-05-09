// Sprite-sheet animation constants. Locked to the Codex `/pet` format so a
// `~/.codex/pets/<id>/spritesheet.webp` (or codeg's own
// `~/.codeg/pets/<id>/spritesheet.webp`) renders identically.
//
// Source of truth: openai/skills hatch-pet `animation-rows.md`.

export const SPRITE_SHEET_WIDTH = 1536
export const SPRITE_SHEET_HEIGHT = 1872
export const SPRITE_GRID_COLS = 8
export const SPRITE_GRID_ROWS = 9
export const SPRITE_FRAME_WIDTH = SPRITE_SHEET_WIDTH / SPRITE_GRID_COLS // 192
export const SPRITE_FRAME_HEIGHT = SPRITE_SHEET_HEIGHT / SPRITE_GRID_ROWS // 208

// Server-emitted PetState (see Rust `PetState` enum, snake_case JSON).
export type PetState =
  | "idle"
  | "running_right"
  | "running_left"
  | "waving"
  | "jumping"
  | "failed"
  | "waiting"
  | "running"
  | "review"

// Row index (0..=8) in the sprite sheet for each state, top-to-bottom.
export const PET_STATE_ROW: Record<PetState, number> = {
  idle: 0,
  running_right: 1,
  running_left: 2,
  waving: 3,
  jumping: 4,
  failed: 5,
  waiting: 6,
  running: 7,
  review: 8,
}

// Per-frame durations in milliseconds, indexed in column order. The number of
// entries also implies the frame count for that row — extra columns in the
// sheet are blank.
export const PET_FRAME_DURATIONS_MS: Record<PetState, number[]> = {
  idle: [280, 110, 110, 140, 140, 320],
  running_right: [120, 120, 120, 120, 120, 120, 120, 220],
  running_left: [120, 120, 120, 120, 120, 120, 120, 220],
  waving: [240, 240, 240, 240],
  jumping: [280, 240, 200, 200, 160],
  failed: [140, 140, 140, 140, 140, 140, 140, 280],
  waiting: [240, 240, 240, 240, 240, 240],
  running: [120, 120, 120, 120, 120, 120],
  review: [200, 200, 200, 200, 200, 200],
}

// CSS background-position for a (row, col) cell. Uses the
// "(n / (count-1)) * 100%" form because that's how `background-size: 800% 900%`
// computes positioning — `0% .. 100%` traverses cells `0..(count-1)`.
export function backgroundPositionFor(row: number, col: number): string {
  const x = (col / (SPRITE_GRID_COLS - 1)) * 100
  const y = (row / (SPRITE_GRID_ROWS - 1)) * 100
  return `${x}% ${y}%`
}

export const SPRITE_BACKGROUND_SIZE = `${SPRITE_GRID_COLS * 100}% ${SPRITE_GRID_ROWS * 100}%`

// Tunable: how often to randomly insert a flourish (waving / jumping) when
// the pet is idle. Avoids the pet looking statue-still during long idle periods.
export const IDLE_FLOURISH_MIN_MS = 8_000
export const IDLE_FLOURISH_MAX_MS = 15_000
export const IDLE_FLOURISH_OPTIONS: readonly PetState[] = [
  "waving",
  "jumping",
] as const
