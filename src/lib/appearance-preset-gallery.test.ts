import { createHash } from "node:crypto"
import { beforeEach, describe, expect, it, vi } from "vitest"

const callMock = vi.fn()

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: callMock }),
}))

import {
  DEFAULT_PRESET_GALLERY_INDEX_URL,
  GALLERY_CACHE_STALE_MS,
  MAX_GALLERY_ENTRIES,
  MAX_GALLERY_INDEX_BYTES,
  downloadGalleryPreset,
  filterGalleryEntries,
  galleryEntryStatus,
  galleryEntrySwatches,
  installedFromDownload,
  isGalleryCacheStale,
  normalizeGalleryIndexUrl,
  parseGalleryCache,
  parseInstalledGalleryPresets,
  parsePresetGalleryIndex,
  presetSwatches,
  readGalleryCache,
  readGalleryIndexUrl,
  refreshGalleryIndex,
  removeInstalledGalleryPreset,
  resolveGalleryFileUrl,
  serializeInstalledGalleryPresets,
  upsertInstalledGalleryPreset,
  writeGalleryIndexUrl,
  type GalleryEntry,
  type InstalledGalleryPreset,
} from "./appearance-preset-gallery"
import {
  serializeAppearancePreset,
  type AppearancePreset,
} from "./appearance-preset"
import { BUNDLED_PRESET_BY_ID } from "./appearance-presets-bundled"
import {
  STORAGE_KEY_PRESET_GALLERY_CACHE,
  STORAGE_KEY_PRESET_GALLERY_INDEX_URL,
} from "./appearance-script"

const INDEX_URL = "https://gallery.example.com/presets/index.json"

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex")
}

const sharedLook: AppearancePreset = {
  schemaVersion: 1,
  id: "shared-look",
  name: "Shared look",
  description: "Came in from the gallery.",
  author: "someone",
  base: "rose",
  mode: "dark",
  colors: { light: { primary: "#336699" }, dark: { primary: "#99ccff" } },
  code: { light: "one-light", dark: "one-dark-pro" },
}
const sharedText = serializeAppearancePreset(sharedLook)
const sharedDigest = sha256(sharedText)

function entry(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    id: "shared-look",
    name: "Shared look",
    author: "someone",
    base: "rose",
    mode: "dark",
    url: "shared-look.codeg-preset.json",
    sha256: sharedDigest,
    bytes: Buffer.byteLength(sharedText),
    swatches: { light: ["#ffffff", "#336699"], dark: ["#000000", "#99ccff"] },
    ...overrides,
  }
}

function indexText(presets: unknown[], extra: Record<string, unknown> = {}) {
  return JSON.stringify({
    schemaVersion: 1,
    name: "Test gallery",
    presets,
    ...extra,
  })
}

beforeEach(() => {
  callMock.mockReset()
  localStorage.clear()
})

