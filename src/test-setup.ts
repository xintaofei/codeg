import "@testing-library/jest-dom/vitest"

// Node 24 exposes an experimental global `localStorage` property whose value is
// `undefined` unless the process receives `--localstorage-file`. It can also
// shadow jsdom's Storage accessor, so install a deterministic browser-compatible
// store for every worker without reading the broken Node accessor first.
class TestStorage implements Storage {
  readonly #values = new Map<string, string>()

  get length() {
    return this.#values.size
  }

  clear() {
    this.#values.clear()
  }

  getItem(key: string) {
    return this.#values.get(String(key)) ?? null
  }

  key(index: number) {
    return Array.from(this.#values.keys())[index] ?? null
  }

  removeItem(key: string) {
    this.#values.delete(String(key))
  }

  setItem(key: string, value: string) {
    this.#values.set(String(key), String(value))
  }
}

const testLocalStorage = new TestStorage()
Object.defineProperty(globalThis, "Storage", {
  configurable: true,
  value: TestStorage,
})
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: testLocalStorage,
})
if (typeof window !== "undefined" && window !== globalThis) {
  Object.defineProperty(window, "Storage", {
    configurable: true,
    value: TestStorage,
  })
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: testLocalStorage,
  })
}

// jsdom doesn't implement a few layout APIs that ProseMirror's EditorView
// touches on mount (used by Tiptap-based editors such as the message composer).
// Polyfill them as no-ops so headless/component editor tests can construct a
// view. Only defined when missing, so real browsers/environments are untouched.
if (typeof document !== "undefined" && !document.elementFromPoint) {
  document.elementFromPoint = () => null
}
if (typeof Element !== "undefined") {
  // jsdom doesn't implement scrollIntoView; the composer's suggestion popup
  // calls it to keep the active row visible.
  Element.prototype.scrollIntoView ??= () => {}
  // jsdom doesn't implement Pointer Capture; Radix menus/popovers touch these
  // during the pointer interactions @testing-library/user-event drives.
  Element.prototype.hasPointerCapture ??= () => false
  Element.prototype.setPointerCapture ??= () => {}
  Element.prototype.releasePointerCapture ??= () => {}
}
if (typeof globalThis !== "undefined" && !("ResizeObserver" in globalThis)) {
  // jsdom doesn't implement ResizeObserver; cmdk (the command palette used by
  // the branch/folder pickers) constructs one on mount. A no-op stub is enough
  // for headless rendering — layout callbacks never need to fire.
  ;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
}
if (typeof Range !== "undefined") {
  Range.prototype.getClientRects ??= () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: function* () {},
    }) as unknown as DOMRectList
  Range.prototype.getBoundingClientRect ??= () =>
    ({
      top: 0,
      left: 0,
      right: 0,
      bottom: 0,
      width: 0,
      height: 0,
      x: 0,
      y: 0,
    }) as DOMRect
}
