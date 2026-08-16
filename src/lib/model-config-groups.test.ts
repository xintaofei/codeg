import { describe, expect, it } from "vitest"

import {
  cursorModelListGroups,
  deriveModelGroups,
  filterModelGroups,
  flattenModelGroups,
  isModelConfigOption,
  modelListGroups,
  type ModelOptionGroup,
} from "./model-config-groups"
import type {
  SessionConfigOptionInfo,
  SessionConfigSelectOptionInfo,
} from "@/lib/types"

// Returns the option with its `kind` narrowed to the select arm, so tests can
// keep poking at `kind.groups` / `kind.options` directly.
function modelOption(
  options: SessionConfigSelectOptionInfo[],
  overrides: Partial<Omit<SessionConfigOptionInfo, "kind">> = {}
): SessionConfigOptionInfo & {
  kind: Extract<SessionConfigOptionInfo["kind"], { type: "select" }>
} {
  return {
    id: "model",
    name: "Model",
    description: null,
    category: null,
    kind: {
      type: "select",
      current_value: options[0]?.value ?? "",
      options,
      groups: [],
    },
    ...overrides,
  }
}

function opt(
  value: string,
  name = value,
  description: string | null = null
): SessionConfigSelectOptionInfo {
  return { value, name, description }
}

describe("isModelConfigOption", () => {
  it("matches the model option by id", () => {
    expect(isModelConfigOption(modelOption([opt("a")]))).toBe(true)
  })

  it("matches by category when id differs", () => {
    expect(
      isModelConfigOption(
        modelOption([opt("a")], { id: "primary-model", category: "model" })
      )
    ).toBe(true)
  })

  it("rejects the mode picker and other options", () => {
    expect(
      isModelConfigOption(
        modelOption([opt("a")], { id: "mode", category: "mode" })
      )
    ).toBe(false)
    expect(isModelConfigOption(modelOption([opt("a")], { id: "effort" }))).toBe(
      false
    )
  })
})

