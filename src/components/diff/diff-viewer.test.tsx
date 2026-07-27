import { render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

// Shared recorder for the fake Monaco pair below. Declared through
// `vi.hoisted` so the module factory (which runs before this file's body) can
// reach it.
const harness = vi.hoisted(() => {
  const events: string[] = []
  // The model pair the fake widget currently holds — the state Monaco's
  // DiffEditorWidget consults through its `onWillDispose` listeners.
  const state: { attached: Record<string, object> | null } = { attached: null }
  return { events, state }
})

// Stand-in for `@monaco-editor/react`'s DiffEditor that reproduces the two
// behaviours the component's teardown exists for: the library disposes both
// text models BEFORE the widget, and Monaco's DiffEditorWidget reports a model
// disposed while still attached as a bug ("TextModel got disposed before
// DiffEditorWidget model got reset", rethrown by its default error handler).
vi.mock("@monaco-editor/react", async () => {
  const { createElement, useEffect } = await import("react")

  const makeDiffEditor = () => {
    let model: Record<string, { dispose: () => void }> | null = null
    const detach = () => {
      model = null
      harness.state.attached = null
    }
    // Monaco's widget listens for `onWillDispose` on the pair it holds: it
    // reports the bug and then resets itself, which is why a real teardown
    // surfaces one error rather than two.
    const makeModel = (side: "original" | "modified") => {
      const m = {
        dispose: () => {
          if (harness.state.attached?.[side] === m) {
            harness.events.push(`bug:${side}`)
            detach()
          }
          harness.events.push(`${side}:dispose`)
        },
      }
      return m
    }
    model = { original: makeModel("original"), modified: makeModel("modified") }
    harness.state.attached = model
    const innerEditor = {
      revealLineInCenter: () => {},
      setPosition: () => {},
    }
    return {
      getModel: () => model,
      setModel: (next: typeof model) => {
        model = next
        harness.state.attached = next
      },
      getModifiedEditor: () => innerEditor,
      getLineChanges: () => [],
      onDidUpdateDiff: () => ({ dispose: () => {} }),
      dispose: () => harness.events.push("editor:dispose"),
    }
  }

  const DiffEditor = ({
    onMount,
  }: {
    onMount?: (editor: ReturnType<typeof makeDiffEditor>) => void
  }) => {
    useEffect(() => {
      const editor = makeDiffEditor()
      onMount?.(editor)
      return () => {
        // @monaco-editor/react@4.7.0's unmount path, verbatim in spirit:
        // models first, widget last.
        const current = editor.getModel()
        current?.original?.dispose()
        current?.modified?.dispose()
        editor.dispose()
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [])
    return createElement("div", { "data-testid": "mock-diff-editor" })
  }

  return {
    DiffEditor,
    useMonaco: () => null,
    loader: { config: () => {} },
  }
})

vi.mock("@/lib/monaco-themes", () => ({
  defineMonacoThemes: () => {},
  MONACO_UNICODE_HIGHLIGHT_OPTIONS: {},
  useMonacoWorkspaceTheme: () => "vs-dark",
}))

vi.mock("@/hooks/use-appearance", () => ({
  useZoomLevel: () => ({ zoomLevel: 100 }),
  useEditorFont: () => ({
    editorFontStack: "monospace",
    editorFontSize: 12,
    editorLigatures: false,
    editorWordWrap: false,
  }),
}))

const { DiffViewer } = await import("./diff-viewer")
const { DiffEditor: MockDiffEditor } =
  (await import("@monaco-editor/react")) as unknown as {
    DiffEditor: () => React.ReactElement
  }

describe("DiffViewer teardown", () => {
  beforeEach(() => {
    harness.events.length = 0
    harness.state.attached = null
  })

  it("disposes the diff models only after detaching them from the widget", async () => {
    const { unmount } = render(<DiffViewer original="a" modified="b" />)
    expect(await screen.findByTestId("mock-diff-editor")).toBeInTheDocument()

    unmount()

    // No "TextModel got disposed before DiffEditorWidget model got reset":
    // the widget no longer holds either model by the time it is disposed.
    expect(harness.events.filter((e) => e.startsWith("bug:"))).toEqual([])
    // Both models still get disposed exactly once — detaching must not leak
    // them (they are anonymous, so nothing else could ever reclaim them).
    expect(harness.events).toEqual([
      "original:dispose",
      "modified:dispose",
      "editor:dispose",
    ])
  })

  it("the fake reproduces the library bug when the component does not intervene", async () => {
    // Guards the test above from passing vacuously: rendered bare, the same
    // fake records exactly the disposal-while-attached that Monaco flags.
    const { unmount } = render(<MockDiffEditor />)
    expect(await screen.findByTestId("mock-diff-editor")).toBeInTheDocument()

    unmount()

    expect(harness.events).toEqual([
      "bug:original",
      "original:dispose",
      "modified:dispose",
      "editor:dispose",
    ])
  })
})
