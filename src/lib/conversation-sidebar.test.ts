import { describe, expect, it } from "vitest"
import {
  isDelegationSubsession,
  isSidebarRootConversation,
} from "./conversation-sidebar"

describe("isDelegationSubsession", () => {
  it("is false for a normal root (even if UI depth would be > 0)", () => {
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(false)
  })

  it("is true for parent_id / kind=delegate / delegation_call_id", () => {
    expect(
      isDelegationSubsession({
        parent_id: 1,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(true)
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "delegate",
        delegation_call_id: null,
      })
    ).toBe(true)
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "regular",
        delegation_call_id: "task-1",
      })
    ).toBe(true)
  })
})

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