describe("deriveModelGroups", () => {
  it("returns null for non-model options", () => {
    const mode = modelOption([opt("anthropic/opus")], {
      id: "mode",
      category: "mode",
    })
    expect(deriveModelGroups(mode)).toBeNull()
  })

  it("returns null when no value carries a slash (stays flat)", () => {
    const option = modelOption([
      opt("default", "Default"),
      opt("opus", "Opus"),
      opt("haiku", "Haiku"),
    ])
    expect(deriveModelGroups(option)).toBeNull()
  })

  it("respects server-provided groups verbatim (returns null)", () => {
    const option = modelOption([opt("anthropic/opus")])
    option.kind.groups = [
      {
        group: "anthropic",
        name: "Anthropic",
        options: [opt("anthropic/opus", "Opus")],
      },
    ]
    expect(deriveModelGroups(option)).toBeNull()
  })

  it("groups by the first slash and preserves first-seen order", () => {
    const option = modelOption([
      opt("anthropic/claude-opus"),
      opt("openai/gpt-4o"),
      opt("anthropic/claude-sonnet"),
      opt("google/gemini-2.0"),
    ])
    const groups = deriveModelGroups(option)
    expect(groups?.map((g) => g.name)).toEqual([
      "anthropic",
      "openai",
      "google",
    ])
    const anthropic = groups?.find((g) => g.name === "anthropic")
    // Labels strip the redundant `anthropic/` prefix (name repeated the value).
    expect(anthropic?.options.map((o) => o.name)).toEqual([
      "claude-opus",
      "claude-sonnet",
    ])
    // Values are never rewritten — only the display label is stripped.
    expect(anthropic?.options.map((o) => o.value)).toEqual([
      "anthropic/claude-opus",
      "anthropic/claude-sonnet",
    ])
  })

  it("uses the shared display-name prefix as header and strips it from rows", () => {
    // Real OpenCode shape: values are `opencode/…` (its id prefix), but every
    // display name repeats a human `OpenCode Zen/…`. The shared NAME prefix wins
    // for the header and is stripped from each row so it isn't shown twice.
    const option = modelOption([
      opt("opencode/big-pickle", "OpenCode Zen/Big Pickle"),
      opt("opencode/claude-haiku", "OpenCode Zen/Claude Haiku"),
      opt("anthropic/claude-opus", "anthropic/claude-opus"),
    ])
    const groups = deriveModelGroups(option)
    const zen = groups?.find((g) => g.key === "opencode")
    expect(zen?.name).toBe("OpenCode Zen")
    expect(zen?.options.map((o) => o.name)).toEqual([
      "Big Pickle",
      "Claude Haiku",
    ])
    // Values stay the full id — only the label changed.
    expect(zen?.options.map((o) => o.value)).toEqual([
      "opencode/big-pickle",
      "opencode/claude-haiku",
    ])
  })

  it("falls back to the value-id prefix as header when names don't share one", () => {
    const option = modelOption([
      opt("anthropic/claude-opus", "Claude Opus"),
      opt("anthropic/claude-sonnet", "Claude Sonnet"),
      opt("openai/gpt-4o", "GPT-4o"),
    ])
    const groups = deriveModelGroups(option)
    const anthropic = groups?.find((g) => g.key === "anthropic")
    // Clean names have no "/" → nothing redundant → header is the id prefix and
    // labels are left untouched.
    expect(anthropic?.name).toBe("anthropic")
    expect(anthropic?.options.map((o) => o.name)).toEqual([
      "Claude Opus",
      "Claude Sonnet",
    ])
  })

  it("floats prefix-less values into a leading headerless bucket", () => {
    const option = modelOption([
      opt("default", "Default"),
      opt("anthropic/opus", "anthropic/opus"),
      opt("openai/gpt", "openai/gpt"),
    ])
    const groups = deriveModelGroups(option) as ModelOptionGroup[]
    expect(groups[0]).toMatchObject({ name: null })
    expect(groups[0].options.map((o) => o.name)).toEqual(["Default"])
    expect(groups.slice(1).map((g) => g.name)).toEqual(["anthropic", "openai"])
  })

  it("groups a lone provider, stripping the prefix repeated on every row", () => {
    const option = modelOption([opt("anthropic/opus"), opt("anthropic/sonnet")])
    const groups = deriveModelGroups(option)
    expect(groups?.map((g) => g.name)).toEqual(["anthropic"])
    expect(groups?.[0].options.map((o) => o.name)).toEqual(["opus", "sonnet"])
  })

  it("strips the shared prefix for a single-provider OpenCode list (R2)", () => {
    // Only one value prefix, nothing floating — must still drop the repeated
    // `OpenCode Zen/` shown on every row (regression for the real OpenCode shape
    // when the list contains only that provider).
    const option = modelOption([
      opt("opencode/big-pickle", "OpenCode Zen/Big Pickle"),
      opt("opencode/claude-haiku", "OpenCode Zen/Claude Haiku"),
    ])
    const groups = deriveModelGroups(option)
    expect(groups?.map((g) => g.name)).toEqual(["OpenCode Zen"])
    expect(groups?.[0].options.map((o) => o.name)).toEqual([
      "Big Pickle",
      "Claude Haiku",
    ])
    expect(groups?.[0].options.map((o) => o.value)).toEqual([
      "opencode/big-pickle",
      "opencode/claude-haiku",
    ])
  })

  it("keeps a lone provider flat when its names carry no repeated prefix", () => {
    // value has a `provider/` prefix but the display names are already clean →
    // nothing redundant to strip, so no lone header is forced.
    const option = modelOption([
      opt("anthropic/opus", "Opus"),
      opt("anthropic/sonnet", "Sonnet"),
    ])
    expect(deriveModelGroups(option)).toBeNull()
  })

  it("does not mistake a lone slashed display name for a provider prefix", () => {
    // The `meta` group has a single row whose display-name slash is NOT the
    // provider (head `Big` ≠ value prefix `meta`) → leave the label intact.
    const option = modelOption([
      opt("anthropic/claude-opus", "anthropic/claude-opus"),
      opt("meta/llama", "Big/Thing"),
    ])
    const groups = deriveModelGroups(option)
    const meta = groups?.find((g) => g.key === "meta")
    expect(meta?.name).toBe("meta")
    expect(meta?.options.map((o) => o.name)).toEqual(["Big/Thing"])
  })

  it("groups a lone provider once a prefix-less value floats beside it", () => {
    const option = modelOption([
      opt("default", "Default"),
      opt("anthropic/opus"),
    ])
    const groups = deriveModelGroups(option)
    expect(groups?.map((g) => g.name)).toEqual([null, "anthropic"])
  })

  it("groups multi-segment values under their first segment", () => {
    const option = modelOption([
      opt("openrouter/anthropic/claude"),
      opt("openrouter/openai/gpt"),
      opt("ollama/llama3"),
    ])
    const groups = deriveModelGroups(option)
    expect(groups?.map((g) => g.name)).toEqual(["openrouter", "ollama"])
    const router = groups?.find((g) => g.name === "openrouter")
    // Only the first `openrouter/` token is stripped; the sub-path remains.
    expect(router?.options.map((o) => o.name)).toEqual([
      "anthropic/claude",
      "openai/gpt",
    ])
  })

  it("does not strip a human label that does not repeat the prefix", () => {
    const option = modelOption([
      opt("anthropic/claude-opus", "Claude Opus"),
      opt("openai/gpt-4o", "GPT-4o"),
    ])
    const groups = deriveModelGroups(option)
    expect(groups?.flatMap((g) => g.options.map((o) => o.name))).toEqual([
      "Claude Opus",
      "GPT-4o",
    ])
  })

  it("treats leading/trailing slashes as ungroupable (floating)", () => {
    const option = modelOption([
      opt("/leading", "/leading"),
      opt("trailing/", "trailing/"),
      opt("anthropic/opus", "anthropic/opus"),
    ])
    const groups = deriveModelGroups(option) as ModelOptionGroup[]
    // `/leading` and `trailing/` have no usable prefix → headerless bucket.
    expect(groups[0]).toMatchObject({ name: null })
    expect(groups[0].options.map((o) => o.value)).toEqual([
      "/leading",
      "trailing/",
    ])
    expect(groups.slice(1).map((g) => g.name)).toEqual(["anthropic"])
  })
})

