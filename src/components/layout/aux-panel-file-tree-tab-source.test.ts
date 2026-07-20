import { readFileSync } from "node:fs"
import { resolve } from "node:path"

const auxSource = readFileSync(
  resolve(process.cwd(), "src/components/layout/aux-panel-file-tree-tab.tsx"),
  "utf8"
)

const watchSource = readFileSync(
  resolve(process.cwd(), "src/hooks/use-open-file-tabs-watch.ts"),
  "utf8"
)

const providerSource = readFileSync(
  resolve(process.cwd(), "src/contexts/workspace-context.tsx"),
  "utf8"
)

describe("aux-panel-file-tree-tab no longer owns tab watching", () => {
  // The external-change reconciliation for open file tabs moved to the
  // always-mounted provider watcher (use-open-file-tabs-watch). The aux
  // panel is closed by default — any tab-reconciliation logic living here
  // would silently stop working whenever the panel is closed. Lock the
  // separation so a future change cannot quietly reintroduce it.
  it("contains no tab reconciliation or conflict machinery", () => {
    expect(auxSource).not.toMatch(/resolveFileChangeDecision/)
    expect(auxSource).not.toMatch(/announceConflict/)
    expect(auxSource).not.toMatch(/externalConflictPrompt/)
    expect(auxSource).not.toMatch(/\bapplyExternalReload\b/)
    expect(auxSource).not.toMatch(/\bmarkTabsStale\b/)
    expect(auxSource).not.toMatch(/\brejectFileTab\b/)
    expect(auxSource).not.toMatch(/\breloadOpenFileBackground\b/)
  })

  it("keeps the lazy-subtree cache invalidation envelope subscription", () => {
    // This envelope use is tree-cache bookkeeping, NOT tab watching — it
    // must stay with the tree it invalidates.
    expect(auxSource).toMatch(/subscribeWorkspaceEnvelopes/)
    expect(auxSource).toMatch(/lazyLoadedChildrenByPathRef/)
  })
})

