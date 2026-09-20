import { describe, expect, it } from "vitest"
import {
  addOrUpdateNote,
  countNotesByFile,
  formatPipelineNotes,
  getNoteForLine,
  getNotesForFile,
  parsePipelineNotes,
  removeNote,
  type PipelineLineNote,
} from "./pipeline-notes"

describe("pipeline-notes", () => {
  describe("formatPipelineNotes", () => {
    it("returns empty string when given empty or invalid input", () => {
      expect(formatPipelineNotes([])).toBe("")
      // @ts-expect-error testing invalid input
      expect(formatPipelineNotes(null)).toBe("")
    })

    it("formats a single note into 'path:line: note'", () => {
      const notes: PipelineLineNote[] = [
        { path: "src/main.rs", line: 42, note: "check boundary condition" },
      ]
      expect(formatPipelineNotes(notes)).toBe(
        "src/main.rs:42: check boundary condition"
      )
    })

    it("formats multiple notes sorted by path then line number", () => {
      const notes: PipelineLineNote[] = [
        { path: "src/b.ts", line: 20, note: "second file note" },
        { path: "src/a.ts", line: 100, note: "first file second line" },
        { path: "src/a.ts", line: 15, note: "first file first line" },
      ]
      const result = formatPipelineNotes(notes)
      expect(result).toBe(
        "src/a.ts:15: first file first line\nsrc/a.ts:100: first file second line\nsrc/b.ts:20: second file note"
      )
    })

    it("omits empty or whitespace-only notes and trims whitespace", () => {
      const notes: PipelineLineNote[] = [
        { path: "src/a.ts", line: 10, note: "   valid note   " },
        { path: "src/b.ts", line: 5, note: "   " },
        { path: "src/c.ts", line: 0, note: "invalid line" },
      ]
      expect(formatPipelineNotes(notes)).toBe("src/a.ts:10: valid note")
    })
  })

  describe("parsePipelineNotes", () => {
    it("parses formatted notes string back into objects", () => {
      const raw =
        "src/main.rs:42: check boundary condition\nsrc/lib/types.ts:15: add missing field"
      const parsed = parsePipelineNotes(raw)
      expect(parsed).toEqual([
        {
          id: "src/main.rs:42",
          path: "src/main.rs",
          line: 42,
          note: "check boundary condition",
        },
        {
          id: "src/lib/types.ts:15",
          path: "src/lib/types.ts",
          line: 15,
          note: "add missing field",
        },
      ])
    })

    it("returns empty array for empty string or invalid lines", () => {
      expect(parsePipelineNotes("")).toEqual([])
      expect(parsePipelineNotes("   \n\n  ")).toEqual([])
      expect(parsePipelineNotes("not a valid note line")).toEqual([])
    })
  })

  describe("addOrUpdateNote", () => {
    it("adds a new note to an empty list", () => {
      const result = addOrUpdateNote([], {
        path: "src/app.tsx",
        line: 10,
        note: "refactor this",
      })
      expect(result).toHaveLength(1)
      expect(result[0]).toEqual({
        id: "src/app.tsx:10",
        path: "src/app.tsx",
        line: 10,
        note: "refactor this",
      })
    })

    it("updates existing note for the same path and line", () => {
      const initial: PipelineLineNote[] = [
        { path: "src/app.tsx", line: 10, note: "old note", id: "note-1" },
      ]
      const result = addOrUpdateNote(initial, {
        path: "src/app.tsx",
        line: 10,
        note: "new updated note",
      })
      expect(result).toHaveLength(1)
      expect(result[0]?.note).toBe("new updated note")
    })

    it("removes the note if updated with empty text", () => {
      const initial: PipelineLineNote[] = [
        { path: "src/app.tsx", line: 10, note: "old note" },
      ]
      const result = addOrUpdateNote(initial, {
        path: "src/app.tsx",
        line: 10,
        note: "   ",
      })
      expect(result).toHaveLength(0)
    })
  })

  describe("removeNote", () => {
    const initial: PipelineLineNote[] = [
      { id: "note-1", path: "src/a.ts", line: 5, note: "first" },
      { id: "src/b.ts:10", path: "src/b.ts", line: 10, note: "second" },
    ]

    it("removes by string id", () => {
      const result = removeNote(initial, "note-1")
      expect(result).toHaveLength(1)
      expect(result[0]?.path).toBe("src/b.ts")
    })

    it("removes by path and line object", () => {
      const result = removeNote(initial, { path: "src/b.ts", line: 10 })
      expect(result).toHaveLength(1)
      expect(result[0]?.path).toBe("src/a.ts")
    })
  })

  describe("getNotesForFile and getNoteForLine", () => {
    const notes: PipelineLineNote[] = [
      { path: "src/a.ts", line: 20, note: "line 20 note" },
      { path: "src/a.ts", line: 5, note: "line 5 note" },
      { path: "src/b.ts", line: 10, note: "file b note" },
    ]

    it("returns sorted notes for a specific file", () => {
      const fileNotes = getNotesForFile(notes, "src/a.ts")
      expect(fileNotes).toHaveLength(2)
      expect(fileNotes[0]?.line).toBe(5)
      expect(fileNotes[1]?.line).toBe(20)
    })

    it("returns empty array for non-existent file", () => {
      expect(getNotesForFile(notes, "src/c.ts")).toEqual([])
    })

    it("returns specific note for line", () => {
      expect(getNoteForLine(notes, "src/a.ts", 5)?.note).toBe("line 5 note")
      expect(getNoteForLine(notes, "src/a.ts", 99)).toBeUndefined()
    })
  })

  describe("countNotesByFile", () => {
    it("calculates correct note counts per file", () => {
      const notes: PipelineLineNote[] = [
        { path: "src/a.ts", line: 5, note: "note 1" },
        { path: "src/a.ts", line: 15, note: "note 2" },
        { path: "src/b.ts", line: 1, note: "note 3" },
      ]
      const counts = countNotesByFile(notes)
      expect(counts).toEqual({
        "src/a.ts": 2,
        "src/b.ts": 1,
      })
    })
  })
})
