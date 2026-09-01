import { beforeEach, describe, expect, it } from "vitest"
import {
  CANVAS_MAX_ZOOM,
  CANVAS_MIN_ZOOM,
  loadCanvasDrafts,
  loadCanvasExpandedCards,
  loadCanvasMinimapVisible,
  loadCanvasViewport,
  saveCanvasDrafts,
  saveCanvasExpandedCards,
  saveCanvasMinimapVisible,
  saveCanvasViewport,
  type CanvasDraftCard,
} from "./canvas-view-storage"

/**
 * These entries are read at mount and drive what the canvas renders before any
 * backend data lands, so every reader has to degrade to "nothing remembered"
 * rather than throw or hand back a shape the view can't use. A corrupted
 * viewport in particular could strand the board at an unreachable zoom.
 */

const VIEWPORT_KEY = "workspace:canvas-viewport"
const CARDS_KEY = "workspace:canvas-expanded-cards"
const DRAFTS_KEY = "workspace:canvas-drafts"
const MINIMAP_KEY = "workspace:canvas-minimap"

describe("canvas view storage", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("round-trips a viewport", () => {
    saveCanvasViewport({ x: -320.5, y: 96, zoom: 0.75 })
    expect(loadCanvasViewport()).toEqual({ x: -320.5, y: 96, zoom: 0.75 })
  })

  it("clamps a stored zoom into the flow's own range", () => {
    localStorage.setItem(VIEWPORT_KEY, JSON.stringify({ x: 0, y: 0, zoom: 40 }))
    expect(loadCanvasViewport()?.zoom).toBe(CANVAS_MAX_ZOOM)
    localStorage.setItem(
      VIEWPORT_KEY,
      JSON.stringify({ x: 0, y: 0, zoom: 0.0001 })
    )
    expect(loadCanvasViewport()?.zoom).toBe(CANVAS_MIN_ZOOM)
  })

  it("treats damaged or incomplete entries as nothing remembered", () => {
    localStorage.setItem(VIEWPORT_KEY, "{not json")
    expect(loadCanvasViewport()).toBeNull()
    localStorage.setItem(VIEWPORT_KEY, JSON.stringify({ x: 1, y: 2 }))
    expect(loadCanvasViewport()).toBeNull()
    localStorage.setItem(
      VIEWPORT_KEY,
      JSON.stringify({ x: Number.NaN, y: 0, zoom: 1 })
    )
    expect(loadCanvasViewport()).toBeNull()
  })

  it("keeps only integral ids in the expanded-card set", () => {
    saveCanvasExpandedCards([3, 9])
    expect(loadCanvasExpandedCards()).toEqual([3, 9])
    localStorage.setItem(CARDS_KEY, JSON.stringify([1, "2", null, 3.5, 4]))
    expect(loadCanvasExpandedCards()).toEqual([1, 4])
    localStorage.setItem(CARDS_KEY, JSON.stringify({ nope: true }))
    expect(loadCanvasExpandedCards()).toEqual([])
  })

  it("round-trips drafts and drops the ones it cannot place", () => {
    const draft: CanvasDraftCard = {
      id: "abc",
      target: { folderId: 4 },
      agentType: "claude_code",
      x: 10,
      y: 20,
      width: 520,
      height: 560,
    }
    const chat: CanvasDraftCard = {
      ...draft,
      id: "def",
      target: { chat: true },
    }
    saveCanvasDrafts([draft, chat])
    expect(loadCanvasDrafts()).toEqual([draft, chat])

    localStorage.setItem(
      DRAFTS_KEY,
      JSON.stringify([
        draft,
        // No target at all, a target naming nothing, and no geometry: a card
        // built from any of these would have nowhere to send its first message.
        { ...draft, id: "x", target: undefined },
        { ...draft, id: "y", target: {} },
        { ...draft, id: "z", x: undefined },
        { ...draft, id: "", target: { chat: true } },
      ])
    )
    expect(loadCanvasDrafts().map((d) => d.id)).toEqual(["abc"])
  })

  it("refuses drafts with no area and collapses repeated ids", () => {
    const draft: CanvasDraftCard = {
      id: "abc",
      target: { chat: true },
      agentType: "codex",
      x: 0,
      y: 0,
      width: 520,
      height: 560,
    }
    localStorage.setItem(
      DRAFTS_KEY,
      JSON.stringify([
        // A zero/negative box restores a window too small to read or close.
        { ...draft, id: "flat", height: 0 },
        { ...draft, id: "inverted", width: -520 },
        draft,
        // The id is the connection key too: two cards under one key would be
        // two surfaces fighting over the same agent.
        { ...draft, x: 999 },
      ])
    )
    const loaded = loadCanvasDrafts()
    expect(loaded.map((d) => d.id)).toEqual(["abc"])
    expect(loaded[0].x).toBe(0)
  })

  it("remembers a draft's colour without ever failing a draft over it", () => {
    // The colour has nowhere else to live until the first send creates the row
    // that will hold it, so it has to survive a reload here.
    const draft: CanvasDraftCard = {
      id: "abc",
      target: { chat: true },
      agentType: "codex",
      color: "sky",
      x: 0,
      y: 0,
      width: 520,
      height: 560,
    }
    saveCanvasDrafts([draft])
    expect(loadCanvasDrafts()).toEqual([draft])

    localStorage.setItem(
      DRAFTS_KEY,
      JSON.stringify([
        // Junk in the one field that is pure decoration. Dropping the draft
        // would take the card AND the text typed into it with it — the colour
        // is simply forgotten instead, and an unknown name would be ignored at
        // paint time anyway.
        { ...draft, id: "wrong-type", color: 7 },
        { ...draft, id: "unknown-name", color: "not-a-colour" },
      ])
    )
    const loaded = loadCanvasDrafts()
    expect(loaded.map((d) => d.id)).toEqual(["wrong-type", "unknown-name"])
    expect(loaded[0].color).toBeUndefined()
    expect(loaded[1].color).toBe("not-a-colour")
  })

  it("clears the draft entry rather than storing an empty list", () => {
    saveCanvasDrafts([
      {
        id: "abc",
        target: { chat: true },
        agentType: "codex",
        x: 0,
        y: 0,
        width: 1,
        height: 1,
      },
    ])
    saveCanvasDrafts([])
    expect(localStorage.getItem(DRAFTS_KEY)).toBeNull()
  })

  it("shows the navigator map until it is explicitly dismissed", () => {
    // The asymmetry is the point: this is the one entry whose default is ON, so
    // "nothing remembered" has to mean shown. Reading it as a plain truthiness
    // check would hide the map for every user who has never opened the canvas.
    expect(loadCanvasMinimapVisible()).toBe(true)
    localStorage.setItem(MINIMAP_KEY, "null")
    expect(loadCanvasMinimapVisible()).toBe(true)
    localStorage.setItem(MINIMAP_KEY, "not json")
    expect(loadCanvasMinimapVisible()).toBe(true)
    localStorage.setItem(MINIMAP_KEY, '"false"')
    expect(loadCanvasMinimapVisible()).toBe(true)
  })

  it("round-trips a dismissed map", () => {
    saveCanvasMinimapVisible(false)
    expect(loadCanvasMinimapVisible()).toBe(false)
    saveCanvasMinimapVisible(true)
    expect(loadCanvasMinimapVisible()).toBe(true)
  })
})
