import { describe, expect, it } from "vitest"
import { isSidebarRootConversation } from "./conversation-sidebar"

describe("isSidebarRootConversation", () => {
  it("accepts a regular root", () => {
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(true)
  })

  it("rejects parent_id set", () => {
    expect(
      isSidebarRootConversation({
        parent_id: 42,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(false)
  })

  it("rejects kind=delegate even without parent_id", () => {
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "delegate",
        delegation_call_id: null,
      })
    ).toBe(false)
  })

  it("rejects delegation_call_id set", () => {
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "regular",
        delegation_call_id: "task-uuid",
      })
    ).toBe(false)
  })

  it("rejects loop kind", () => {
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "loop",
        delegation_call_id: null,
      })
    ).toBe(false)
  })
})
