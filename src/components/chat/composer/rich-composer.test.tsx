import { act, render, waitFor } from "@testing-library/react"
import { createRef } from "react"
import { describe, expect, it, vi } from "vitest"

import { RichComposer, type RichComposerHandle } from "./rich-composer"

/** Wait until the editor has mounted (immediatelyRender:false makes it async). */
async function mount(props: React.ComponentProps<typeof RichComposer> = {}) {
  const ref = createRef<RichComposerHandle>()
  const result = render(<RichComposer ref={ref} {...props} />)
  // Generous timeout: editor construction (ProseMirror + React node view) can
  // be slow under parallel worker CPU contention.
  await waitFor(() => expect(ref.current?.getEditor()).not.toBeNull(), {
    timeout: 5000,
  })
  return { ref, ...result }
}

describe("RichComposer", () => {
  it("mounts and reports an empty document via the handle", async () => {
    const { ref } = await mount()
    expect(ref.current?.isEmpty()).toBe(true)
    expect(ref.current?.getText()).toBe("")
  })

  it("paints the placeholder on the empty document", async () => {
    const { ref, container } = await mount({ placeholder: "Ask anything" })
    expect(ref.current).not.toBeNull()
    expect(
      container.querySelector('[data-placeholder="Ask anything"]')
    ).not.toBeNull()
  })

  it("exposes an accessible multiline textbox", async () => {
    const { container } = await mount({ ariaLabel: "Message" })
    const textbox = container.querySelector('[role="textbox"]')
    expect(textbox).not.toBeNull()
    expect(textbox).toHaveAttribute("aria-multiline", "true")
    expect(textbox).toHaveAttribute("aria-label", "Message")
  })

  it("round-trips text through the handle and notifies onChange", async () => {
    const onChange = vi.fn()
    const { ref } = await mount({ onChange })

    act(() => {
      ref.current?.setText("hello **world**")
    })

    // Plain text: the markdown-looking syntax is preserved literally.
    expect(ref.current?.getText()).toContain("**world**")
    expect(ref.current?.isEmpty()).toBe(false)
    expect(onChange).toHaveBeenCalled()
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1]
    expect(lastCall?.[0]).toContain("**world**")

    act(() => {
      ref.current?.clear()
    })
    expect(ref.current?.isEmpty()).toBe(true)
  })

  it("preserves CJK content through the handle", async () => {
    const { ref } = await mount()
    act(() => {
      ref.current?.setText("发送给智能体的消息")
    })
    expect(ref.current?.getText()).toContain("发送给智能体的消息")
  })

  it("initializes from defaultText without firing onChange", async () => {
    const onChange = vi.fn()
    const { ref } = await mount({
      defaultText: "# Heading",
      onChange,
    })
    // Inserted as literal text (no heading formatting).
    expect(ref.current?.getText().trim()).toBe("# Heading")
    // onCreate sets content with emitUpdate:false → no spurious change events.
    expect(onChange).not.toHaveBeenCalled()
  })
})

function dispatchKey(
  ref: React.RefObject<RichComposerHandle | null>,
  init: KeyboardEventInit
) {
  const dom = ref.current?.getEditor()?.view.dom as HTMLElement
  act(() => {
    dom.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init })
    )
  })
}

describe("RichComposer imperative inserts", () => {
  it("inserts text at the cursor (markdown syntax stays literal)", async () => {
    const { ref } = await mount()
    act(() => ref.current?.insertTextAtCursor("hello **world**"))
    expect(ref.current?.getText()).toContain("**world**")
  })

  it("inserts a reference badge and exposes it via getJSON", async () => {
    const { ref } = await mount()
    act(() =>
      ref.current?.insertReference({
        refType: "file",
        id: "a.ts",
        label: "a.ts",
        uri: "file:///a.ts",
        meta: null,
      })
    )
    expect(JSON.stringify(ref.current?.getJSON())).toContain(
      '"type":"reference"'
    )
  })

  it("hydrates the document from a Tiptap JSON doc via setDoc", async () => {
    const { ref } = await mount()
    act(() =>
      ref.current?.setDoc({
        type: "doc",
        content: [
          { type: "paragraph", content: [{ type: "text", text: "from json" }] },
        ],
      })
    )
    expect(ref.current?.getText()).toContain("from json")
    expect(ref.current?.isEmpty()).toBe(false)
  })

  it("preserves a reference badge through a getJSON → setDoc round-trip", async () => {
    const { ref } = await mount()
    act(() =>
      ref.current?.insertReference({
        refType: "file",
        id: "a.ts",
        label: "a.ts",
        uri: "file:///a.ts",
        meta: null,
      })
    )
    const doc = ref.current!.getJSON()
    act(() => ref.current?.clear())
    expect(ref.current?.isEmpty()).toBe(true)
    act(() => ref.current?.setDoc(doc))
    expect(JSON.stringify(ref.current?.getJSON())).toContain(
      '"type":"reference"'
    )
  })
})

