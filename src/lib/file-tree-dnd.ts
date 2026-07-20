/**
 * Shared drag-and-drop contract for the workspace file tree. A single MIME type
 * and payload shape are read by two independent drop targets:
 *  - the tree itself (drop a node onto a directory → move it there), and
 *  - the message composer (drop a node onto the input → insert a file reference).
 *
 * The payload is serialized as JSON on a private MIME type so it can't be
 * confused with an OS file drop (`dataTransfer.files` / the `"Files"` type) or a
 * plain-text drop. A `text/plain` fallback (the absolute path) is set alongside
 * so dropping onto a terminal or a plain text field still yields something
 * useful.
 */

/** Private MIME type carrying the JSON {@link FileTreeDragPayload}. Lowercase to
 * match how `DataTransfer.types` normalizes custom types. */
export const FILE_TREE_DND_MIME = "application/x-codeg-tree-entry"

/** The five kinds of inline reference the composer can embed — mirrors the
 * tree node kinds it can carry. */
export interface FileTreeDragPayload {
  /** Absolute workspace root path the entry lives under. */
  rootPath: string
  /** Entry path relative to {@link rootPath} (forward slashes). */
  relPath: string
  /** Absolute path of the entry (`rootPath` joined with `relPath`). */
  absPath: string
  /** Leaf name of the entry. */
  name: string
  kind: "file" | "dir"
}

/**
 * The subset of `DataTransfer` the codec touches. Declaring it structurally
 * keeps the helpers unit-testable with a plain mock (jsdom's `DataTransfer` does
 * not implement `setData`/`getData` faithfully).
 */
export interface DragDataLike {
  setData: (format: string, data: string) => void
  getData: (format: string) => string
  readonly types: ReadonlyArray<string>
}

/**
 * Write the payload onto a drag's `dataTransfer`: the private JSON type (read
 * back by our drop targets) plus a `text/plain` absolute-path fallback for
 * foreign drop targets. Callers set `effectAllowed` separately since the tree
 * ("move") and composer ("copy") diverge.
 */
export function writeFileTreeDragData(
  dataTransfer: DragDataLike,
  payload: FileTreeDragPayload
): void {
  dataTransfer.setData(FILE_TREE_DND_MIME, JSON.stringify(payload))
  dataTransfer.setData("text/plain", payload.absPath)
}

/**
 * Whether a drag carries a file-tree payload. Reads `types` (available in
 * `dragover`, unlike `getData`), so a drop target can decide to accept the drop
 * before it lands.
 */
export function hasFileTreeDragType(
  dataTransfer: Pick<DragDataLike, "types"> | null | undefined
): boolean {
  if (!dataTransfer) return false
  // `types` entries are lowercased by the platform; compare accordingly.
  return Array.from(dataTransfer.types).some(
    (type) => type.toLowerCase() === FILE_TREE_DND_MIME
  )
}

/**
 * Parse the file-tree payload from a completed drop, or null when the drag
 * carries none or the JSON is malformed / the wrong shape. Only valid to call in
 * a `drop` handler (where `getData` returns data).
 */
export function readFileTreeDragPayload(
  dataTransfer: DragDataLike | null | undefined
): FileTreeDragPayload | null {
  if (!dataTransfer) return null
  const raw = dataTransfer.getData(FILE_TREE_DND_MIME)
  if (!raw) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== "object") return null
  const candidate = parsed as Record<string, unknown>
  const { rootPath, relPath, absPath, name, kind } = candidate
  if (
    typeof rootPath !== "string" ||
    typeof relPath !== "string" ||
    typeof absPath !== "string" ||
    typeof name !== "string" ||
    (kind !== "file" && kind !== "dir")
  ) {
    return null
  }
  return { rootPath, relPath, absPath, name, kind }
}

/**
 * DOM attribute marking a directory drop zone. Its value is the destination
 * directory path relative to the workspace root (`""` = the root itself).
 */
export const FILE_TREE_DROP_DIR_ATTR = "data-tree-drop-dir"
/**
 * DOM attribute marking the message composer as a drop zone. Its value is the
 * composer's attachment tab id, used to route a dropped entry to that specific
 * session's input.
 */
export const FILE_TREE_DROP_COMPOSER_ATTR = "data-tree-drop-composer"

/** Where a file-tree drag landed, resolved from the element under the drop
 *  point. `dir` moves the entry; `composer` inserts a file reference. */
export type FileTreeDropZone =
  | { kind: "dir"; destDir: string }
  | { kind: "composer"; tabId: string }

/**
 * Resolve the drop zone under `element` by walking up to the nearest marked
 * ancestor. Used by the desktop drop path, where Tauri's webview consumes the
 * HTML5 `drop` event before WebKit dispatches it to the DOM (its native
 * drag-drop handler always reports the drop as handled), so a drag is committed
 * from Tauri's own drag-drop event by hit-testing the drop coordinates rather
 * than from a `drop` handler that never runs. Returns null when the point isn't
 * over a directory row, the workspace-root row, or a composer.
 */
export function resolveFileTreeDropZone(
  element: Element | null
): FileTreeDropZone | null {
  if (!element) return null
  const dirEl = element.closest(`[${FILE_TREE_DROP_DIR_ATTR}]`)
  const composerEl = element.closest(`[${FILE_TREE_DROP_COMPOSER_ATTR}]`)
  const toDir = (el: Element): FileTreeDropZone => ({
    kind: "dir",
    destDir: el.getAttribute(FILE_TREE_DROP_DIR_ATTR) ?? "",
  })
  const toComposer = (el: Element): FileTreeDropZone | null => {
    const tabId = el.getAttribute(FILE_TREE_DROP_COMPOSER_ATTR)
    return tabId ? { kind: "composer", tabId } : null
  }
  // The tree and the composer never nest, but if both matched, prefer the
  // deeper (more specific) zone.
  if (dirEl && composerEl) {
    return composerEl.contains(dirEl) ? toDir(dirEl) : toComposer(composerEl)
  }
  if (dirEl) return toDir(dirEl)
  if (composerEl) return toComposer(composerEl)
  return null
}