describe("use-open-file-tabs-watch external-change coverage", () => {
  it("destructures the background-reload, stale, and prefetched-apply APIs", () => {
    // Catching external changes for non-active tabs requires these APIs;
    // source-grep them so a future refactor cannot silently regress to
    // active-tab-only behavior by dropping them.
    expect(watchSource).toMatch(/\breloadOpenFileBackground\b/)
    expect(watchSource).toMatch(/\bmarkTabsStaleBatch\b/)
    expect(watchSource).toMatch(/\bapplyExternalReload\b/)
    expect(watchSource).toMatch(/\brejectFileTab\b/)
  })

  it("keys the subscription effect on the collision-safe watch signature only", () => {
    // Blocker #13: depending on anything derived per-render from fileTabs
    // would tear down and rebuild every store subscription on each
    // keystroke. The effect must key on the JSON signature string.
    expect(watchSource).toMatch(/JSON\.stringify\(entries\)/)
    expect(watchSource).toMatch(/JSON\.parse\(watchSignature\)/)
  })

  it("dispatches applyExternalReload from the watcher to avoid double-reads", () => {
    // The resolver already paid for one readFileForEdit. Reloading via
    // openFilePreview would trigger a second read; applyExternalReload
    // writes the prefetched payload directly.
    const awaitIdx = watchSource.indexOf("await resolveFileChangeDecision(")
    expect(awaitIdx).toBeGreaterThan(-1)
    const window = watchSource.slice(awaitIdx, awaitIdx + 2000)
    expect(window).toMatch(/applyExternalReload\s*\(/)
  })

  it("re-reads the active tab id after the conflict resolve await", () => {
    // If the user switches away mid-read, the conflict must degrade to a
    // stale mark instead of popping a dialog for a tab they just left.
    expect(watchSource).toMatch(
      /tab\.id\s*===\s*activeFileTabIdRef\.current[\s\S]{0,400}enqueueExternalConflict/
    )
  })

  it("branches image tabs around the etag resolver", () => {
    // Image tabs use readFileBase64 (no etag); the etag resolver would
    // report a spurious mismatch and trigger a full base64 re-read every
    // workspace event. The watcher MUST branch on image-ness BEFORE
    // invoking the resolver.
    const awaitIdx = watchSource.indexOf("await resolveFileChangeDecision(")
    expect(awaitIdx).toBeGreaterThan(-1)
    const block = watchSource.slice(Math.max(0, awaitIdx - 1600), awaitIdx)
    expect(block).toMatch(/isImageFile/)
  })

  it("falls back to a full scan when an envelope signals resync_hint", () => {
    expect(watchSource).toMatch(/resync_hint/)
  })

  it("models a missing/read-failure decision in the resolver", () => {
    expect(watchSource).toMatch(/kind:\s*"missing"/)
  })

  it("batch-marks background tabs stale instead of reading them eagerly", () => {
    // The lazy pillar: background tabs must not cost disk reads on every
    // workspace event — one batched setState marks them stale and the
    // activation path refreshes them.
    expect(watchSource).toMatch(/staleBatch/)
    expect(watchSource).toMatch(/markTabsStaleBatch\(staleBatch\)/)
  })
})

describe("file-workspace-panel routes active-tab openers by tab folder", () => {
  const panelSource = readFileSync(
    resolve(process.cwd(), "src/components/files/file-workspace-panel.tsx"),
    "utf8"
  )

  it("diff-overview rows open files in the overview tab's folder", () => {
    // A background-folder overview must never open its rows through the
    // active workspace folder.
    expect(panelSource).toMatch(
      /openFilePreview\(path, \{ folderId: overviewFolderId \}\)/
    )
  })

  it("markdown preview links open by absolute path", () => {
    // preprocessMarkdownPaths resolves every local href against the
    // document's ABSOLUTE directory, so the click handler must hand the
    // target to openFilePreview as-is (keeping the leading slash) — never
    // strip it back to a folder-relative path.
    expect(panelSource).toMatch(/void openFilePreview\(target\)/)
    expect(panelSource).not.toMatch(
      /target\s*=\s*clean\s*\.replace\(\/\^\\\/\+\//
    )
  })

  it("excludes protocol-relative // hrefs from the local anchor branch", () => {
    // "//host/…" is a web url; collapsing it into a local path would read
    // the wrong file. The isRelative gate must reject the double-slash form.
    const gateIdx = panelSource.indexOf("const isRelative =")
    expect(gateIdx).toBeGreaterThan(-1)
    const gate = panelSource.slice(gateIdx, gateIdx + 200)
    expect(gate).toMatch(/\^\\\/\\\//)
  })

  it("treats protocol-relative // image srcs as remote, never local file IO", () => {
    // A "/"-prefixed src is an absolute local path under the new model,
    // but "//host/…" is a protocol-relative URL — routing it into
    // readFileBase64 would attempt local reads of "//Users/…"-style
    // paths. The isLocal gate must exclude the double-slash form.
    const isLocalIdx = panelSource.indexOf("const isLocal =")
    expect(isLocalIdx).toBeGreaterThan(-1)
    const gate = panelSource.slice(isLocalIdx, isLocalIdx + 300)
    expect(gate).toMatch(/\^\\\/\\\//)
  })

  it("disables local markdown resolution for UNC-hosted documents", () => {
    // A UNC document's local sub-resources cannot be resolved safely: the
    // //server/share authority is lost through the harden round trip and a
    // collapsed single-slash path would read a DIFFERENT local file. So
    // preprocessing, the image loader, and the link opener are all gated on
    // localRefsEnabled = non-UNC fileDir.
    expect(panelSource).toMatch(
      /const localRefsEnabled = !fileDir \|\| !isUncPath\(fileDir\)/
    )
    expect(panelSource).toMatch(
      /fileDir=\{localRefsEnabled \? fileDir : null\}/
    )
    expect(panelSource).toMatch(/isRelative && href && localRefsEnabled/)
  })
})

describe("aux file tree derives its selection from the absolute tab path", () => {
  const auxTreeSource = readFileSync(
    resolve(process.cwd(), "src/components/layout/aux-panel-file-tree-tab.tsx"),
    "utf8"
  )

  it("maps the absolute active path to a folder-relative tree selection", () => {
    // Tree node paths are relative to THIS panel's folder; the absolute
    // activeFilePath must be re-based (and unselected when outside). The
    // re-based path flows into the tree through focusedTreePath (which also
    // tracks the clicked directory), never as the raw absolute path.
    expect(auxTreeSource).toMatch(/findOwningFolder\(activeFilePath/)
    expect(auxTreeSource).toMatch(/setFocusedTreePath\(selectedTreePath\)/)
    expect(auxTreeSource).toMatch(/selectedPath=\{focusedTreePath\}/)
    expect(auxTreeSource).not.toMatch(/selectedPath=\{activeFilePath/)
    // The sync MUST be unconditional: a `if (selectedTreePath)` guard would
    // strand a stale file highlight when the active file is cleared or moves
    // outside this folder (selectedTreePath -> undefined).
    expect(auxTreeSource).not.toMatch(
      /if \(selectedTreePath\)\s*setFocusedTreePath/
    )
  })
})

describe("aux file tree highlights the desktop drop target from native DRAG_OVER", () => {
  const auxTreeSource = readFileSync(
    resolve(process.cwd(), "src/components/layout/aux-panel-file-tree-tab.tsx"),
    "utf8"
  )

  it("derives the drop highlight from the drag hit-test, OR-ed into each row", () => {
    // WebKit swallows the target-side DOM dragover during a native desktop
    // drag, so the directory drop highlight can't come from onDragOver — it
    // must be hit-tested into desktopDropDir and OR-ed into each row's
    // dropActive (folder rows by path, root by "").
    expect(auxTreeSource).toMatch(/setDesktopDropDir\(/)
    expect(auxTreeSource).toMatch(
      /dropActive=\{dropActive \|\| desktopDropDir === node\.path\}/
    )
    expect(auxTreeSource).toMatch(/useContext\(DesktopDropDirContext\) === ""/)
  })

  it("drives the highlight from the source-side DOM drag event (clientX/Y)", () => {
    // The primary driver: `drag` is source-side (NOT suppressed by WebKit like
    // the target-side dragover) and reports the cursor already in CSS px, so it
    // hit-tests elementFromPoint(clientX, clientY) with no coordinate scaling.
    expect(auxTreeSource).toMatch(
      /onDrag[:=].*dnd\.onEntryDrag\(event\.clientX, event\.clientY\)/
    )
    expect(auxTreeSource).toMatch(/elementFromPoint\(clientX, clientY\)/)
  })
})

describe("aux file tree drag/selection polish", () => {
  const auxTreeSource = readFileSync(
    resolve(process.cwd(), "src/components/layout/aux-panel-file-tree-tab.tsx"),
    "utf8"
  )

  it("gives the drag a compact custom ghost instead of the full-width row", () => {
    // The default drag image snapshots the now-full-width row, which reads as a
    // big translucent block with a WebKit drop shadow. A custom setDragImage
    // chip keeps the dragged item legible; lock it so a refactor can't drop back
    // to the native ghost.
    expect(auxTreeSource).toMatch(/setDragImage\(/)
    expect(auxTreeSource).toMatch(/applyCompactDragImage\(event, node\.name\)/)
  })

  it("selects the dragged row on drag start (deselecting the rest)", () => {
    // Picking a row up moves the single-select focus to it, so the dragged
    // file/dir shows the selected style and every other row loses it.
    expect(auxTreeSource).toMatch(/setFocusedTreePath\(node\.path\)/)
  })

  it("insets the row highlight from the panel edges", () => {
    // The selection/hover/drop highlight is the full-width row background;
    // horizontal padding on the tree container leaves a small left/right gutter
    // so it no longer runs edge-to-edge.
    expect(auxTreeSource).toMatch(/w-max min-w-full px-1\.5/)
  })
})

describe("workspace-context divergence-aware save guard", () => {
  it("verifies stale and unwatched dirty tabs against disk inside saveFileTab", () => {
    // Blocker #18: every write path funnels through saveFileTab, so the
    // guard must live there — a stale buffer is never written blindly.
    // Files outside every registered folder have no live watcher, so
    // their saves must ALWAYS pre-verify (`unwatched`).
    expect(providerSource).toMatch(
      /if \(\(tab\.stale \|\| unwatched\) && !options\?\.force\)[\s\S]{0,600}readFileForEdit\(io\.rootPath, io\.ioPath\)/
    )
    expect(providerSource).toMatch(
      /if \(\(tab\.stale \|\| unwatched\) && !options\?\.force\)[\s\S]{0,1200}enqueueExternalConflict/
    )
  })
})
