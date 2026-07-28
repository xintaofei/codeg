import { describe, it, expect } from "vitest"
import {
  fileNameOf,
  isAbsoluteFilePath,
  isAddedFileDiff,
  isRemovedFileDiff,
  toAbsoluteFilePath,
  toFolderRelativePath,
  toNativeAbsoluteFilePath,
} from "./file-path-display"

describe("isAddedFileDiff", () => {
  it("detects a write-op diff (synthesized from /dev/null)", () => {
    const diff = [
      "--- /dev/null",
      "+++ b/src/a.ts",
      "@@ -0,0 +1,1 @@",
      "+x",
    ].join("\n")
    expect(isAddedFileDiff(diff)).toBe(true)
  })

  it("detects an apply_patch Add File section", () => {
    const diff = ["*** Add File: src/new.ts", "+line one", "+line two"].join(
      "\n"
    )
    expect(isAddedFileDiff(diff)).toBe(true)
  })

  it("detects a git-style new file mode header", () => {
    const diff = [
      "diff --git a/src/n.ts b/src/n.ts",
      "new file mode 100644",
      "--- /dev/null",
      "+++ b/src/n.ts",
    ].join("\n")
    expect(isAddedFileDiff(diff)).toBe(true)
  })

  it("does not flag a plain modification (edit) diff", () => {
    const diff = [
      "--- a/src/a.ts",
      "+++ b/src/a.ts",
      "@@ -1,1 +1,1 @@",
      "-const a = 1",
      "+const a = 2",
    ].join("\n")
    expect(isAddedFileDiff(diff)).toBe(false)
  })

  it("does not flag a deletion diff or null", () => {
    const removed = ["*** Delete File: src/gone.ts", "-old"].join("\n")
    expect(isAddedFileDiff(removed)).toBe(false)
    expect(isAddedFileDiff(null)).toBe(false)
  })
})

describe("isRemovedFileDiff / isAddedFileDiff precedence", () => {
  it("classifies a diff that both adds and deletes as removed-first at the call site", () => {
    // A reply that creates then deletes a file joins both chunks; the card
    // checks removed BEFORE added, so it lands in the changed list, not "new".
    const joined = [
      "--- /dev/null",
      "+++ b/tmp.ts",
      "+x",
      "",
      "*** Delete File: tmp.ts",
      "-x",
    ].join("\n")
    expect(isRemovedFileDiff(joined)).toBe(true)
    expect(isAddedFileDiff(joined)).toBe(true)
    // The component resolves the tie as: removed wins.
  })
})

describe("isAbsoluteFilePath", () => {
  it("treats POSIX and Windows drive paths as absolute", () => {
    expect(isAbsoluteFilePath("/repo/src/a.ts")).toBe(true)
    expect(isAbsoluteFilePath("C:\\repo\\a.ts")).toBe(true)
    expect(isAbsoluteFilePath("C:/repo/a.ts")).toBe(true)
  })

  it("treats bare relative paths as not absolute", () => {
    expect(isAbsoluteFilePath("src/a.ts")).toBe(false)
    expect(isAbsoluteFilePath("./src/a.ts")).toBe(false)
  })
})

describe("toAbsoluteFilePath", () => {
  it("passes an absolute path through (slash-normalized)", () => {
    expect(toAbsoluteFilePath("/repo/src/a.ts", "/repo")).toBe("/repo/src/a.ts")
    expect(toAbsoluteFilePath("C:\\repo\\a.ts", "C:\\repo")).toBe(
      "C:/repo/a.ts"
    )
  })

  it("joins a relative path onto the active folder", () => {
    expect(toAbsoluteFilePath("src/a.ts", "/repo")).toBe("/repo/src/a.ts")
    // Trailing folder slash and ./ prefix are both trimmed.
    expect(toAbsoluteFilePath("./src/a.ts", "/repo/")).toBe("/repo/src/a.ts")
  })

  it("returns null for a relative path with no folder", () => {
    expect(toAbsoluteFilePath("src/a.ts")).toBeNull()
    expect(toAbsoluteFilePath("src/a.ts", "")).toBeNull()
  })
})

describe("toNativeAbsoluteFilePath", () => {
  it("joins relative paths with the folder's native separator", () => {
    expect(toNativeAbsoluteFilePath("src/a.ts", "/repo")).toBe("/repo/src/a.ts")
    expect(toNativeAbsoluteFilePath("src/a.ts", "C:\\repo")).toBe(
      "C:\\repo\\src\\a.ts"
    )
    expect(toNativeAbsoluteFilePath("./src/a.ts", "C:\\repo\\")).toBe(
      "C:\\repo\\src\\a.ts"
    )
  })

  it("normalizes absolute Windows paths to backslashes when appropriate", () => {
    expect(toNativeAbsoluteFilePath("C:/repo/a.ts", "C:\\repo")).toBe(
      "C:\\repo\\a.ts"
    )
    expect(toNativeAbsoluteFilePath("C:\\repo\\a.ts")).toBe("C:\\repo\\a.ts")
  })

  it("keeps POSIX absolute paths slash-normalized", () => {
    expect(toNativeAbsoluteFilePath("/repo/src/a.ts", "/repo")).toBe(
      "/repo/src/a.ts"
    )
  })

  it("returns null when a relative path has no folder", () => {
    expect(toNativeAbsoluteFilePath("src/a.ts")).toBeNull()
    expect(toNativeAbsoluteFilePath("src/a.ts", "")).toBeNull()
  })
})

describe("toFolderRelativePath / fileNameOf", () => {
  it("strips the folder prefix and extracts the file name", () => {
    const rel = toFolderRelativePath("/repo/src/a.ts", "/repo")
    expect(rel).toBe("src/a.ts")
    expect(fileNameOf(rel)).toBe("a.ts")
  })

  it("strips a case-mismatched Windows prefix (keeping original casing)", () => {
    // Agent reports `c:\repo\...`; folder is stored as `C:\Repo`. Windows FS is
    // case-insensitive, so this must still resolve to a relative path (not leak
    // an absolute one to openFilePreview) — and keep the file's own casing.
    expect(toFolderRelativePath("c:\\repo\\src\\New.ts", "C:\\Repo")).toBe(
      "src/New.ts"
    )
  })

  it("strips a Windows drive-root workspace prefix (C:\\)", () => {
    // The trailing-slash trim turns `C:/` into `C:`; detection must still see
    // this as Windows (via the re-added slash) so a case-mismatched drive root
    // still resolves in-workspace files to a relative path.
    expect(toFolderRelativePath("c:\\src\\a.ts", "C:\\")).toBe("src/a.ts")
    expect(toFolderRelativePath("C:/src/a.ts", "C:/")).toBe("src/a.ts")
  })

  it("keeps POSIX prefix matching case-sensitive", () => {
    // Distinct dirs on a case-sensitive FS — must NOT strip.
    expect(toFolderRelativePath("/Repo/src/a.ts", "/repo")).toBe(
      "/Repo/src/a.ts"
    )
  })

  it("leaves an outside-workspace path absolute so callers can skip it", () => {
    const rel = toFolderRelativePath("/other/x.ts", "/repo")
    expect(rel).toBe("/other/x.ts")
    expect(isAbsoluteFilePath(rel)).toBe(true)
  })
})
