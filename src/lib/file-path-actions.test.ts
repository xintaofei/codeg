import { describe, it, expect } from "vitest"
import {
  EXTERNAL_EDITOR_OPEN_WITH,
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

describe("EXTERNAL_EDITOR_OPEN_WITH", () => {
  it("maps editors to CLI names the opener plugin understands", () => {
    expect(EXTERNAL_EDITOR_OPEN_WITH.vscode).toBe("code")
    expect(EXTERNAL_EDITOR_OPEN_WITH.cursor).toBe("cursor")
  })
})
