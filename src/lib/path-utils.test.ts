import { describe, expect, it } from "vitest"

import { joinFsPath, parentFsPath } from "./path-utils"

describe("parentFsPath", () => {
  describe("POSIX paths", () => {
    it("returns the parent directory", () => {
      expect(parentFsPath("/home/me/work")).toBe("/home/me")
      expect(parentFsPath("/home/me")).toBe("/home")
    })

    it("returns the root for a first-level directory", () => {
      expect(parentFsPath("/home")).toBe("/")
    })

    it("ignores a trailing slash", () => {
      expect(parentFsPath("/home/me/")).toBe("/home")
    })

    it("returns null at the filesystem root", () => {
      expect(parentFsPath("/")).toBeNull()
    })

    it("returns null for a bare relative segment with no parent", () => {
      expect(parentFsPath("foo")).toBeNull()
    })
  })

  describe("Windows drive paths", () => {
    it("returns the parent directory", () => {
      expect(parentFsPath("C:\\Users\\a\\project")).toBe("C:\\Users\\a")
      expect(parentFsPath("C:\\Users\\a")).toBe("C:\\Users")
    })

    it("returns the drive root (with separator) for a first-level dir", () => {
      expect(parentFsPath("C:\\Users")).toBe("C:\\")
    })

    it("returns null at the drive root", () => {
      expect(parentFsPath("C:\\")).toBeNull()
      expect(parentFsPath("C:")).toBeNull()
    })
  })

  describe("UNC paths", () => {
    it("returns the parent within a share", () => {
      expect(parentFsPath("\\\\server\\share\\folder\\sub")).toBe(
        "\\\\server\\share\\folder"
      )
      expect(parentFsPath("\\\\server\\share\\folder")).toBe(
        "\\\\server\\share"
      )
    })

    it("returns null at or above the share root", () => {
      expect(parentFsPath("\\\\server\\share")).toBeNull()
      expect(parentFsPath("\\\\server")).toBeNull()
    })

    it("handles forward-slash UNC prefixes", () => {
      expect(parentFsPath("//server/share/folder")).toBe("//server/share")
      expect(parentFsPath("//server/share")).toBeNull()
    })
  })

  it("returns null for an empty path", () => {
    expect(parentFsPath("")).toBeNull()
  })
})

describe("joinFsPath", () => {
  it("joins POSIX paths with a forward slash", () => {
    expect(joinFsPath("/home/me", "work")).toBe("/home/me/work")
  })

  it("does not double the separator when the base already ends with one", () => {
    expect(joinFsPath("/home/me/", "work")).toBe("/home/me/work")
    expect(joinFsPath("C:\\Users\\", "a")).toBe("C:\\Users\\a")
  })

  it("joins Windows paths with a backslash and normalizes the relative part", () => {
    expect(joinFsPath("C:\\Users", "a")).toBe("C:\\Users\\a")
    expect(joinFsPath("C:\\Users", "a/b")).toBe("C:\\Users\\a\\b")
  })

  it("returns the base unchanged for an empty relative path", () => {
    expect(joinFsPath("/base", "")).toBe("/base")
  })
})
