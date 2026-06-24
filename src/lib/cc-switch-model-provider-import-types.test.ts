import { describe, expect, it } from "vitest"

import type {
  CcSwitchModelProviderPreviewItem,
  ImportCcSwitchModelProvidersRequest,
} from "@/lib/types"

describe("cc-switch model provider import transport types", () => {
  it("accepts preview items and import requests", () => {
    const item: CcSwitchModelProviderPreviewItem = {
      sourceId: "codex:demo",
      sourceAppType: "codex",
      targetAgentType: "codex",
      name: "Demo",
      apiUrl: "https://api.example.com/v1",
      model: "gpt-5",
      importable: true,
      skipReason: null,
    }
    const request: ImportCcSwitchModelProvidersRequest = {
      sourceIds: [item.sourceId],
    }

    expect(request.sourceIds).toEqual(["codex:demo"])
  })
})
