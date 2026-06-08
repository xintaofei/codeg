/**
 * Active-tick resolution for the conversation message navigator.
 *
 * The navigator highlights the user-message tick nearest the top of the
 * viewport (a scroll-spy). Two wrinkles are handled here as pure functions so
 * they stay unit-testable away from the virtualizer:
 *
 *  1. A click optimistically activates the clicked tick. The smooth scroll that
 *     follows fires scroll-spy readings; we must not let them regress the
 *     highlight to the *previous* tick before the scroll arrives — and for a
 *     bottom-clamped target (a message near the end that can never reach the
 *     viewport top) the scroll never arrives at all.
 */

/** Pending "I just clicked this tick" intent. */
export interface ActiveClickGuard {
  /** threadIndex the user clicked. */
  target: number
  /**
   * Monotonic time (ms, e.g. `performance.now()`) after which the guard is
   * force-released. Safety net for bottom-clamped targets whose scroll never
   * produces a reading equal to `target`.
   */
  releaseAfter: number
}

/**
 * Last nav entry at or above the viewport top. `entries` must be ascending by
 * `threadIndex`. Returns null when the top is above the first entry.
 */
export function pickActiveThreadIndex(
  entries: readonly { threadIndex: number }[],
  startIndex: number
): number | null {
  let active: number | null = null
  for (const entry of entries) {
    if (entry.threadIndex <= startIndex) active = entry.threadIndex
    else break
  }
  return active
}

export interface ReconciledActive {
  /** The active threadIndex to render. */
  active: number | null
  /** Guard to carry into the next reading (null once released). */
  guard: ActiveClickGuard | null
}

/**
 * Reconcile a fresh scroll-spy reading (`computed`) with a pending click
 * `guard`. While the guard holds we keep showing `guard.target` so the clicked
 * tick never regresses to the previous one. The guard releases when the reading
 * reaches the target (the scroll arrived) or once `now >= guard.releaseAfter`
 * (the clamped-target safety net), after which normal scroll-spy resumes.
 */
export function reconcileActive(
  computed: number | null,
  guard: ActiveClickGuard | null,
  now: number
): ReconciledActive {
  if (!guard) return { active: computed, guard: null }
  // Arrived at the clicked tick → resume normal tracking.
  if (computed === guard.target) return { active: computed, guard: null }
  // Still settling (incl. clamped targets) → hold the clicked tick.
  if (now < guard.releaseAfter) return { active: guard.target, guard }
  // Safety release → resume normal tracking.
  return { active: computed, guard: null }
}
