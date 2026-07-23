import { describe, it, expect } from "vitest"
import {
  getExternalEditorOpenWith,
  resolveExternalEditorOpenWithCandidates,
  resolveFilePathTargets,
  systemExplorerLabelKey,
} from "./file-path-actions"

describe("resolveFilePathTargets", () => {
  it("resolves relative and absolute forms for a workspace file", () => {
    const targets = resolveFilePathTargets("src/a.ts", "/repo")
    expect(targets.relativePath).toBe("src/a.ts")
    expect(targets.absolutePath).toBe("/repo/src/a.ts")
    expect(targets.fileName).toBe("a.ts")
  })

  it("uses native separators on Windows folders", () => {
    const targets = resolveFilePathTargets("src/a.ts", "C:\\repo")
    expect(targets.relativePath).toBe("src/a.ts")
    expect(targets.absolutePath).toBe("C:\\repo\\src\\a.ts")
    expect(targets.fileName).toBe("a.ts")
  })

  it("keeps absolute agent paths and strips folder prefix when inside", () => {
    const targets = resolveFilePathTargets("/repo/src/a.ts", "/repo")
    expect(targets.relativePath).toBe("src/a.ts")
    expect(targets.absolutePath).toBe("/repo/src/a.ts")
  })
})

describe("systemExplorerLabelKey", () => {
  it("picks platform-specific keys", () => {
    expect(systemExplorerLabelKey("MacIntel Macintosh")).toBe("openInFinder")
    expect(systemExplorerLabelKey("Win32 Windows NT")).toBe("openInExplorer")
    expect(systemExplorerLabelKey("Linux x86_64")).toBe("openInFileManager")
  })
})

describe("getExternalEditorOpenWith", () => {
  it("uses full app names on macOS for open -a", () => {
    expect(getExternalEditorOpenWith("vscode", "macos")).toBe(
      "Visual Studio Code"
    )
    expect(getExternalEditorOpenWith("cursor", "macos")).toBe("Cursor")
  })

  it("uses CLI shims on Windows and Linux", () => {
    expect(getExternalEditorOpenWith("vscode", "windows")).toBe("code")
    expect(getExternalEditorOpenWith("cursor", "linux")).toBe("cursor")
  })
})

describe("resolveExternalEditorOpenWithCandidates", () => {
  it("returns macOS application names only", async () => {
    await expect(
      resolveExternalEditorOpenWithCandidates("cursor", "macos")
    ).resolves.toEqual(["Cursor"])
    await expect(
      resolveExternalEditorOpenWithCandidates("vscode", "macos")
    ).resolves.toEqual(["Visual Studio Code"])
  })

  it("ends Windows cursor candidates with .cmd then bare name (exe first when LocalAppData works)", async () => {
    const candidates = await resolveExternalEditorOpenWithCandidates(
      "cursor",
      "windows"
    )
    expect(candidates.length).toBeGreaterThanOrEqual(2)
    expect(candidates.at(-2)).toBe("cursor.cmd")
    expect(candidates.at(-1)).toBe("cursor")
    // Prefer real EXE when path API is available (Tauri); otherwise PATH only.
    const exe = candidates.find((c) => /Cursor\.exe$/i.test(c))
    if (exe) {
      expect(exe).toMatch(/Programs/i)
    }
  })

  it("lists Linux CLI shims", async () => {
    await expect(
      resolveExternalEditorOpenWithCandidates("vscode", "linux")
    ).resolves.toEqual(["code", "code-insiders"])
  })
})
