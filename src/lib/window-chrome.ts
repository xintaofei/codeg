/**
 * Shared geometry for the desktop window-chrome corner overlays.
 *
 * The sidebar toggle / remote (top-left) and terminal / aux / settings
 * (top-right) clusters are pinned to fixed overlays at the window's corners so
 * they never move — and never re-mount — when the side panels open or close
 * (that re-parenting is what used to make them flicker). Because the overlays
 * float ABOVE whichever column owns that edge, the column must reserve exactly
 * the overlay's width so its tabs / content never render underneath. Keeping the
 * overlay width and the column reservation in one place guarantees they agree.
 */

/**
 * Clearance for the native macOS traffic lights, which float over the window's
 * top-left corner (nudged to Y=21 for the h-10 bar in the Rust window config).
 * The left cluster sits to their right.
 */
export const MAC_TRAFFIC_LIGHT_INSET = 76

/**
 * Windows/Linux caption buttons (min / max / close) occupy the window's
 * top-right corner. Mirrors `WINDOW_CONTROLS_WIDTH` in window-controls.tsx; the
 * right cluster sits to their left.
 */
export const WINDOW_CAPTION_WIDTH = 138

/** Left cluster: sidebar toggle + remote (two icon buttons + padding). */
export const LEFT_CHROME_CLUSTER = 80

/** Right cluster: terminal + aux + settings (three icon buttons + padding). */
export const RIGHT_CHROME_CLUSTER = 116

/**
 * Width the window's left-edge column reserves for the left overlay.
 * `macInset` adds the traffic-light clearance (desktop macOS only).
 */
export function leftChromeReserve(macInset: boolean): number {
  return (macInset ? MAC_TRAFFIC_LIGHT_INSET : 0) + LEFT_CHROME_CLUSTER
}

/**
 * Width the window's right-edge column reserves for the right overlay.
 * `winLinuxCaption` adds the native caption-button strip (desktop Win/Linux).
 */
export function rightChromeReserve(winLinuxCaption: boolean): number {
  return RIGHT_CHROME_CLUSTER + (winLinuxCaption ? WINDOW_CAPTION_WIDTH : 0)
}
