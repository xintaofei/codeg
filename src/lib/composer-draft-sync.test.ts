import { describe, expect, it } from "vitest"

import {
  conversationIdFromDraftKey,
  shouldApplyRemoteDraft,
} from "./composer-draft-sync"
import {
  attachmentsEqual,
  collectEditorFileAttachments,
  draftSnapshotKey,
  fileUriToPath,
  imageToDraftAttachment,
  isUploadJailPath,
} from "./composer-draft-attachments"

describe("conversationIdFromDraftKey", () => {
  it("accepts persisted conversation keys only", () => {
    expect(conversationIdFromDraftKey("conv:12")).toBe(12)
    expect(conversationIdFromDraftKey("conv:1")).toBe(1)
  })

  it("rejects new-tab drafts and junk", () => {
    expect(conversationIdFromDraftKey("draft:abc")).toBeNull()
    expect(conversationIdFromDraftKey("new")).toBeNull()
    expect(conversationIdFromDraftKey("conv:")).toBeNull()
    expect(conversationIdFromDraftKey("conv:-3")).toBeNull()
    expect(conversationIdFromDraftKey("conv:0")).toBeNull()
    expect(conversationIdFromDraftKey(null)).toBeNull()
  })
})

describe("shouldApplyRemoteDraft", () => {
  const localOrigin = "desk-1"

  it("ignores the writer's own echo", () => {
    expect(
      shouldApplyRemoteDraft({
        remoteRevision: 9,
        lastAppliedRevision: 1,
        remoteOrigin: localOrigin,
        localOrigin,
      })
    ).toBe(false)
  })

  it("applies a newer revision from another client", () => {
    expect(
      shouldApplyRemoteDraft({
        remoteRevision: 4,
        lastAppliedRevision: 3,
        remoteOrigin: "phone-1",
        localOrigin,
      })
    ).toBe(true)
  })

  it("drops an equal or older revision", () => {
    expect(
      shouldApplyRemoteDraft({
        remoteRevision: 3,
        lastAppliedRevision: 3,
        remoteOrigin: "phone-1",
        localOrigin,
      })
    ).toBe(false)
    expect(
      shouldApplyRemoteDraft({
        remoteRevision: 2,
        lastAppliedRevision: 3,
        remoteOrigin: "phone-1",
        localOrigin,
      })
    ).toBe(false)
  })
})

describe("composer draft attachments", () => {
  it("fingerprints text plus refs", () => {
    const a = draftSnapshotKey("hi", [
      { id: "1", kind: "file", name: "a.md", uri: "file:///tmp/a.md" },
    ])
    const b = draftSnapshotKey("hi", [
      { id: "1", kind: "file", name: "a.md", uri: "file:///tmp/a.md" },
    ])
    const c = draftSnapshotKey("hi", [])
    expect(a).toBe(b)
    expect(a).not.toBe(c)
    expect(attachmentsEqual([], [])).toBe(true)
  })

  it("decodes file uris and recognizes the uploads jail", () => {
    expect(fileUriToPath("file:///C:/Users/a/.codeg/uploads/anon/x.png")).toBe(
      "C:/Users/a/.codeg/uploads/anon/x.png"
    )
    expect(isUploadJailPath("C:/Users/a/.codeg/uploads/anon/x.png")).toBe(true)
    expect(isUploadJailPath("C:/Users/a/Desktop/shot.png")).toBe(false)
  })

  it("only drafts images that already have a jail path", () => {
    expect(
      imageToDraftAttachment({
        id: "image:1",
        type: "image",
        data: "abc",
        uri: "file:///C:/Users/a/.codeg/uploads/anon/x.png",
        name: "x.png",
        mimeType: "image/png",
      })?.path
    ).toContain("uploads")
    expect(
      imageToDraftAttachment({
        id: "image:2",
        type: "image",
        data: "abc",
        uri: "file:///C:/Users/a/Desktop/x.png",
        name: "x.png",
        mimeType: "image/png",
      })
    ).toBeNull()
  })

  it("collects file:// badges from a composer document", () => {
    const files = collectEditorFileAttachments({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            {
              type: "reference",
              attrs: {
                refType: "file",
                label: "notes.md",
                uri: "file:///tmp/notes.md",
              },
            },
            {
              type: "reference",
              attrs: {
                refType: "file",
                label: "embedded",
                uri: "codeg://embedded/1",
              },
            },
          ],
        },
      ],
    })
    expect(files).toEqual([
      {
        id: "file:file:///tmp/notes.md",
        kind: "file",
        name: "notes.md",
        mime: null,
        size: 0,
        path: null,
        uri: "file:///tmp/notes.md",
      },
    ])
  })
})
