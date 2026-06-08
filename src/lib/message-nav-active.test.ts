import { describe, it, expect } from "vitest"
import {
  pickActiveThreadIndex,
  reconcileActive,
  type ActiveClickGuard,
} from "./message-nav-active"

// User-message ticks are sparse in the thread (assistant turns sit between
// them), so threadIndex jumps: 0, 3, 6.
const entries = [{ threadIndex: 0 }, { threadIndex: 3 }, { threadIndex: 6 }]

describe("pickActiveThreadIndex", () => {
  it("returns null when the top is above the first entry", () => {
    expect(pickActiveThreadIndex(entries, -1)).toBeNull()
    expect(pickActiveThreadIndex([], 5)).toBeNull()
  })

  it("picks the last entry at or above the viewport top", () => {
    expect(pickActiveThreadIndex(entries, 0)).toBe(0)
    expect(pickActiveThreadIndex(entries, 2)).toBe(0)
    expect(pickActiveThreadIndex(entries, 3)).toBe(3)
    expect(pickActiveThreadIndex(entries, 5)).toBe(3)
    expect(pickActiveThreadIndex(entries, 6)).toBe(6)
    expect(pickActiveThreadIndex(entries, 100)).toBe(6)
  })
})

describe("reconcileActive", () => {
  it("passes the reading through when no guard is armed", () => {
    expect(reconcileActive(3, null, 0)).toEqual({ active: 3, guard: null })
    expect(reconcileActive(null, null, 0)).toEqual({
      active: null,
      guard: null,
    })
  })

  it("releases the guard as soon as the scroll arrives at the target", () => {
    const guard: ActiveClickGuard = { target: 6, releaseAfter: 1000 }
    expect(reconcileActive(6, guard, 200)).toEqual({ active: 6, guard: null })
  })

  it("holds the clicked tick instead of regressing to the previous one", () => {
    // The smooth scroll toward tick 6 momentarily reads tick 3 (the previous
    // user message); within the guard window we keep showing 6, not 3.
    const guard: ActiveClickGuard = { target: 6, releaseAfter: 1000 }
    expect(reconcileActive(3, guard, 200)).toEqual({ active: 6, guard })
  })

  it("safety-releases after the window so normal tracking resumes", () => {
    const guard: ActiveClickGuard = { target: 6, releaseAfter: 1000 }
    expect(reconcileActive(3, guard, 1500)).toEqual({ active: 3, guard: null })
  })

  it("keeps a bottom-clamped click active, then resumes on later scroll", () => {
    // Click the last message; align:"start" is clamped so the scroll-spy can
    // never read 6 — it keeps reading the previous tick 3.
    let guard: ActiveClickGuard | null = { target: 6, releaseAfter: 1000 }

    // During the clamped scroll: stays on the clicked tick, never regresses.
    let step = reconcileActive(3, guard, 100)
    expect(step.active).toBe(6)
    guard = step.guard
    step = reconcileActive(3, guard, 600)
    expect(step.active).toBe(6)
    guard = step.guard

    // After the safety window (a genuine later scroll): normal tracking resumes.
    step = reconcileActive(3, guard, 1200)
    expect(step.active).toBe(3)
    expect(step.guard).toBeNull()
  })
})
