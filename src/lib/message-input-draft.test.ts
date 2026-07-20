import type { JSONContent } from "@tiptap/core"
import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  clearMessageInputDraftV2,
  loadMessageInputDraftV2,
  saveMessageInputDraft,
  saveMessageInputDraftV2,
} from "./message-input-draft"

const DOC: JSONContent = {
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text: "hello" }] }],
}

const V1 = (k: string) => `codeg:message-input-draft:v1:${k}`
const V2 = (k: string) => `codeg:message-input-draft:v2:${k}`

beforeEach(() => {
  localStorage.clear()
})

describe("message-input-draft v2", () => {
  it("round-trips a document through save/load", () => {
    saveMessageInputDraftV2("k-roundtrip", DOC)
    expect(loadMessageInputDraftV2("k-roundtrip")).toEqual({
      kind: "doc",
      doc: DOC,
    })
  })

  it("persists the document to localStorage under the v2 key", () => {
    saveMessageInputDraftV2("k-persist", DOC)
    const raw = localStorage.getItem(V2("k-persist"))
    expect(raw).not.toBeNull()
    expect(JSON.parse(raw as string)).toEqual({ doc: DOC })
  })

  it("returns null when there is no draft", () => {
    expect(loadMessageInputDraftV2("k-empty")).toBeNull()
  })

  it("falls back to a legacy v1 text draft (migration read)", () => {
    saveMessageInputDraft("k-legacy", "draft text")
    expect(loadMessageInputDraftV2("k-legacy")).toEqual({
      kind: "legacyMarkdown",
      markdown: "draft text",
    })
  })

  it("does not delete the legacy v1 draft on read (unedited migration survives)", () => {
    saveMessageInputDraft("k-keep", "still here")
    loadMessageInputDraftV2("k-keep")
    expect(localStorage.getItem(V1("k-keep"))).not.toBeNull()
  })

  it("a saved v2 document supersedes and removes the legacy v1 draft", () => {
    saveMessageInputDraft("k-mig", "old text")
    expect(loadMessageInputDraftV2("k-mig")).toMatchObject({
      kind: "legacyMarkdown",
    })
    saveMessageInputDraftV2("k-mig", DOC)
    expect(loadMessageInputDraftV2("k-mig")).toEqual({ kind: "doc", doc: DOC })
    expect(localStorage.getItem(V1("k-mig"))).toBeNull()
  })

  it("clearMessageInputDraftV2 removes both the v2 and legacy v1 entries", () => {
    saveMessageInputDraft("k-clear", "legacy")
    saveMessageInputDraftV2("k-clear", DOC)
    clearMessageInputDraftV2("k-clear")
    expect(loadMessageInputDraftV2("k-clear")).toBeNull()
    expect(localStorage.getItem(V2("k-clear"))).toBeNull()
    expect(localStorage.getItem(V1("k-clear"))).toBeNull()
  })

  it("keeps the legacy v1 draft when the v2 write fails (not retired early)", () => {
    saveMessageInputDraft("k-fail", "legacy survives")
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation((key: string) => {
        if (key.startsWith("codeg:message-input-draft:v2:")) {
          throw new Error("quota exceeded")
        }
      })
    try {
      // Flushes synchronously in jsdom; the v2 setItem throws.
      saveMessageInputDraftV2("k-fail", DOC)
    } finally {
      setItem.mockRestore()
    }
    // v1 is still on disk because the v2 write never succeeded.
    expect(localStorage.getItem(V1("k-fail"))).not.toBeNull()
  })

  it("ignores a corrupt v2 payload and falls back to the legacy v1 draft", () => {
    localStorage.setItem(V2("k-corrupt"), JSON.stringify({ doc: {} }))
    saveMessageInputDraft("k-corrupt", "fallback")
    expect(loadMessageInputDraftV2("k-corrupt")).toEqual({
      kind: "legacyMarkdown",
      markdown: "fallback",
    })
  })

  it("ignores a non-object/array v2 payload and returns null without a v1 draft", () => {
    localStorage.setItem(V2("k-corrupt2"), JSON.stringify({ doc: [1, 2] }))
    expect(loadMessageInputDraftV2("k-corrupt2")).toBeNull()
  })

  it("keeps mobile drafts in memory without writing localStorage", () => {
    const windowRecord = window as unknown as Record<string, unknown>
    const originalInternals = windowRecord.__TAURI_INTERNALS__
    const originalUserAgent = Object.getOwnPropertyDescriptor(
      navigator,
      "userAgent"
    )
    try {
      windowRecord.__TAURI_INTERNALS__ = { invoke: () => {} }
      Object.defineProperty(navigator, "userAgent", {
        configurable: true,
        value: "Mozilla/5.0 (Linux; Android 15)",
      })

      saveMessageInputDraftV2("k-mobile-memory", DOC)
      expect(loadMessageInputDraftV2("k-mobile-memory")).toEqual({
        kind: "doc",
        doc: DOC,
      })
      expect(localStorage.getItem(V2("k-mobile-memory"))).toBeNull()
    } finally {
      clearMessageInputDraftV2("k-mobile-memory")
      if (originalInternals === undefined) {
        delete windowRecord.__TAURI_INTERNALS__
      } else {
        windowRecord.__TAURI_INTERNALS__ = originalInternals
      }
      if (originalUserAgent) {
        Object.defineProperty(navigator, "userAgent", originalUserAgent)
      } else {
        delete (navigator as unknown as Record<string, unknown>).userAgent
      }
    }
  })
})