describe("parsePresetGalleryIndex", () => {
  it("accepts a well-formed index and normalizes each listing", () => {
    const result = parsePresetGalleryIndex(
      indexText([
        entry(),
        entry({
          id: "other",
          name: "Other",
          url: "https://gallery.example.com/elsewhere/other.json",
          sha256: sharedDigest.toUpperCase(),
          swatches: undefined,
          bytes: undefined,
          author: undefined,
          mode: undefined,
        }),
      ]),
      INDEX_URL
    )
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.skipped).toEqual([])
    expect(result.index.name).toBe("Test gallery")
    expect(result.index.presets.map((p) => p.id)).toEqual([
      "shared-look",
      "other",
    ])
    const [first, second] = result.index.presets
    // Relative URLs resolve against the index; digests come back lowercase.
    expect(first.url).toBe(
      "https://gallery.example.com/presets/shared-look.codeg-preset.json"
    )
    expect(first.swatches).toEqual({
      light: ["#ffffff", "#336699"],
      dark: ["#000000", "#99ccff"],
    })
    expect(second.url).toBe("https://gallery.example.com/elsewhere/other.json")
    expect(second.sha256).toBe(sharedDigest)
    expect(second.swatches).toBeUndefined()
  })

  it("refuses an oversized document at the byte count, before parsing", () => {
    const result = parsePresetGalleryIndex(
      "x".repeat(MAX_GALLERY_INDEX_BYTES + 1),
      INDEX_URL
    )
    expect(result).toEqual({
      ok: false,
      issues: [
        {
          path: "",
          code: "too-large",
          detail: `${MAX_GALLERY_INDEX_BYTES + 1} > ${MAX_GALLERY_INDEX_BYTES}`,
        },
      ],
    })
  })

  it("refuses malformed JSON and non-objects", () => {
    const malformed = parsePresetGalleryIndex("{ not json", INDEX_URL)
    expect(malformed.ok).toBe(false)
    if (!malformed.ok) expect(malformed.issues[0].code).toBe("invalid-json")

    const list = parsePresetGalleryIndex("[]", INDEX_URL)
    expect(list.ok).toBe(false)
    if (!list.ok) expect(list.issues[0].code).toBe("not-object")
  })

  it("refuses a document whose shape is wrong, naming each field", () => {
    const codes = (text: string) => {
      const result = parsePresetGalleryIndex(text, INDEX_URL)
      return result.ok
        ? []
        : result.issues.map((issue) => `${issue.path}:${issue.code}`)
    }
    expect(codes(JSON.stringify({ presets: [] }))).toEqual([
      "schemaVersion:missing",
    ])
    expect(codes(JSON.stringify({ schemaVersion: 2, presets: [] }))).toEqual([
      "schemaVersion:unsupported-version",
    ])
    expect(
      codes(JSON.stringify({ schemaVersion: 1, presets: [], extra: true }))
    ).toEqual(["extra:unknown-key"])
    expect(codes(JSON.stringify({ schemaVersion: 1, presets: {} }))).toEqual([
      "presets:wrong-type",
    ])
    expect(codes(JSON.stringify({ schemaVersion: 1 }))).toEqual([
      "presets:missing",
    ])
    expect(
      codes(
        JSON.stringify({
          schemaVersion: 1,
          presets: Array.from({ length: MAX_GALLERY_ENTRIES + 1 }, () =>
            entry()
          ),
        })
      )
    ).toEqual(["presets:too-many"])
  })

  it("skips a listing it cannot trust and keeps the rest", () => {
    const result = parsePresetGalleryIndex(
      indexText([
        entry(),
        // A digest that is not SHA-256 hex.
        entry({ id: "bad-digest", sha256: "abc" }),
        // A file on another site than the index.
        entry({ id: "elsewhere", url: "https://evil.example/x.json" }),
        // A downgrade to http, and a loopback host.
        entry({ id: "plain", url: "http://gallery.example.com/x.json" }),
        entry({ id: "loop", url: "https://127.0.0.1/x.json" }),
        // A field the format does not have.
        entry({ id: "extra", tags: ["dark"] }),
        // A base palette the app does not ship.
        entry({ id: "no-base", base: "plum" }),
        // A swatch that is not a colour (a declaration-escape attempt).
        entry({
          id: "escape",
          swatches: { light: ["red; background: url(x)"] },
        }),
        // Too many swatches.
        entry({ id: "many", swatches: { light: Array(7).fill("#000000") } }),
        // A duplicate of the first id.
        entry({ name: "Shared look again" }),
        // A listing that is not an object at all.
        "nope",
      ]),
      INDEX_URL
    )
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.index.presets.map((p) => p.id)).toEqual(["shared-look"])
    expect(
      result.skipped.map((issue) => `${issue.path}:${issue.code}`)
    ).toEqual([
      "presets[1].sha256:invalid-digest",
      "presets[2].url:cross-origin",
      "presets[3].url:invalid-url",
      "presets[4].url:invalid-url",
      "presets[5].tags:unknown-key",
      "presets[6].base:unknown-base",
      "presets[7].swatches.light[0]:invalid-color",
      "presets[8].swatches.light:out-of-range",
      "presets[9].id:duplicate-id",
      "presets[10]:not-object",
    ])
  })
})

