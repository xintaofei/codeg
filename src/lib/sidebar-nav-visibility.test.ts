import { describe, expect, it } from "vitest"

import {
  DEFAULT_SIDEBAR_NAV_VISIBILITY,
  parseSidebarNavVisibility,
} from "./sidebar-nav-visibility"

describe("parseSidebarNavVisibility", () => {
  it("falls back to every row visible when nothing is stored", () => {
    expect(parseSidebarNavVisibility(null)).toEqual({
      automations: true,
      tasks: true,
      forge: true,
    })
    expect(parseSidebarNavVisibility("")).toEqual(
      DEFAULT_SIDEBAR_NAV_VISIBILITY
    )
  })

  it("falls back on corrupt JSON and non-record shapes", () => {
    expect(parseSidebarNavVisibility("not json")).toEqual(
      DEFAULT_SIDEBAR_NAV_VISIBILITY
    )
    expect(parseSidebarNavVisibility('["automations"]')).toEqual(
      DEFAULT_SIDEBAR_NAV_VISIBILITY
    )
    expect(parseSidebarNavVisibility("null")).toEqual(
      DEFAULT_SIDEBAR_NAV_VISIBILITY
    )
  })

  it("round-trips a full record", () => {
    const stored = JSON.stringify({
      automations: false,
      tasks: true,
      forge: false,
    })
    expect(parseSidebarNavVisibility(stored)).toEqual({
      automations: false,
      tasks: true,
      forge: false,
    })
  })

  it("defaults rows missing from an older store to visible", () => {
    // Forward compatibility: a row added in a later release must show up for
    // users carrying a stored record that predates it, not vanish.
    expect(parseSidebarNavVisibility('{"forge":false}')).toEqual({
      automations: true,
      tasks: true,
      forge: false,
    })
  })

  it("drops unknown keys and non-boolean values", () => {
    const stored = JSON.stringify({
      automations: false,
      forge: "false",
      newChat: false,
    })
    expect(parseSidebarNavVisibility(stored)).toEqual({
      automations: false,
      tasks: true,
      forge: true,
    })
  })
})
