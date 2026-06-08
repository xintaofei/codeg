import { describe, it, expect } from "vitest"
import { extractSessionFilesGrouped } from "./session-files"
import type { MessageTurn } from "./types"

function userTurn(id: string, text: string): MessageTurn {
  return {
    id,
    role: "user",
    blocks: [{ type: "text", text }],
    timestamp: "2024-01-01T00:00:00Z",
  }
}

function writeTurn(
  id: string,
  toolId: string,
  filePath: string,
  content: string
): MessageTurn {
  return {
    id,
    role: "assistant",
    blocks: [
      {
        type: "tool_use",
        tool_use_id: toolId,
        tool_name: "Write",
        input_preview: JSON.stringify({ file_path: filePath, content }),
      },
    ],
    timestamp: "2024-01-01T00:00:01Z",
  }
}

describe("extractSessionFilesGrouped", () => {
  it("drops user turns with no edits by default", () => {
    const turns = [
      userTurn("u1", "hello"),
      userTurn("u2", "write a file"),
      writeTurn("a2", "t2", "/repo/src/a.ts", "a\nb\nc\n"),
    ]

    const groups = extractSessionFilesGrouped(turns)

    expect(groups).toHaveLength(1)
    expect(groups[0].userTurnId).toBe("u2")
    expect(groups[0].files).toHaveLength(1)
  })

  it("includeEmpty keeps a placeholder for every user turn, in order", () => {
    const turns = [
      userTurn("u1", "hello"),
      userTurn("u2", "write a file"),
      writeTurn("a2", "t2", "/repo/src/a.ts", "a\nb\nc\n"),
      userTurn("u3", "thanks"),
    ]

    const groups = extractSessionFilesGrouped(turns, { includeEmpty: true })

    // One slot per user message, preserving conversation order.
    expect(groups.map((g) => g.userTurnId)).toEqual(["u1", "u2", "u3"])
    // No-edit turns are placeholders with an empty file list.
    expect(groups[0].files).toEqual([])
    expect(groups[2].files).toEqual([])
    // The edited turn carries the file + line counts the rail surfaces.
    const edited = groups[1]
    expect(edited.userMessage).toBe("write a file")
    expect(edited.files).toHaveLength(1)
    expect(edited.files[0].path).toBe("/repo/src/a.ts")
    expect(edited.files[0].additions).toBeGreaterThan(0)
  })

  it("returns an empty array when there are no user turns", () => {
    expect(extractSessionFilesGrouped([], { includeEmpty: true })).toEqual([])
    expect(extractSessionFilesGrouped([])).toEqual([])
  })
})
