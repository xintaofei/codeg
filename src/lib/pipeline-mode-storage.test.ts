import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  clearPipelineMode,
  DEFAULT_PIPELINE_MODE,
  getPipelineModeStorageKey,
  isPipelineModeKey,
  loadPipelineMode,
  PIPELINE_MODE_KEYS,
  savePipelineMode,
} from "./pipeline-mode-storage"

describe("pipeline-mode-storage", () => {
  beforeEach(() => {
    localStorage.clear()
    vi.restoreAllMocks()
  })

  describe("getPipelineModeStorageKey", () => {
    it("returns folder-scoped key when folderId is a number", () => {
      expect(getPipelineModeStorageKey(1)).toBe("codeg.pipeline.mode.1")
      expect(getPipelineModeStorageKey(42)).toBe("codeg.pipeline.mode.42")
    })

    it("returns global key when folderId is null or undefined", () => {
      expect(getPipelineModeStorageKey(null)).toBe("codeg.pipeline.mode.global")
      expect(getPipelineModeStorageKey(undefined)).toBe(
        "codeg.pipeline.mode.global"
      )
      expect(getPipelineModeStorageKey()).toBe("codeg.pipeline.mode.global")
    })
  })

  describe("isPipelineModeKey", () => {
    it("accepts all valid pipeline mode keys", () => {
      for (const mode of PIPELINE_MODE_KEYS) {
        expect(isPipelineModeKey(mode)).toBe(true)
      }
    })

    it("rejects invalid values and non-strings", () => {
      expect(isPipelineModeKey("invalid")).toBe(false)
      expect(isPipelineModeKey("")).toBe(false)
      expect(isPipelineModeKey(null)).toBe(false)
      expect(isPipelineModeKey(undefined)).toBe(false)
      expect(isPipelineModeKey(123)).toBe(false)
      expect(isPipelineModeKey({})).toBe(false)
    })
  })

  describe("loadPipelineMode", () => {
    it("returns default 'single' mode when nothing is stored", () => {
      expect(loadPipelineMode(10)).toBe(null)
      expect(loadPipelineMode() ?? DEFAULT_PIPELINE_MODE).toBe("single")
    })

    it("loads saved mode for a specific folderId", () => {
      localStorage.setItem("codeg.pipeline.mode.5", "duet")
      expect(loadPipelineMode(5)).toBe("duet")
    })

    it("falls back to default if stored value is invalid", () => {
      localStorage.setItem("codeg.pipeline.mode.5", "invalid_mode")
      expect(loadPipelineMode(5) ?? DEFAULT_PIPELINE_MODE).toBe("single")
    })

    it("handles localStorage.getItem exceptions gracefully", () => {
      vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
        throw new Error("SecurityError: Access is denied")
      })
      expect(loadPipelineMode(1) ?? DEFAULT_PIPELINE_MODE).toBe("single")
    })
  })

  describe("savePipelineMode", () => {
    it("saves mode for a specific folderId", () => {
      savePipelineMode("team", 12)
      expect(localStorage.getItem("codeg.pipeline.mode.12")).toBe("team")
    })

    it("saves mode globally when folderId is omitted", () => {
      savePipelineMode("custom")
      expect(localStorage.getItem("codeg.pipeline.mode.global")).toBe("custom")
    })

    it("keeps different folders independent", () => {
      savePipelineMode("duet", 1)
      savePipelineMode("team", 2)
      expect(loadPipelineMode(1)).toBe("duet")
      expect(loadPipelineMode(2)).toBe("team")
    })

    it("handles localStorage.setItem exceptions gracefully", () => {
      vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
        throw new Error("QuotaExceededError")
      })
      expect(() => savePipelineMode("duet", 1)).not.toThrow()
    })
  })

  describe("clearPipelineMode", () => {
    it("removes the stored mode for the folder", () => {
      savePipelineMode("team", 7)
      expect(loadPipelineMode(7)).toBe("team")
      clearPipelineMode(7)
      expect(loadPipelineMode(7)).toBe(null)
    })

    it("handles localStorage.removeItem exceptions gracefully", () => {
      vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
        throw new Error("Storage failure")
      })
      expect(() => clearPipelineMode(7)).not.toThrow()
    })
  })
})