const SAMPLE_GROUPS: ModelOptionGroup[] = [
  { key: "__ungrouped__", name: null, options: [opt("default", "Default")] },
  {
    key: "anthropic",
    name: "anthropic",
    options: [opt("anthropic/opus", "opus"), opt("anthropic/sonnet", "sonnet")],
  },
  {
    key: "openai",
    name: "openai",
    options: [opt("openai/gpt-4o", "gpt-4o")],
  },
]

describe("modelListGroups", () => {
  it("uses the derived provider groups when applicable", () => {
    const option = modelOption([
      opt("anthropic/opus", "anthropic/opus"),
      opt("openai/gpt-4o", "openai/gpt-4o"),
    ])
    expect(modelListGroups(option).map((g) => g.name)).toEqual([
      "anthropic",
      "openai",
    ])
  })

  it("preserves server-provided groups (does NOT flatten them)", () => {
    // The agent shipped its own grouping → derive returns null; the picker must
    // keep those groups, not collapse to one headerless bucket.
    const option = modelOption([])
    option.kind.groups = [
      {
        group: "fast",
        name: "Fast",
        options: [opt("a", "A"), opt("b", "B")],
      },
      { group: "smart", name: "Smart", options: [opt("c", "C")] },
    ]
    const groups = modelListGroups(option)
    expect(groups.map((g) => g.name)).toEqual(["Fast", "Smart"])
    expect(groups.flatMap((g) => g.options.map((o) => o.value))).toEqual([
      "a",
      "b",
      "c",
    ])
  })

  it("falls back to a single headerless group for a flat list", () => {
    const option = modelOption([opt("a", "A"), opt("b", "B"), opt("c", "C")])
    const groups = modelListGroups(option)
    expect(groups).toHaveLength(1)
    expect(groups[0].name).toBeNull()
    expect(groups[0].options.map((o) => o.value)).toEqual(["a", "b", "c"])
  })
})

