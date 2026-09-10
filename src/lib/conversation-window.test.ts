import { afterEach, describe, expect, it } from "vitest"

import {
  conversationWindowName,
  conversationWindowRoute,
  conversationWindowTarget,
  parseConversationWindowTarget,
} from "./conversation-window"

const TARGET = {
  folderId: 4,
  conversationId: 7,
  agentType: "claude_code",
} as const

afterEach(() => {
  window.history.replaceState({}, "", "/workspace")
})

describe("parseConversationWindowTarget", () => {
  it("reads the target a conversation window was opened for", () => {
    expect(
      parseConversationWindowTarget(
        "?conversationWindow=1&folderId=4&conversationId=7&agent=claude_code"
      )
    ).toEqual(TARGET)
  })

  // The plain deep link (`/workspace?folderId=…`, handled by DeepLinkBootstrap)
  // carries the same identity WITHOUT the marker, and must stay an ordinary
  // workspace: detaching it would cut that window off from `opened_tabs`.
  it("ignores a deep link that is not marked as a conversation window", () => {
    expect(
      parseConversationWindowTarget(
        "?folderId=4&conversationId=7&agent=claude_code"
      )
    ).toBeNull()
    expect(
      parseConversationWindowTarget(
        "?conversationWindow=0&folderId=4&conversationId=7&agent=claude_code"
      )
    ).toBeNull()
  })

  // A partial URL must fall through to the ordinary workspace rather than open
  // a detached window with nothing to show.
  it("rejects an incomplete target", () => {
    for (const search of [
      "?conversationWindow=1",
      "?conversationWindow=1&folderId=4&agent=claude_code",
      "?conversationWindow=1&conversationId=7&agent=claude_code",
      "?conversationWindow=1&folderId=4&conversationId=7",
      "?conversationWindow=1&folderId=x&conversationId=7&agent=claude_code",
      "?conversationWindow=1&folderId=4&conversationId=x&agent=claude_code",
      "?conversationWindow=1&folderId=4&conversationId=&agent=claude_code",
      "?conversationWindow=1&folderId=4&conversationId=0&agent=claude_code",
      "?conversationWindow=1&folderId=4&conversationId=7&agent=",
    ]) {
      expect(parseConversationWindowTarget(search), search).toBeNull()
    }
  })

  it("survives extra parameters (the remote-workspace context rides along)", () => {
    expect(
      parseConversationWindowTarget(
        "?conversationWindow=1&folderId=4&conversationId=7&agent=claude_code" +
          "&remoteConnectionId=3&remoteWindowId=abc"
      )
    ).toEqual(TARGET)
  })
})

describe("conversationWindowTarget", () => {
  it("reads the live location, so the marker survives for the window's life", () => {
    expect(conversationWindowTarget()).toBeNull()
    window.history.replaceState({}, "", conversationWindowRoute(TARGET))
    expect(conversationWindowTarget()).toEqual(TARGET)
  })
})

describe("conversationWindowRoute", () => {
  // Static export: a query on `/workspace`, never a dynamic route. Mirrors
  // `conversation_window_route` in src-tauri/src/commands/windows.rs.
  it("round-trips through the parser", () => {
    const route = conversationWindowRoute(TARGET)
    expect(route.startsWith("/workspace?")).toBe(true)
    expect(route).toContain("conversationWindow=1")
    expect(
      parseConversationWindowTarget(route.slice(route.indexOf("?")))
    ).toEqual(TARGET)
  })
})

describe("conversationWindowName", () => {
  // Keyed by conversation id alone, so re-invoking "Open in New Window"
  // focuses the window that already exists instead of stacking duplicates.
  it("is stable per conversation and distinct across conversations", () => {
    expect(conversationWindowName(7)).toBe("conversation-7")
    expect(conversationWindowName(7)).toBe(conversationWindowName(7))
    expect(conversationWindowName(7)).not.toBe(conversationWindowName(8))
  })
})
