import { describe, expect, it } from "vitest"

import {
  conversationIdFromDraftKey,
  shouldApplyRemoteDraft,
} from "./composer-draft-sync"

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
