import type { IntentEnvelope } from "@/lib/types"

describe("IntentEnvelope mirror", () => {
  it("matches the Rust field set", () => {
    const e: IntentEnvelope = {
      intent: "x",
      why: "y",
      ops: [{ tool: "shell", params: { cmd: "ls" } }],
      accept: "accepted",
      result: "ok",
      raw: "ls\nx",
    }
    expect(e.accept).toBe("accepted")
    expect(e.ops[0].tool).toBe("shell")
  })
})
