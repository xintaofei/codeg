import { describe, expect, it } from "vitest"

import {
  parseManualSpec,
  specKinds,
} from "@/components/settings/add-custom-agent-dialog"

describe("parseManualSpec", () => {
  it("accepts a bare distribution object", () => {
    const result = parseManualSpec(
      '{"npx": {"package": "some-agent@1.0.0", "args": ["--acp"]}}'
    )
    expect(result.error).toBeUndefined()
    expect(result.spec?.npx?.package).toBe("some-agent@1.0.0")
    // Nothing else can be inferred from a bare distribution.
    expect(result.registryId).toBeUndefined()
    expect(result.name).toBeUndefined()
  })

  it("unwraps a whole registry entry and keeps its metadata", () => {
    // Verbatim shape from the published ACP registry.
    const result = parseManualSpec(
      JSON.stringify({
        id: "goose",
        name: "goose",
        version: "1.44.0",
        description: "A local, extensible, open source AI agent",
        icon: "https://cdn.example.com/goose.svg",
        distribution: {
          binary: {
            "darwin-aarch64": {
              archive: "https://example.com/goose.tar.bz2",
              cmd: "./goose",
              args: ["acp"],
              sha256: "abc",
            },
          },
        },
      })
    )
    expect(result.error).toBeUndefined()
    expect(result.registryId).toBe("goose")
    expect(result.name).toBe("goose")
    expect(result.version).toBe("1.44.0")
    expect(result.iconUrl).toBe("https://cdn.example.com/goose.svg")
    expect(result.spec?.binary?.["darwin-aarch64"]?.sha256).toBe("abc")
  })

  it("reports malformed input instead of throwing", () => {
    expect(parseManualSpec("").error).toBe("empty")
    expect(parseManualSpec("   ").error).toBe("empty")
    expect(parseManualSpec("{not json").error).toBe("invalidJson")
    expect(parseManualSpec("[1,2,3]").error).toBe("invalidJson")
    expect(parseManualSpec('"a string"').error).toBe("invalidJson")
    // Valid JSON, but nothing codeg can launch.
    expect(parseManualSpec('{"something": "else"}').error).toBe(
      "noDistribution"
    )
  })
})

describe("specKinds", () => {
  it("prefers a native binary over npx over uvx", () => {
    expect(
      specKinds({
        npx: { package: "a@1" },
        binary: { "linux-x86_64": { archive: "https://e/a.tgz" } },
      })
    ).toEqual(["binary", "npx"])
    expect(specKinds({ uvx: { package: "a==1" } })).toEqual(["uvx"])
    expect(specKinds({ npx: { package: "a@1" } })).toEqual(["npx"])
  })

  it("ignores an empty binary map", () => {
    expect(specKinds({ binary: {} })).toEqual([])
    expect(specKinds({})).toEqual([])
  })
})
