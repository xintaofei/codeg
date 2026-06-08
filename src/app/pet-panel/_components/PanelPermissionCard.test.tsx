import { render, screen, fireEvent, cleanup } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Stub the backend command with a never-resolving promise so `busy` stays true
// after the first click (models an in-flight response).
vi.mock("@/lib/api", () => ({
  acpRespondPermission: vi.fn(() => new Promise<void>(() => {})),
}))

import { acpRespondPermission } from "@/lib/api"
import { PanelPermissionCard } from "./PanelPermissionCard"

const permission = {
  requestId: "r1",
  toolCall: { tool_name: "Bash", rawInput: { command: "ls -la" } },
  options: [
    { option_id: "allow", name: "Allow", kind: "allow_once" },
    { option_id: "reject", name: "Reject", kind: "reject_once" },
  ],
}

describe("PanelPermissionCard", () => {
  beforeEach(() => vi.clearAllMocks())
  afterEach(() => cleanup())

  it("forwards a single response with the right ids", () => {
    render(<PanelPermissionCard connectionId="c1" permission={permission} />)
    fireEvent.click(screen.getByRole("button", { name: "Allow" }))
    expect(acpRespondPermission).toHaveBeenCalledTimes(1)
    expect(acpRespondPermission).toHaveBeenCalledWith("c1", "r1", "allow")
  })

  it("ignores rapid double-clicks while a response is in flight", () => {
    render(<PanelPermissionCard connectionId="c1" permission={permission} />)
    const allow = screen.getByRole("button", { name: "Allow" })
    fireEvent.click(allow)
    fireEvent.click(allow)
    fireEvent.click(allow)
    expect(acpRespondPermission).toHaveBeenCalledTimes(1)
  })
})
