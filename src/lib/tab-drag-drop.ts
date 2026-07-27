/**
 * Pure math for cross-group tab drag & drop. The DOM side (hit-testing the
 * strip/shell under the pointer) lives in the tab bar; this module only turns
 * measured geometry into a drop index and normalizes pointer coordinates, so
 * both are unit-testable without a DOM.
 */

/**
 * Insertion index for a drop at `clientX` given the ascending midpoints of the
 * target strip's tabs: the tab lands before the first tab whose midpoint is to
 * the right of the cursor. (The app renders LTR in every locale, so a plain
 * left-to-right count is correct.)
 */
export function dropIndexFromMidpoints(
  clientX: number,
  midpoints: number[]
): number {
  let index = 0
  for (const mid of midpoints) {
    if (mid < clientX) index += 1
  }
  return index
}

export interface DragClientPoint {
  x: number
  y: number
}

/**
 * Client (viewport) coordinates of a motion drag event. Motion hands back the
 * original event plus `PanInfo` whose `point` is in PAGE coordinates — prefer
 * the event's own client coords (pointer/mouse), fall back to page − scroll
 * (touch events carry coords per touch, not on the event itself).
 */
export function clientPointFromDrag(
  event: unknown,
  info: { point: { x: number; y: number } }
): DragClientPoint {
  const maybe = event as { clientX?: unknown; clientY?: unknown } | null
  if (
    maybe != null &&
    typeof maybe.clientX === "number" &&
    typeof maybe.clientY === "number"
  ) {
    return { x: maybe.clientX, y: maybe.clientY }
  }
  return {
    x: info.point.x - (typeof window !== "undefined" ? window.scrollX : 0),
    y: info.point.y - (typeof window !== "undefined" ? window.scrollY : 0),
  }
}
