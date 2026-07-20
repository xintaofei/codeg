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
  idle: [1680, 660, 660, 840, 840, 1920],
  running_right: [120, 120, 120, 120, 120, 120, 120, 220],
  running_left: [120, 120, 120, 120, 120, 120, 120, 220],
  waving: [140, 140, 140, 280],
  jumping: [140, 140, 140, 140, 280],
  failed: [140, 140, 140, 140, 140, 140, 140, 240],
  waiting: [150, 150, 150, 150, 150, 260],
  running: [120, 120, 120, 120, 120, 220],
  review: [150, 150, 150, 150, 150, 280],
}

// Derive the number of animation rows from a sheet's natural pixel height.
// The frame cell is a constant 192×208, so rows = height / 208. Older pets are
// 9 rows (1872px); v2 marketplace pets are 11 (2288px). We never report fewer
// than the 9 base states, so a blank/mis-measured image still maps the known
// states to valid rows.
export function spriteRowsFromHeight(
  naturalHeight: number | null | undefined
): number {
  if (
    naturalHeight == null ||
    !Number.isFinite(naturalHeight) ||
    naturalHeight <= 0
  ) {
    return SPRITE_GRID_ROWS
  }
  const rows = Math.round(naturalHeight / SPRITE_FRAME_HEIGHT)
  return Math.max(SPRITE_GRID_ROWS, rows)
}

// CSS `background-size` for a sheet with `rows` rows (columns fixed at 8).
// Pair with `backgroundPositionFor(_, _, rows)` — both must agree on the row
// count or frames land off-grid.
export function spriteBackgroundSize(rows: number = SPRITE_GRID_ROWS): string {
  const safeRows = Math.max(1, Math.floor(rows))
  return `${SPRITE_GRID_COLS * 100}% ${safeRows * 100}%`
}

// CSS background-position for a (row, col) cell. Uses the
// "(n / (count-1)) * 100%" form because that's how `background-size` percentages
// compute positioning — `0% .. 100%` traverses cells `0..(count-1)`. `rows`
// defaults to the base 9-row layout for callers rendering a legacy sheet.
export function backgroundPositionFor(
  row: number,
  col: number,
  rows: number = SPRITE_GRID_ROWS
): string {
  const x = SPRITE_GRID_COLS > 1 ? (col / (SPRITE_GRID_COLS - 1)) * 100 : 0
  const y = rows > 1 ? (row / (rows - 1)) * 100 : 0
  return `${x}% ${y}%`
}

// Number of frames in a horizontal preview filmstrip of natural size
// `width`×`height`. Each filmstrip frame keeps the 192:208 cell aspect, so
// frameWidth = height × (192/208) and frames = width / frameWidth. v1 strips
// hold 57 frames (the 9 base states); v2 strips hold 73 (two appended states).
// Returns 0 when the size is unknown so callers can fall back to a default.
export function filmstripFrameCount(width: number, height: number): number {
  if (
    !Number.isFinite(width) ||
    !Number.isFinite(height) ||
    width <= 0 ||
    height <= 0
  ) {
    return 0
  }
  const frameWidth = height * (SPRITE_FRAME_WIDTH / SPRITE_FRAME_HEIGHT)
  if (frameWidth <= 0) return 0
  return Math.round(width / frameWidth)
}

// Tunable: how often to randomly insert a flourish (waving / jumping) when
// the pet is idle. Avoids the pet looking statue-still during long idle periods.
export const IDLE_FLOURISH_MIN_MS = 8_000
export const IDLE_FLOURISH_MAX_MS = 15_000
export const IDLE_FLOURISH_OPTIONS: readonly PetState[] = [
  "waving",
  "jumping",
] as const

export const PET_ONESHOT_KINDS = [
  "jumping",
  "waving",
  "failed",
  "review",
] as const
export type PetOneShotKind = (typeof PET_ONESHOT_KINDS)[number]

// Backend-driven one-shot animations (turn_complete, git commit/push,
// merge abort, agent install, conversation entering PendingReview, manual
// `pet_celebrate`). Sized to play a few full loops so the user actually
// registers the cue, with `failed` kept short to avoid lingering on a
// frowning sprite.
export const PET_ONESHOT_LOOPS: Record<PetOneShotKind, number> = {
  jumping: 3,
  waving: 3,
  failed: 2,
  review: 3,
}