describe("RichComposer configurable submit / newline", () => {
  it("submits on a plain Enter by default", async () => {
    const onSubmit = vi.fn()
    const { ref } = await mount({ onSubmit })
    dispatchKey(ref, { key: "Enter" })
    expect(onSubmit).toHaveBeenCalledTimes(1)
  })

  it("treats Enter as a newline when submitShortcut is mod+enter", async () => {
    const onSubmit = vi.fn()
    const { ref } = await mount({ onSubmit, submitShortcut: "mod+enter" })
    dispatchKey(ref, { key: "Enter" })
    expect(onSubmit).not.toHaveBeenCalled()
    dispatchKey(ref, { key: "Enter", metaKey: true })
    expect(onSubmit).toHaveBeenCalledTimes(1)
  })

  it("inserts a hard break on Shift+Enter without submitting", async () => {
    const onSubmit = vi.fn()
    const { ref } = await mount({ onSubmit })
    act(() => ref.current?.focus())
    dispatchKey(ref, { key: "Enter", shiftKey: true })
    expect(onSubmit).not.toHaveBeenCalled()
    expect(JSON.stringify(ref.current?.getJSON())).toContain(
      '"type":"hardBreak"'
    )
  })

  it("does not submit while an external menu is open", async () => {
    const onSubmit = vi.fn()
    const { ref } = await mount({ onSubmit, isExternalMenuOpen: true })
    dispatchKey(ref, { key: "Enter" })
    expect(onSubmit).not.toHaveBeenCalled()
  })

  it("submits on a custom non-Enter binding (Tab)", async () => {
    const onSubmit = vi.fn()
    const { ref } = await mount({ onSubmit, submitShortcut: "tab" })
    dispatchKey(ref, { key: "Tab" })
    expect(onSubmit).toHaveBeenCalledTimes(1)
  })

  it("breaks on a custom newline binding (Shift+Tab) without submitting", async () => {
    const onSubmit = vi.fn()
    const { ref } = await mount({ onSubmit, newlineShortcut: "shift+tab" })
    act(() => ref.current?.focus())
    dispatchKey(ref, { key: "Tab", shiftKey: true })
    expect(onSubmit).not.toHaveBeenCalled()
    expect(JSON.stringify(ref.current?.getJSON())).toContain(
      '"type":"hardBreak"'
    )
  })

  it("does not swallow Enter when no onSubmit handler is provided", async () => {
    const { ref } = await mount()
    act(() => ref.current?.setText("hello"))
    act(() => ref.current?.focus())
    dispatchKey(ref, { key: "Enter" })
    // Enter fell through to the editor default (paragraph split), not swallowed.
    expect(ref.current?.getJSON().content?.length).toBeGreaterThanOrEqual(2)
  })
})

/**
 * Dispatch a keydown and return the event so the caller can inspect
 * `defaultPrevented` — i.e. whether the composer consumed the key. (Returning
 * true from ProseMirror's handleKeyDown calls preventDefault.)
 */
function pressKey(dom: HTMLElement, init: KeyboardEventInit): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    bubbles: true,
    cancelable: true,
    ...init,
  })
  act(() => {
    dom.dispatchEvent(event)
  })
  return event
}