describe("cursorModelListGroups", () => {
  it("puts Cursor's real default value in a CLI-default group without rewriting ids", () => {
    const option = modelOption([
      opt("default", "Auto"),
      opt("gpt-5.3-codex", "Codex 5.3"),
      opt("claude-opus-5", "Opus 5"),
    ])

    const groups = cursorModelListGroups(option, {
      groupName: "CLI defaults",
      optionName: "Auto (default)",
    })

    expect(groups).toEqual([
      {
        key: "__cursor_cli_defaults__",
        name: "CLI defaults",
        options: [
          { value: "default", name: "Auto (default)", description: null },
        ],
      },
      {
        key: "__cursor_models__",
        name: null,
        options: [
          { value: "gpt-5.3-codex", name: "Codex 5.3", description: null },
          { value: "claude-opus-5", name: "Opus 5", description: null },
        ],
      },
    ])
  })

  it("recognizes Cursor's compatibility-mode default tuple but leaves other agents alone", () => {
    const option = modelOption([
      opt("default[]", "Auto"),
      opt("gpt-5.3-codex[reasoning=high,fast=true]", "Codex 5.3 High Fast"),
    ])
    const labels = {
      groupName: "CLI defaults",
      optionName: "Auto (default)",
    }

    expect(cursorModelListGroups(option, labels)?.[0].options[0].value).toBe(
      "default[]"
    )
    expect(modelListGroups(option)[0].name).toBeNull()
    expect(modelListGroups(option)[0].options[0].name).toBe("Auto")
  })
})

describe("filterModelGroups", () => {
  it("returns the groups unchanged for an empty query", () => {
    expect(filterModelGroups(SAMPLE_GROUPS, "   ")).toBe(SAMPLE_GROUPS)
  })

  it("matches on the display name (case-insensitive) and drops empty groups", () => {
    const result = filterModelGroups(SAMPLE_GROUPS, "OPUS")
    // Only "opus" (anthropic) matches; floating + openai groups drop out, and
    // the sibling "sonnet" is filtered out of the anthropic group.
    expect(result.map((g) => g.key)).toEqual(["anthropic"])
    expect(result[0].options.map((o) => o.value)).toEqual(["anthropic/opus"])
  })

  it("matches on the value id even when the label was stripped", () => {
    // The label is "gpt-4o" but the value is "openai/gpt-4o" — querying the
    // provider still finds it.
    const result = filterModelGroups(SAMPLE_GROUPS, "openai")
    expect(result.map((g) => g.key)).toEqual(["openai"])
  })

  it("returns an empty list when nothing matches", () => {
    expect(filterModelGroups(SAMPLE_GROUPS, "zzz")).toEqual([])
  })

  it("keeps and searches every distinct Cursor compatibility variant", () => {
    const variants: ModelOptionGroup[] = [
      {
        key: "cursor",
        name: null,
        options: [
          opt("base[reasoning=high,fast=true]", "Base High Fast"),
          opt("base[context=1m,thinking=true]", "Base 1M Thinking"),
          opt("base[reasoning=extra-high]", "Base Extra High"),
          opt("private-base", "Private Base (NO ZDR)"),
        ],
      },
    ]

    expect(flattenModelGroups(variants)).toHaveLength(4)
    expect(filterModelGroups(variants, "fast")[0].options[0].value).toBe(
      "base[reasoning=high,fast=true]"
    )
    expect(filterModelGroups(variants, "thinking")[0].options[0].value).toBe(
      "base[context=1m,thinking=true]"
    )
    expect(filterModelGroups(variants, "1m")[0].options[0].value).toBe(
      "base[context=1m,thinking=true]"
    )
    expect(filterModelGroups(variants, "extra high")[0].options[0].value).toBe(
      "base[reasoning=extra-high]"
    )
    expect(filterModelGroups(variants, "no zdr")[0].options[0].value).toBe(
      "private-base"
    )
  })
})

describe("flattenModelGroups", () => {
  it("emits a header row per named group and one row per option", () => {
    const rows = flattenModelGroups(SAMPLE_GROUPS)
    expect(
      rows.map((r) => (r.kind === "header" ? `#${r.name}` : r.option.value))
    ).toEqual([
      // The floating bucket (name === null) contributes NO header row.
      "default",
      "#anthropic",
      "anthropic/opus",
      "anthropic/sonnet",
      "#openai",
      "openai/gpt-4o",
    ])
  })

  it("namespaces option keys by group so duplicate values never collide", () => {
    const rows = flattenModelGroups([
      { key: "a", name: "a", options: [opt("x/m", "m")] },
      { key: "b", name: "b", options: [opt("x/m", "m")] },
    ])
    const optionKeys = rows.filter((r) => r.kind === "option").map((r) => r.key)
    expect(new Set(optionKeys).size).toBe(optionKeys.length)
  })
})
