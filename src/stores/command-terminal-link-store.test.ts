import { beforeEach, describe, expect, it } from "vitest"
import {
  resolveLiveCommandTerminalId,
  useCommandTerminalLinkStore,
} from "./command-terminal-link-store"

beforeEach(() => {
  useCommandTerminalLinkStore.setState({ links: {} })
})

describe("command-terminal-link store", () => {
  describe("setLink", () => {
    it("adds a link", () => {
      useCommandTerminalLinkStore.getState().setLink(1, "term-a")
      expect(useCommandTerminalLinkStore.getState().links).toEqual({
        1: "term-a",
      })
    })

    it("overwrites the link for the same command", () => {
      const store = useCommandTerminalLinkStore.getState()
      store.setLink(1, "term-a")
      store.setLink(1, "term-b")
      expect(useCommandTerminalLinkStore.getState().links).toEqual({
        1: "term-b",
      })
    })

    it("keeps the links reference stable when unchanged", () => {
      const store = useCommandTerminalLinkStore.getState()
      store.setLink(1, "term-a")
      const before = useCommandTerminalLinkStore.getState().links
      store.setLink(1, "term-a")
      expect(useCommandTerminalLinkStore.getState().links).toBe(before)
    })
  })

  describe("clearLink", () => {
    it("removes a link, leaving the others", () => {
      const store = useCommandTerminalLinkStore.getState()
      store.setLink(1, "term-a")
      store.setLink(2, "term-b")
      store.clearLink(1)
      expect(useCommandTerminalLinkStore.getState().links).toEqual({
        2: "term-b",
      })
    })

    it("is a stable no-op when the command has no link", () => {
      const store = useCommandTerminalLinkStore.getState()
      store.setLink(1, "term-a")
      const before = useCommandTerminalLinkStore.getState().links
      store.clearLink(999)
      expect(useCommandTerminalLinkStore.getState().links).toBe(before)
    })
  })

  describe("pruneTerminals", () => {
    it("drops links whose terminal matches the predicate", () => {
      const store = useCommandTerminalLinkStore.getState()
      store.setLink(1, "term-a")
      store.setLink(2, "term-b")
      store.setLink(3, "term-c")
      store.pruneTerminals((termId) => termId === "term-b")
      expect(useCommandTerminalLinkStore.getState().links).toEqual({
        1: "term-a",
        3: "term-c",
      })
    })

    it("is a stable no-op when nothing matches", () => {
      const store = useCommandTerminalLinkStore.getState()
      store.setLink(1, "term-a")
      const before = useCommandTerminalLinkStore.getState().links
      store.pruneTerminals(() => false)
      expect(useCommandTerminalLinkStore.getState().links).toBe(before)
    })
  })

  describe("resolveLiveCommandTerminalId", () => {
    const isLive = (id: string) => id === "term-a"

    it("returns the terminal id when the link exists and is live", () => {
      expect(resolveLiveCommandTerminalId({ 1: "term-a" }, 1, isLive)).toBe(
        "term-a"
      )
    })

    it("returns undefined when the command has no link", () => {
      expect(resolveLiveCommandTerminalId({}, 1, isLive)).toBeUndefined()
    })

    it("returns undefined when the linked terminal is not live (exited/closed)", () => {
      expect(
        resolveLiveCommandTerminalId({ 1: "term-dead" }, 1, isLive)
      ).toBeUndefined()
    })
  })
})
