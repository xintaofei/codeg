import { readFileSync } from "node:fs"
import { resolve } from "node:path"

const source = readFileSync(
  resolve(process.cwd(), "src/components/layout/aux-panel-file-tree-tab.tsx"),
  "utf8"
)

describe("aux-panel-file-tree-tab external conflict reload wiring", () => {
  it("invokes openFilePreview with { reload: true } from handleReloadExternalConflict", () => {
    const startMarker = "const handleReloadExternalConflict = useCallback("
    const start = source.indexOf(startMarker)
    expect(start).toBeGreaterThan(-1)

    // The callback body ends with the closing of useCallback's dependency
    // array. Scan to the next "}, [" which closes the inner arrow function
    // and starts the deps array — that bounds the callback we care about.
    const end = source.indexOf("}, [", start)
    expect(end).toBeGreaterThan(start)

    const block = source.slice(start, end)

    // openFilePreview must be invoked with the explicit reload option so the
    // user's "Reload" choice bypasses the workspace-context cache hit and
    // actually re-reads from disk, discarding the dirty buffer.
    expect(block).toMatch(
      /openFilePreview\([^)]*externalConflictPrompt\.path[^)]*\{[^}]*reload:\s*true[^}]*\}/
    )
  })
})

describe("aux-panel-file-tree-tab file tree presentation", () => {
  it("uses a padded transparent tree surface in the aux panel", () => {
    expect(source).toMatch(/<ScrollArea className="[^"]*px-2[^"]*py-1\.5/)
    expect(source).toMatch(
      /<FileTree[\s\S]*className="[^"]*bg-transparent[^"]*text-\[13px\]/
    )
  })

  it("keeps the Codex-style workspace tree filter local to the aux panel", () => {
    expect(source).toMatch(/placeholder=\{t\("filterPlaceholder"\)\}/)
    expect(source).toMatch(/\bfilterFileTreeNodesForQuery\b/)
    expect(source).not.toMatch(/file-workspace-panel/)
    expect(source).not.toMatch(/monaco-editor/)
  })

  it("uses compact git status markers instead of coloring whole file rows", () => {
    expect(source).toMatch(/prefix=\{getGitFileStateIndicator/)
    expect(source).not.toMatch(
      /<FileTreeFile[\s\S]*className=\{[\s\S]*getGitFileStateClassName/
    )
  })
})

describe("aux-panel-file-tree-tab external-change watcher coverage", () => {
  it("destructures the background-reload, stale, and prefetched-apply APIs", () => {
    // Catching external changes for non-active tabs requires these APIs;
    // source-grep them so a future refactor cannot silently regress to
    // active-tab-only behavior by dropping imports.
    expect(source).toMatch(/\breloadOpenFileBackground\b/)
    expect(source).toMatch(/\bmarkTabsStale\b/)
    expect(source).toMatch(/\bapplyExternalReload\b/)
  })

  it("does not poll workspaceState.seq for change detection", () => {
    // Seq-tick polling forces a full open-tab scan on every workspace
    // event — regardless of relevance — and reads each file twice
    // (resolver + reload). The change-detection watcher must instead be
    // driven by envelope subscription. previousWorkspaceSeqRef belonged
    // solely to the old seq-tick effect; its absence locks the change.
    expect(source).not.toMatch(/previousWorkspaceSeqRef\b/)
  })

  it("dispatches applyExternalReload from the change watcher to avoid double-reads", () => {
    // The resolver already paid for one readFileForEdit. Reloading via
    // openFilePreview would trigger a second read; applyExternalReload
    // writes the prefetched payload directly.
    const awaitIdx = source.indexOf("await resolveFileChangeDecision(")
    expect(awaitIdx).toBeGreaterThan(-1)
    const window = source.slice(awaitIdx, awaitIdx + 2000)
    expect(window).toMatch(/applyExternalReload\s*\(/)
  })

  it("re-reads the active tab id after each per-tab resolve await", () => {
    // Stale activeId bug: capturing activeFileTabRef.current once at the
    // start of an async scan lets a tab the user has since switched away
    // from be re-activated by a foreground reload. The active-id check
    // MUST dereference activeFileTabRef.current freshly inside the loop.
    const awaitIdx = source.indexOf("await resolveFileChangeDecision(")
    expect(awaitIdx).toBeGreaterThan(-1)
    const window = source.slice(awaitIdx, awaitIdx + 600)
    expect(window).toMatch(/tab\.id\s*===\s*[^=]*activeFileTabRef\.current/)
  })

  it("branches image tabs around the etag resolver", () => {
    // ImagePreview tabs use readFileBase64 (no etag); the etag resolver
    // would either fail (binary) or report a spurious mismatch, then
    // trigger a full base64 re-read every workspace event. The watcher
    // MUST branch on image-ness BEFORE invoking the resolver.
    const awaitIdx = source.indexOf("await resolveFileChangeDecision(")
    expect(awaitIdx).toBeGreaterThan(-1)
    const start = Math.max(0, awaitIdx - 1200)
    const block = source.slice(start, awaitIdx)
    expect(block).toMatch(/isImageFile|isImagePath|imageExtensions|IMAGE_EXT/i)
  })

  it("falls back to a full scan when an envelope signals resync_hint", () => {
    // Targeted scanning by changed_paths is the fast path. A backend
    // resync (or any envelope without changed_paths) must trigger a
    // full sweep, or external changes can be missed silently.
    expect(source).toMatch(/resync_hint/)
  })

  it("models a missing/read-failure decision in the resolver", () => {
    // Silent stale content after an external delete is unacceptable. The
    // resolver MUST surface read failure as its own decision kind so the
    // watcher can branch into reject (clean) or mark-stale (dirty),
    // instead of collapsing the catch into { kind: "none" }.
    expect(source).toMatch(/kind:\s*"missing"/)
  })

  it("dispatches rejectFileTab from the change watcher for missing decisions", () => {
    // The watcher must route the missing decision into a user-visible
    // error path for clean tabs. markTabsStale alone covers the dirty
    // case; rejectFileTab is required for the clean case so the buffer
    // is not silently preserved against a now-deleted disk file.
    expect(source).toMatch(/\brejectFileTab\b/)
  })
})