describe("URL policy", () => {
  it("resolves relative file URLs against the index and refuses the rest", () => {
    expect(resolveGalleryFileUrl("a/b.json", INDEX_URL)).toBe(
      "https://gallery.example.com/presets/a/b.json"
    )
    expect(resolveGalleryFileUrl("/root.json#frag", INDEX_URL)).toBe(
      "https://gallery.example.com/root.json"
    )
    expect(
      resolveGalleryFileUrl("http://gallery.example.com/x", INDEX_URL)
    ).toBe(null)
    expect(
      resolveGalleryFileUrl("https://u:p@gallery.example.com/x", INDEX_URL)
    ).toBe(null)
    expect(resolveGalleryFileUrl("https://localhost/x", INDEX_URL)).toBe(null)
    expect(resolveGalleryFileUrl("https://[::1]/x", INDEX_URL)).toBe(null)
    expect(resolveGalleryFileUrl("not a url at all", "not a url")).toBe(null)
  })

  it("normalizes a source the user typed and refuses what the backend would", () => {
    expect(
      normalizeGalleryIndexUrl("  https://Example.com/gallery/index.json#x ")
    ).toBe("https://example.com/gallery/index.json")
    expect(normalizeGalleryIndexUrl("")).toBe(null)
    expect(normalizeGalleryIndexUrl("http://example.com/index.json")).toBe(null)
    expect(normalizeGalleryIndexUrl("https://10.0.0.8/index.json")).toBe(null)
    expect(normalizeGalleryIndexUrl("https://user:pw@example.com/i.json")).toBe(
      null
    )
    expect(normalizeGalleryIndexUrl("file:///etc/passwd")).toBe(null)
  })

  it("stores a custom source and treats the default as nothing stored", () => {
    expect(readGalleryIndexUrl()).toBe(DEFAULT_PRESET_GALLERY_INDEX_URL)
    writeGalleryIndexUrl("https://example.com/index.json")
    expect(localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INDEX_URL)).toBe(
      "https://example.com/index.json"
    )
    expect(readGalleryIndexUrl()).toBe("https://example.com/index.json")
    writeGalleryIndexUrl(DEFAULT_PRESET_GALLERY_INDEX_URL)
    expect(localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INDEX_URL)).toBe(
      null
    )
    // A stored value that no longer passes the policy falls back to the default.
    localStorage.setItem(
      STORAGE_KEY_PRESET_GALLERY_INDEX_URL,
      "http://example.com/index.json"
    )
    expect(readGalleryIndexUrl()).toBe(DEFAULT_PRESET_GALLERY_INDEX_URL)
  })
})

describe("search and swatches", () => {
  const entries: GalleryEntry[] = [
    {
      id: "a",
      name: "Warm terminal",
      author: "Ada",
      url: "https://x/a",
      sha256: sharedDigest,
    },
    {
      id: "b",
      name: "Ink",
      description: "Monochrome and square",
      url: "https://x/b",
      sha256: sharedDigest,
    },
  ]

  it("filters case-insensitively across id, name, author and description", () => {
    expect(filterGalleryEntries(entries, "").map((e) => e.id)).toEqual([
      "a",
      "b",
    ])
    expect(filterGalleryEntries(entries, "WARM").map((e) => e.id)).toEqual([
      "a",
    ])
    expect(filterGalleryEntries(entries, "ada").map((e) => e.id)).toEqual(["a"])
    expect(filterGalleryEntries(entries, "square").map((e) => e.id)).toEqual([
      "b",
    ])
    expect(filterGalleryEntries(entries, "  b ").map((e) => e.id)).toEqual([
      "b",
    ])
    expect(filterGalleryEntries(entries, "zzz")).toEqual([])
  })

  it("draws a card from the listing's own colours, else from its base palette", () => {
    expect(
      galleryEntrySwatches(
        { base: "rose", swatches: { light: ["#fff", "#000"] } },
        false
      )
    ).toEqual(["#fff", "#000"])
    // No dark swatches listed: neutral surfaces around the base's dark primary.
    const dark = galleryEntrySwatches(
      { base: "rose", swatches: { light: ["#fff"] } },
      true
    )
    expect(dark).toHaveLength(4)
    expect(dark[0]).toBe("oklch(0.145 0 0)")
    expect(dark[2]).not.toBe(galleryEntrySwatches({ base: "blue" }, true)[2])
    // An installed preset paints from its own document.
    const warm = BUNDLED_PRESET_BY_ID["warm-terminal"]
    expect(presetSwatches(warm, true)).toEqual([
      warm.colors.dark?.background,
      warm.colors.dark?.sidebar,
      warm.colors.dark?.primary,
      warm.colors.dark?.foreground,
    ])
    expect(presetSwatches(BUNDLED_PRESET_BY_ID["default"], false)).toEqual([
      "oklch(1 0 0)",
      "oklch(0.985 0 0)",
      "oklch(0.205 0 0)",
      "oklch(0.145 0 0)",
    ])
  })
})