describe("RichComposer paste without formatting (Ctrl/⌘+Shift+V)", () => {
  it("routes Ctrl+Shift+V to onPlainPaste and consumes the key when handled", async () => {
    const onPlainPaste = vi.fn(() => true)
    const { ref } = await mount({ onPlainPaste })
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    const event = pressKey(dom, { key: "V", ctrlKey: true, shiftKey: true })
    expect(onPlainPaste).toHaveBeenCalledTimes(1)
    // Consumed → the browser's native rich paste is suppressed.
    expect(event.defaultPrevented).toBe(true)
  })

  it("routes ⌘+Shift+V (metaKey) to onPlainPaste as well", async () => {
    const onPlainPaste = vi.fn(() => true)
    const { ref } = await mount({ onPlainPaste })
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    const event = pressKey(dom, { key: "V", metaKey: true, shiftKey: true })
    expect(onPlainPaste).toHaveBeenCalledTimes(1)
    expect(event.defaultPrevented).toBe(true)
  })

  it("does not consume the key when onPlainPaste declines (returns false)", async () => {
    const onPlainPaste = vi.fn(() => false)
    const { ref } = await mount({ onPlainPaste })
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    const event = pressKey(dom, { key: "V", ctrlKey: true, shiftKey: true })
    expect(onPlainPaste).toHaveBeenCalledTimes(1)
    // Declined → the browser's native "paste and match style" proceeds.
    expect(event.defaultPrevented).toBe(false)
  })

  it("ignores a plain Ctrl+V so the native rich paste stays in effect", async () => {
    const onPlainPaste = vi.fn(() => true)
    const { ref } = await mount({ onPlainPaste })
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    const event = pressKey(dom, { key: "v", ctrlKey: true })
    expect(onPlainPaste).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(false)
  })
})

/** Dispatch a `paste` carrying the given clipboard flavors at the editor. */
function dispatchPaste(
  dom: HTMLElement,
  flavors: { html?: string; text?: string }
): void {
  const html = flavors.html ?? ""
  const text = flavors.text ?? ""
  const clipboardData = {
    getData: (type: string) =>
      type === "text/html" ? html : type === "text/plain" ? text : "",
    setData: () => {},
    clearData: () => {},
    types: [html && "text/html", text && "text/plain"].filter(Boolean),
    files: [] as unknown as FileList,
    items: [] as unknown,
  }
  const event = new Event("paste", { bubbles: true, cancelable: true })
  Object.defineProperty(event, "clipboardData", { value: clipboardData })
  act(() => {
    dom.dispatchEvent(event)
  })
}

describe("RichComposer text paste (plain-text schema)", () => {
  it("pastes a URL as text/plain, not the browser's title-bearing <a> fragment", async () => {
    const { ref } = await mount()
    act(() => ref.current?.focus())
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    // Exactly what a browser writes when a URL is copied from the address bar:
    // the anchor text is the page <title>, so the default HTML parse (no Link
    // mark) would keep the title. We must insert the URL instead.
    dispatchPaste(dom, {
      html: '<a href="https://github.com/">GitHub · Change is constant. GitHub keeps you ahead. · GitHub</a>',
      text: "https://github.com/",
    })
    expect(ref.current?.getText()).toBe("https://github.com/")
  })

  it("preserves structure for content copied from within the editor (data-pm-slice), without blank lines", async () => {
    const { ref } = await mount()
    act(() => ref.current?.focus())
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    // A native ProseMirror copy of two paragraphs: HTML tagged with data-pm-slice,
    // and text/plain "one\n\ntwo" (block separator is "\n\n"). We must defer to
    // the native HTML paste — forcing text/plain would serialize back to
    // "one\n\ntwo", introducing a blank line the composer never had.
    dispatchPaste(dom, {
      html: '<p data-pm-slice="0 0 []">one</p><p>two</p>',
      text: "one\n\ntwo",
    })
    expect(ref.current?.getText()).toBe("one\ntwo")
  })

  it("reconstructs a reference badge pasted from within the composer", async () => {
    const { ref } = await mount()
    act(() => ref.current?.focus())
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    // A real composer copy carries both the slice wrapper and the badge span;
    // defer to ProseMirror so the badge round-trips instead of collapsing to its
    // plain-text token.
    dispatchPaste(dom, {
      html: '<p data-pm-slice="0 0 []"><span data-reference data-ref-type="file" data-ref-id="a.ts" data-label="a.ts" data-uri="file:///a.ts">a.ts</span></p>',
      text: "a.ts",
    })
    expect(JSON.stringify(ref.current?.getJSON())).toContain(
      '"type":"reference"'
    )
  })

  it("does not insert text when the host consumes the paste as files", async () => {
    const onPasteFiles = vi.fn(() => true)
    const { ref } = await mount({ onPasteFiles })
    act(() => ref.current?.focus())
    const dom = ref.current?.getEditor()?.view.dom as HTMLElement
    dispatchPaste(dom, { html: "<a href='x'>x</a>", text: "some text" })
    expect(onPasteFiles).toHaveBeenCalledTimes(1)
    expect(ref.current?.getText()).toBe("")
  })
})
