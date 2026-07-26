import { describe, expect, it } from "vitest"

import {
  isDelegationSubsession,
  isSidebarRootConversation,
} from "./conversation-sidebar"

describe("isDelegationSubsession", () => {
  it("is true when parent_id is set", () => {
    expect(
      isDelegationSubsession({
        parent_id: 42,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(true)
  })

  it("is true when kind is delegate even without parent_id", () => {
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "delegate",
        delegation_call_id: null,
      })
    ).toBe(true)
  })

  it("is true when delegation_call_id is set", () => {
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "regular",
        delegation_call_id: "task-uuid",
      })
    ).toBe(true)
  })

  // The bug upstream PR #375 introduced then fixed in commit 1ad6f8f1: the card
  // once decided sub-sessions via `parent_id != null || depth > 0`. Worktree layout
  // indents ORDINARY root conversations at depth ≥ 1 (repo root-group /
  // worktree buckets under "Show worktree folders"), so that mislabelled plain
  // sessions as delegated children. This predicate therefore reads DB markers
  // ONLY — it never takes a UI depth argument, which makes the regression
  // structurally impossible.
  it("is false for a plain root conversation regardless of UI indent depth", () => {
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(false)
  })

  it("treats an empty delegation_call_id as no marker", () => {
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "regular",
        delegation_call_id: "",
      })
    ).toBe(false)
  })

  it("is false for a chat root", () => {
    expect(
      isDelegationSubsession({
        parent_id: null,
        kind: "chat",
        delegation_call_id: null,
      })
    ).toBe(false)
  })
})

describe("isSidebarRootConversation", () => {
  it("accepts a plain regular root", () => {
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(true)
  })

  it("accepts a chat root (the Chat section still lists it)", () => {
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "chat",
        delegation_call_id: null,
      })
    ).toBe(true)
  })

  it("rejects every delegation marker", () => {
    expect(
      isSidebarRootConversation({
        parent_id: 42,
        kind: "regular",
        delegation_call_id: null,
      })
    ).toBe(false)
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "delegate",
        delegation_call_id: null,
      })
    ).toBe(false)
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "regular",
        delegation_call_id: "task-uuid",
      })
    ).toBe(false)
  })

  // Loop-engineering runs belong to the loops workbench, mirroring the backend
  // `list_all` filter (`kind != loop`).
  it("rejects a loop row", () => {
    expect(
      isSidebarRootConversation({
        parent_id: null,
        kind: "loop",
        delegation_call_id: null,
      })
    ).toBe(false)
  })
})