describe("installed library", () => {
  const installed: InstalledGalleryPreset = {
    preset: sharedLook,
    url: "https://gallery.example.com/presets/shared-look.codeg-preset.json",
    indexUrl: INDEX_URL,
    sha256: sharedDigest,
    installedAt: "2026-09-06T10:00:00.000Z",
  }

  it("reports available, installed and update from the digest alone", () => {
    expect(galleryEntryStatus({ sha256: sharedDigest }, undefined)).toBe(
      "available"
    )
    expect(galleryEntryStatus({ sha256: sharedDigest }, installed)).toBe(
      "installed"
    )
    expect(galleryEntryStatus({ sha256: sha256("changed") }, installed)).toBe(
      "update"
    )
  })

  it("installs by appending, updates in place, removes only that id", () => {
    const other: InstalledGalleryPreset = {
      ...installed,
      preset: { ...sharedLook, id: "other", name: "Other" },
    }
    const one = upsertInstalledGalleryPreset([], installed)
    expect(one).toHaveLength(1)
    const two = upsertInstalledGalleryPreset(one, other)
    expect(two.map((i) => i.preset.id)).toEqual(["shared-look", "other"])

    const updated = upsertInstalledGalleryPreset(two, {
      ...installed,
      sha256: sha256("changed"),
      installedAt: "2026-09-07T10:00:00.000Z",
    })
    expect(updated.map((i) => i.preset.id)).toEqual(["shared-look", "other"])
    expect(updated[0].sha256).toBe(sha256("changed"))
    // The inputs were not mutated.
    expect(two[0].sha256).toBe(sharedDigest)

    const removed = removeInstalledGalleryPreset(updated, "shared-look")
    expect(removed.map((i) => i.preset.id)).toEqual(["other"])
    expect(removeInstalledGalleryPreset(removed, "missing")).toEqual(removed)
  })

  it("round-trips through storage and forgets what no longer validates", () => {
    const text = serializeInstalledGalleryPresets([installed])
    expect(parseInstalledGalleryPresets(text)).toEqual([installed])

    const tampered = JSON.stringify([
      installed,
      // Duplicate id: the first wins.
      { ...installed, sha256: sha256("other") },
      // A preset document that fails validation.
      {
        ...installed,
        preset: {
          ...sharedLook,
          id: "bad",
          colors: { dark: { primary: "plum" } },
        },
      },
      // A digest that is not SHA-256 hex.
      { ...installed, preset: { ...sharedLook, id: "nohash" }, sha256: "nope" },
      // Missing provenance.
      { preset: { ...sharedLook, id: "orphan" } },
      "garbage",
    ])
    expect(
      parseInstalledGalleryPresets(tampered).map((i) => i.preset.id)
    ).toEqual(["shared-look"])
    expect(parseInstalledGalleryPresets("not json")).toEqual([])
    expect(parseInstalledGalleryPresets(null)).toEqual([])
    expect(parseInstalledGalleryPresets("{}")).toEqual([])
  })
})

describe("saved index", () => {
  it("accepts a saved copy of this index only, within the cap", () => {
    const cache = {
      indexUrl: INDEX_URL,
      fetchedAt: "2026-09-06T10:00:00.000Z",
      text: "{}",
    }
    expect(parseGalleryCache(JSON.stringify(cache), INDEX_URL)).toEqual(cache)
    expect(
      parseGalleryCache(
        JSON.stringify(cache),
        "https://other.example/index.json"
      )
    ).toBe(null)
    expect(
      parseGalleryCache(
        JSON.stringify({
          ...cache,
          text: "x".repeat(MAX_GALLERY_INDEX_BYTES + 1),
        }),
        INDEX_URL
      )
    ).toBe(null)
    expect(
      parseGalleryCache(JSON.stringify({ ...cache, fetchedAt: 5 }), INDEX_URL)
    ).toBe(null)
    expect(parseGalleryCache("nope", INDEX_URL)).toBe(null)
    expect(parseGalleryCache(null, INDEX_URL)).toBe(null)
  })

  it("is stale after a day, or when the timestamp is unreadable", () => {
    const now = Date.parse("2026-09-06T12:00:00.000Z")
    expect(isGalleryCacheStale("2026-09-06T11:00:00.000Z", now)).toBe(false)
    expect(
      isGalleryCacheStale(
        new Date(now - GALLERY_CACHE_STALE_MS - 1).toISOString(),
        now
      )
    ).toBe(true)
    expect(isGalleryCacheStale("yesterday", now)).toBe(true)
  })
})

