import { describe, expect, it } from "vitest"

import type {
  BrowserCapabilities,
  BrowserPopupPayload,
  BrowserTabState,
} from "./types"

// These literals are copied from what the Rust side serializes (see the
// `wire_names_are_camel_and_kebab` test in src-tauri/src/browser/types.rs and
// the P0 puppet output). If a field is renamed on one side, `satisfies` fails
// here and the Rust test fails there.
describe("browser wire types", () => {
  it("matches the Rust serialization of BrowserTabState", () => {
    const state = {
      tabId: "t1",
      ownerWindow: "main",
      surface: "child",
      channel: "native",
      url: "https://example.com/",
      requestedUrl: "https://example.com/",
      title: "Example Domain",
      favicon: null,
      loading: false,
      canGoBack: false,
      canGoForward: false,
      origin: "https://example.com",
      zoom: 1.0,
      error: null,
      remoteHost: null,
      openerTabId: null,
    } satisfies BrowserTabState
    expect(state.surface).toBe("child")
  })

  it("matches the Rust serialization of BrowserCapabilities and popups", () => {
    const caps = {
      available: true,
      surface: "child",
      platform: "macos",
      channel: "degraded",
      reasons: ["page channel not installed yet"],
      isolatedStorage: true,
      proxy: {
        url: "http://127.0.0.1:7890",
        applies: "live",
        reason: null,
      },
      downloadsDir: "/Users/dev/Downloads",
    } satisfies BrowserCapabilities
    const popup = {
      presentation: "adopted",
      openerTabId: "t1",
      tabId: "t1-p1",
      url: "http://127.0.0.1:8765/popup.html",
      requestedSize: [520, 640],
      reason: null,
    } satisfies BrowserPopupPayload
    expect(caps.available && popup.presentation === "adopted").toBe(true)
  })
})