describe("refreshGalleryIndex", () => {
  it("fetches through the backend, validates, and saves a valid index", async () => {
    const text = indexText([entry()])
    callMock.mockResolvedValue({
      text,
      sha256: sha256(text),
      bytes: Buffer.byteLength(text),
    })
    const result = await refreshGalleryIndex(INDEX_URL, () =>
      Date.parse("2026-09-06T10:00:00.000Z")
    )
    expect(callMock).toHaveBeenCalledWith("preset_gallery_fetch_index", {
      url: INDEX_URL,
    })
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.fetchedAt).toBe("2026-09-06T10:00:00.000Z")
    expect(result.index.presets[0].id).toBe("shared-look")
    expect(readGalleryCache(INDEX_URL)).toEqual({
      indexUrl: INDEX_URL,
      fetchedAt: "2026-09-06T10:00:00.000Z",
      text,
    })
  })

  it("leaves the saved copy alone when the fetched index is invalid or unreachable", async () => {
    const good = {
      indexUrl: INDEX_URL,
      fetchedAt: "2026-09-06T10:00:00.000Z",
      text: indexText([entry()]),
    }
    localStorage.setItem(STORAGE_KEY_PRESET_GALLERY_CACHE, JSON.stringify(good))

    callMock.mockResolvedValue({ text: "{ broken", sha256: "", bytes: 8 })
    const invalid = await refreshGalleryIndex(INDEX_URL)
    expect(invalid.ok).toBe(false)
    if (!invalid.ok) expect(invalid.issues[0].code).toBe("invalid-json")
    expect(readGalleryCache(INDEX_URL)).toEqual(good)

    callMock.mockRejectedValue(new Error("offline"))
    await expect(refreshGalleryIndex(INDEX_URL)).rejects.toThrow("offline")
    expect(readGalleryCache(INDEX_URL)).toEqual(good)
  })
})

describe("downloadGalleryPreset", () => {
  const listed: GalleryEntry = {
    id: "shared-look",
    name: "Shared look",
    url: "https://gallery.example.com/presets/shared-look.codeg-preset.json",
    sha256: sharedDigest,
  }

  it("hands the listed digest to the backend and accepts a matching, valid file", async () => {
    callMock.mockResolvedValue({
      text: sharedText,
      sha256: sharedDigest,
      bytes: Buffer.byteLength(sharedText),
    })
    const result = await downloadGalleryPreset(listed)
    expect(callMock).toHaveBeenCalledWith("preset_gallery_fetch_preset", {
      url: listed.url,
      sha256: sharedDigest,
    })
    expect(result).toEqual({
      ok: true,
      preset: sharedLook,
      sha256: sharedDigest,
    })
    const record = installedFromDownload(
      listed,
      { preset: sharedLook, sha256: sharedDigest },
      INDEX_URL,
      Date.parse("2026-09-06T10:00:00.000Z")
    )
    expect(record).toEqual({
      preset: sharedLook,
      url: listed.url,
      indexUrl: INDEX_URL,
      sha256: sharedDigest,
      installedAt: "2026-09-06T10:00:00.000Z",
    })
  })

  it("refuses a file whose digest is not the listed one", async () => {
    // The backend already refuses this; if it ever returned bytes under a
    // different digest, they are refused here too.
    callMock.mockResolvedValue({
      text: sharedText,
      sha256: sha256("tampered"),
      bytes: Buffer.byteLength(sharedText),
    })
    expect(await downloadGalleryPreset(listed)).toEqual({
      ok: false,
      code: "hash-mismatch",
    })
  })

  it("refuses a file that does not validate as a preset, with the field", async () => {
    const text = JSON.stringify({ ...sharedLook, script: "alert(1)" })
    callMock.mockResolvedValue({
      text,
      sha256: sharedDigest,
      bytes: Buffer.byteLength(text),
    })
    const result = await downloadGalleryPreset(listed)
    expect(result.ok).toBe(false)
    if (result.ok || result.code !== "invalid-preset")
      throw new Error("expected invalid-preset")
    expect(result.issues).toEqual([{ path: "script", code: "unknown-key" }])
  })

  it("refuses a valid preset filed under a different id", async () => {
    const text = serializeAppearancePreset({ ...sharedLook, id: "other-id" })
    callMock.mockResolvedValue({
      text,
      sha256: sharedDigest,
      bytes: Buffer.byteLength(text),
    })
    expect(await downloadGalleryPreset(listed)).toEqual({
      ok: false,
      code: "id-mismatch",
    })
  })

  it("propagates a backend refusal untouched", async () => {
    callMock.mockRejectedValue(new Error("Preset file exceeds the 32 KiB cap."))
    await expect(downloadGalleryPreset(listed)).rejects.toThrow("32 KiB cap")
  })
})
