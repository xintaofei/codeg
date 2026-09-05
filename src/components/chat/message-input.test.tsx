import {
  render,
  screen,
  waitFor,
  within,
  cleanup,
  fireEvent,
  act,
} from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { toast } from "sonner"
import type { ComponentProps } from "react"
import type { Editor } from "@tiptap/core"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { RichComposerHandle } from "./composer/rich-composer"
import { serializeDocToText } from "./composer/to-prompt-blocks"
import {
  emitAttachFileToSession,
  emitAttachSessionToSession,
} from "@/lib/session-attachment-events"
import type { DbConversationSummary } from "@/lib/types"

// MessageInput holds its RichComposer handle internally and does not forward a
// ref, so capture that handle through a partial mock that still renders the real
// composer. The "insertion position" tests below drive the very Tiptap editor
// the attach-to-chat event writes into — setting its content + caret — then
// assert where the badge lands.
const composerHandle = vi.hoisted(() => ({
  current: null as RichComposerHandle | null,
}))
vi.mock("./composer/rich-composer", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("./composer/rich-composer")>()
  const React = await import("react")
  const Captured = React.forwardRef<
    RichComposerHandle,
    ComponentProps<typeof actual.RichComposer>
  >((props, ref) => {
    const assign = (handle: RichComposerHandle | null) => {
      composerHandle.current = handle
      if (typeof ref === "function") ref(handle)
      else if (ref) ref.current = handle
    }
    return React.createElement(actual.RichComposer, { ...props, ref: assign })
  })
  Captured.displayName = "CapturedRichComposer"
  return { ...actual, RichComposer: Captured }
})

// Mock the data hooks / platform so MessageInput mounts without hitting the
// backend. The reference-search provider and slash sources are all empty: this
// is a wiring smoke test (does the RichComposer-based input mount and reflect
// empty/send state), not a data test.
vi.mock("@/hooks/use-shortcut-settings", () => ({
  useShortcutSettings: () => ({
    shortcuts: { send_message: "enter", newline_in_message: "shift+enter" },
  }),
}))
// A vi.fn (not a bare arrow) so the `$` autocomplete tests can land the disk
// scan mid-test; defaults to "no skills" for every other test in this file.
const agentSkills = vi.hoisted(() => vi.fn(() => [] as unknown[]))
vi.mock("@/hooks/use-agent-skills", () => ({ useAgentSkills: agentSkills }))
vi.mock("@/hooks/use-built-in-experts", () => ({ useBuiltInExperts: () => [] }))
vi.mock("@/hooks/use-built-in-science", () => ({ useBuiltInScience: () => [] }))
vi.mock("@/hooks/use-enabled-skill-ids", () => ({
  useEnabledSkillIds: () => ({
    enabledIds: new Set(),
    ready: false,
    supported: true,
  }),
}))
vi.mock("@/components/chat/composer/use-reference-search", () => ({
  useReferenceSearch: () => async () => [],
}))
vi.mock("@/components/chat/conversation-context-bar", () => ({
  ConversationContextBar: ({
    extraContent,
  }: {
    extraContent?: React.ReactNode
  }) => <div data-testid="ctx-bar">{extraContent}</div>,
  // The composer imports these to render the below-input folder/branch row.
  // Keep it hidden here (visibility → false) so these tests exercise the bare
  // composer without pulling in the picker's tab-store/git dependencies.
  ConversationFolderBranchPicker: () => null,
  useConversationFolderBranchPickerVisible: () => false,
}))
vi.mock("@/lib/platform", () => ({
  isDesktop: () => false,
  openFileDialog: vi.fn(),
}))
vi.mock("@/lib/transport", () => ({
  getActiveRemoteConnectionId: () => null,
}))
// Real classifier only recognizes actual backend NoActiveTurn payloads; the
// steering tests flip this per-case to drive the enqueue fallback.
vi.mock("@/lib/turn-busy", () => ({
  isNoActiveTurnRejection: vi.fn(() => false),
}))
// Nothing here mounts a Toaster, so toasts would vanish silently — record them
// instead. The steering tests assert the uploading gate's honest signal.
vi.mock("sonner", () => ({
  toast: { error: vi.fn(), info: vi.fn(), success: vi.fn(), dismiss: vi.fn() },
}))
// Wrap-mock (rich-composer pattern above): render the REAL attachments hook,
// but let a test stage image attachments — the drop/paste pipelines that
// normally populate them need real files and upload endpoints.
type ComposerAttachmentsApi = ReturnType<
  typeof import("./composer/use-composer-attachments").useComposerAttachments
>
const attachmentsOverride = vi.hoisted(() => ({
  current: null as Partial<ComposerAttachmentsApi> | null,
}))
vi.mock("./composer/use-composer-attachments", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("./composer/use-composer-attachments")>()
  return {
    ...actual,
    useComposerAttachments: (
      ...args: Parameters<typeof actual.useComposerAttachments>
    ) => {
      const real = actual.useComposerAttachments(...args)
      const override = attachmentsOverride.current
      return override ? { ...real, ...override } : real
    },
  }
})
// virtua renders 0 rows under jsdom — render children directly so the large
// (searchable + virtualized) model list is exercisable here too.
vi.mock("virtua", async () => {
  const { forwardRef, useImperativeHandle } = await import("react")
  return {
    Virtualizer: forwardRef(function VirtualizerMock(
      props: { children?: React.ReactNode },
      ref: React.Ref<{ scrollToIndex: () => void }>
    ) {
      useImperativeHandle(ref, () => ({ scrollToIndex: () => {} }))
      return <>{props.children}</>
    }),
  }
})

// ModelOptionList mounts virtua only after the OverlayScrollbars viewport is
// surfaced via `onViewportRef`; jsdom never initializes OS, so drive it here.
vi.mock("@/components/ui/scroll-area", async () => {
  const { useEffect } = await import("react")
  return {
    ScrollArea: ({
      children,
      onViewportRef,
    }: {
      children?: React.ReactNode
      onViewportRef?: (el: HTMLElement | null) => void
    }) => {
      useEffect(() => {
        onViewportRef?.(document.createElement("div"))
      }, [onViewportRef])
      return <>{children}</>
    },
  }
})

// ModelOptionList sizes its scroll window in rem, so it reads the live zoom
// level — which throws outside an AppearanceProvider, and this suite renders the
// composer bare. Pin it at 100% (1rem = 16px). Spread the real module so the
// other appearance hooks keep their real (provider-requiring) behaviour instead
// of silently resolving to `undefined` if something here starts using one.
vi.mock("@/hooks/use-appearance", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/use-appearance")>()),
  useZoomLevel: () => ({ zoomLevel: 100, setZoomLevel: () => {} }),
}))

import enMessages from "@/i18n/messages/en.json"
import type {
  PromptCapabilitiesInfo,
  SessionConfigOptionInfo,
} from "@/lib/types"

import { MessageInput } from "./message-input"

const CAPS: PromptCapabilitiesInfo = {
  image: true,
  audio: false,
  embedded_context: true,
}

function renderInput(
  props: Partial<React.ComponentProps<typeof MessageInput>>
) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <MessageInput onSend={vi.fn()} promptCapabilities={CAPS} {...props} />
    </NextIntlClientProvider>
  )
}

describe("MessageInput (RichComposer integration)", () => {
  afterEach(() => cleanup())

  it("mounts and renders the rich-text composer surface", async () => {
    const { container } = renderInput({})
    await waitFor(
      () => expect(container.querySelector('[role="textbox"]')).not.toBeNull(),
      { timeout: 5000 }
    )
    const textbox = container.querySelector('[role="textbox"]')
    expect(textbox).toHaveAttribute("aria-multiline", "true")
  })

  it("disables Send while the composer is empty and has no attachments", async () => {
    const { container } = renderInput({})
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )
    const sendButton = container.querySelector<HTMLButtonElement>(
      `button[title="${enMessages.Folder.chat.messageInput.send}"]`
    )
    expect(sendButton).not.toBeNull()
    expect(sendButton).toBeDisabled()
  })

  it("claims a mousedown on the input's empty chrome (P8d focus wiring)", async () => {
    const { container } = renderInput({})
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )
    // The bordered card carries the chrome-focus handler; a mousedown on the
    // card itself (not on the editor or a control) is claimed via preventDefault
    // before refocusing the editor. Asserting preventDefault (fireEvent returns
    // false when the event was canceled) avoids relying on jsdom focus.
    const card = container.querySelector('[class~="@container"]') as HTMLElement
    expect(card).not.toBeNull()
    // The same box paints the text I-beam across its blank chrome (see the
    // `.codeg-composer-chrome` rule in globals.css).
    expect(card.className).toContain("codeg-composer-chrome")
    expect(fireEvent.mouseDown(card)).toBe(false)
  })
})

describe("MessageInput attach-to-chat insertion position", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
  })

  async function mountWithEditor() {
    renderInput({ attachmentTabId: "tab-1" })
    await waitFor(
      () => expect(composerHandle.current?.getEditor()).toBeTruthy(),
      { timeout: 5000 }
    )
    const editor = composerHandle.current?.getEditor()
    if (!editor) throw new Error("composer editor not mounted")
    return editor
  }

  // Seed "hello world" and drop the caret right after "hello" (pos 6), so an
  // insertion at the caret lands between the two words while an append would
  // land after "world".
  function seedWithMidCaret(editor: Editor) {
    act(() => {
      editor.commands.setContent("hello world")
      editor.commands.setTextSelection(6)
    })
  }

  function assertBetweenHelloAndWorld(text: string, link: string) {
    const at = text.indexOf(link)
    expect(at).toBeGreaterThanOrEqual(0)
    // Caret insertion: "hello" precedes the badge and "world" follows it.
    // (An end-of-doc append would put "world" before the link, failing the
    // second assertion.)
    expect(text.slice(0, at)).toContain("hello")
    expect(text.slice(at + link.length)).toContain("world")
  }

  it("drops an attached whole-file badge at the caret, not the end", async () => {
    const editor = await mountWithEditor()
    seedWithMidCaret(editor)
    act(() => {
      emitAttachFileToSession({ tabId: "tab-1", path: "/repo/app.ts" })
    })
    const link = "[app.ts](file:///repo/app.ts)"
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toContain(link)
    )
    assertBetweenHelloAndWorld(serializeDocToText(editor.state.doc), link)
  })

  it("drops a ranged selection badge at the caret, not the end", async () => {
    const editor = await mountWithEditor()
    seedWithMidCaret(editor)
    act(() => {
      emitAttachFileToSession({
        tabId: "tab-1",
        path: "/repo/app.ts",
        range: { start: 10, end: 25 },
      })
    })
    const link = "[app.ts:10-25](file:///repo/app.ts#L10-25)"
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toContain(link)
    )
    assertBetweenHelloAndWorld(serializeDocToText(editor.state.doc), link)
  })
})

// `injectContent` is the composer's inbox for content pushed in from outside:
// welcome quick actions (replace) and quoted transcript selections (append).
describe("MessageInput injectContent", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
  })

  function tree(props: Partial<React.ComponentProps<typeof MessageInput>>) {
    return (
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <MessageInput onSend={vi.fn()} promptCapabilities={CAPS} {...props} />
      </NextIntlClientProvider>
    )
  }

  async function mount(
    props: Partial<React.ComponentProps<typeof MessageInput>>
  ) {
    const view = render(tree(props))
    await waitFor(
      () => expect(composerHandle.current?.getEditor()).toBeTruthy(),
      { timeout: 5000 }
    )
    const editor = composerHandle.current?.getEditor()
    if (!editor) throw new Error("composer editor not mounted")
    return { view, editor }
  }

  it("appends a quote after an existing draft, separated by a blank line", async () => {
    const onInjectConsumed = vi.fn()
    const { view, editor } = await mount({ onInjectConsumed })
    act(() => {
      editor.commands.setContent("what does this mean?")
    })

    view.rerender(
      tree({
        onInjectConsumed,
        injectContent: { text: "> quoted bit", mode: "append" },
      })
    )

    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toBe(
        "what does this mean?\n\n> quoted bit\n\n"
      )
    )
    expect(onInjectConsumed).toHaveBeenCalled()
  })

  it("appends a quote into an empty composer with no leading gap", async () => {
    const { view, editor } = await mount({})
    view.rerender(
      tree({ injectContent: { text: "> quoted bit", mode: "append" } })
    )
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toBe("> quoted bit\n\n")
    )
  })

  it("still replaces the whole document in the default mode", async () => {
    const { view, editor } = await mount({})
    act(() => {
      editor.commands.setContent("draft to be replaced")
    })
    view.rerender(tree({ injectContent: { text: "quick action prompt" } }))
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toBe("quick action prompt")
    )
  })
})

// The sidebar's "add to session" drops a session mention — the same reference the
// `@` panel's Sessions group builds — into the active tab's composer.
describe("MessageInput session-mention attach", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
  })

  function summary(id: number, title: string | null): DbConversationSummary {
    return {
      id,
      folder_id: 1,
      title,
      title_locked: false,
      agent_type: "claude_code",
      status: "pending",
      kind: "regular",
      model: null,
      git_branch: null,
      external_id: null,
      message_count: 0,
      child_count: 0,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      pinned_at: null,
    }
  }

  async function mountWithEditor() {
    renderInput({ attachmentTabId: "tab-1" })
    await waitFor(
      () => expect(composerHandle.current?.getEditor()).toBeTruthy(),
      { timeout: 5000 }
    )
    const editor = composerHandle.current?.getEditor()
    if (!editor) throw new Error("composer editor not mounted")
    return editor
  }

  it("drops a session badge at the caret, not the end", async () => {
    const editor = await mountWithEditor()
    act(() => {
      editor.commands.setContent("hello world")
      editor.commands.setTextSelection(6)
    })
    act(() => {
      emitAttachSessionToSession({
        tabId: "tab-1",
        conversation: summary(42, "Fix auth"),
      })
    })
    const link = "[Fix auth](codeg://session/42)"
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toContain(link)
    )
    const text = serializeDocToText(editor.state.doc)
    const at = text.indexOf(link)
    expect(text.slice(0, at)).toContain("hello")
    expect(text.slice(at + link.length)).toContain("world")
  })

  it("falls back to #id for an untitled conversation", async () => {
    const editor = await mountWithEditor()
    act(() => {
      emitAttachSessionToSession({
        tabId: "tab-1",
        conversation: summary(7, "   "),
      })
    })
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toContain(
        "[#7](codeg://session/7)"
      )
    )
  })

  it("ignores an event addressed to another tab", async () => {
    const editor = await mountWithEditor()
    act(() => {
      emitAttachSessionToSession({
        tabId: "other-tab",
        conversation: summary(42, "Fix auth"),
      })
    })
    expect(serializeDocToText(editor.state.doc)).not.toContain(
      "codeg://session/42"
    )
  })

  it("does not stack a second badge for the same session", async () => {
    const editor = await mountWithEditor()
    const conversation = summary(42, "Fix auth")
    act(() => {
      emitAttachSessionToSession({ tabId: "tab-1", conversation })
    })
    const link = "[Fix auth](codeg://session/42)"
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toContain(link)
    )
    act(() => {
      emitAttachSessionToSession({ tabId: "tab-1", conversation })
    })
    const text = serializeDocToText(editor.state.doc)
    expect(text.indexOf(link)).toBe(text.lastIndexOf(link))
  })
})

describe("MessageInput file-tree drag-and-drop", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
  })

  // A minimal DataTransfer carrying a file-tree drag: the private JSON payload
  // plus a text/plain absolute-path fallback (jsdom's DataTransfer can't do
  // setData/getData/types faithfully).
  function treeDrag(payload: {
    rootPath: string
    relPath: string
    absPath: string
    name: string
    kind: "file" | "dir"
  }) {
    const store = new Map<string, string>([
      ["application/x-codeg-tree-entry", JSON.stringify(payload)],
      ["text/plain", payload.absPath],
    ])
    return {
      getData: (f: string) => store.get(String(f).toLowerCase()) ?? "",
      setData: (f: string, v: string) => store.set(String(f).toLowerCase(), v),
      get types() {
        return Array.from(store.keys())
      },
      dropEffect: "none",
      effectAllowed: "all",
      files: [] as File[],
      items: [] as DataTransferItem[],
    }
  }

  async function mountWithHost() {
    const { container } = renderInput({ attachmentTabId: "tab-1" })
    await waitFor(
      () => expect(composerHandle.current?.getEditor()).toBeTruthy(),
      { timeout: 5000 }
    )
    const editor = composerHandle.current?.getEditor()
    if (!editor) throw new Error("composer editor not mounted")
    const host = container.firstElementChild as HTMLElement
    return { editor, host }
  }

  const OVERLAY = enMessages.Folder.chat.messageInput.dropFilesToAttach
  const PAYLOAD = {
    rootPath: "/repo",
    relPath: "src/app.ts",
    absPath: "/repo/src/app.ts",
    name: "app.ts",
    kind: "file" as const,
  }
  const LINK = "[app.ts](file:///repo/src/app.ts)"

  it("inserts a single reference (no literal path) when dropped on the chrome", async () => {
    const { editor, host } = await mountWithHost()
    const dt = treeDrag(PAYLOAD)

    act(() => {
      fireEvent.dragOver(host, { dataTransfer: dt })
    })
    // The drag overlay shows while a valid drag hovers the composer.
    expect(screen.queryByText(OVERLAY)).not.toBeNull()

    act(() => {
      fireEvent.drop(host, { dataTransfer: dt })
    })
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toContain(LINK)
    )
    // Exactly one reference and no stray text/plain absolute-path insertion.
    expect(serializeDocToText(editor.state.doc).trim()).toBe(LINK)
    // The overlay is cleared by the drop.
    expect(screen.queryByText(OVERLAY)).toBeNull()
  })

  it("clears the overlay and inserts once when dropped on the editor surface", async () => {
    const { editor, host } = await mountWithHost()
    const textbox = host.querySelector('[role="textbox"]') as HTMLElement
    const dt = treeDrag(PAYLOAD)

    // Hover raises the overlay (container-level dragover)…
    act(() => {
      fireEvent.dragOver(host, { dataTransfer: dt })
    })
    expect(screen.queryByText(OVERLAY)).not.toBeNull()

    // …then drop directly on the editor. Whether ProseMirror's handleDrop
    // consumes it (stopping propagation) or it bubbles to the container, the
    // result must be one reference and no lingering overlay — the regression
    // being that an editor-consumed drop left the overlay stuck.
    act(() => {
      fireEvent.drop(textbox, { dataTransfer: dt })
    })
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).toContain(LINK)
    )
    expect(serializeDocToText(editor.state.doc).trim()).toBe(LINK)
    expect(screen.queryByText(OVERLAY)).toBeNull()
  })
})

// When the composer is narrow the model/config/mode selectors collapse behind a
// cog button into a single Popover that renders a master–detail panel: the
// settings on the left, the active setting's options (plain buttons) on the
// right. This is the WebKit-safe replacement for the old nested dropdown/submenu
// — a nested Radix dismissable layer drops the selection on WKWebView, so the
// options are plain <button>s in the one popover layer. jsdom has no layout, so
// the container-query-hidden wide row stays hidden and this collapsed path is
// what renders here.
const MODEL_OPTION: SessionConfigOptionInfo = {
  id: "model",
  name: "Model",
  description: "Pick the model",
  category: null,
  kind: {
    type: "select",
    current_value: "default",
    options: [
      { value: "default", name: "Default", description: "Use the default" },
      { value: "opus", name: "Opus", description: "Most capable" },
    ],
    groups: [],
  },
}

const MSGS = enMessages.Folder.chat.messageInput

// Cline 3.0.50's `auto_approve` — the first boolean config option any pinned
// agent ships. Both the wide inline row and the collapsed popover are always in
// the DOM (a container query, which jsdom does not evaluate, picks one), so a
// single render exercises both surfaces.
const AUTO_APPROVE_OPTION: SessionConfigOptionInfo = {
  id: "auto_approve",
  name: "Auto-approve tools",
  description: "Automatically approve all tool calls without asking",
  category: null,
  kind: { type: "boolean", current_value: false },
}

describe("MessageInput boolean config options", () => {
  afterEach(() => cleanup())

  it("renders the inline chip as a toggle and flips it on click", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [AUTO_APPROVE_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    const toggle = screen.getByRole("button", {
      name: `Auto-approve tools: ${MSGS.toggleOff}`,
    })
    expect(toggle).toHaveAttribute("aria-pressed", "false")

    await user.click(toggle)
    expect(onConfigOptionChange).toHaveBeenCalledWith("auto_approve", "true")
  })

  it("offers On/Off rows in the collapsed cog popover", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [AUTO_APPROVE_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    const settingsLabel = MSGS.agentSettings
    await user.click(screen.getByRole("button", { name: settingsLabel }))
    const popover = await screen.findByRole("dialog", { name: settingsLabel })

    // The left rail summarizes the current state…
    expect(
      within(popover).getByRole("button", { name: /Auto-approve tools/ })
    ).toBeInTheDocument()
    // …and the detail pane is a plain two-item choice.
    await user.click(
      within(popover).getByRole("button", { name: MSGS.toggleOn })
    )
    expect(onConfigOptionChange).toHaveBeenCalledWith("auto_approve", "true")
  })
})

describe("MessageInput collapsed selectors popover", () => {
  afterEach(() => cleanup())

  it("selects a config option from the cog Popover and closes it", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [MODEL_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    const settingsLabel = enMessages.Folder.chat.messageInput.agentSettings
    await user.click(screen.getByRole("button", { name: settingsLabel }))

    const popover = await screen.findByRole("dialog", { name: settingsLabel })
    // The left rail shows the setting as a title + current value row.
    expect(
      within(popover).getByRole("button", { name: /Model/ })
    ).toBeInTheDocument()

    // Options are plain buttons (native clicks) — selecting fires the change.
    await user.click(within(popover).getByRole("button", { name: /Opus/ }))
    expect(onConfigOptionChange).toHaveBeenCalledWith("model", "opus")

    // Selecting a value closes the controlled popover.
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: settingsLabel })).toBeNull()
    )
  })

  it("groups model values by their provider prefix in the cog Popover", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const groupedModel: SessionConfigOptionInfo = {
      id: "model",
      name: "Model",
      description: "Pick the model",
      category: null,
      kind: {
        type: "select",
        current_value: "anthropic/claude-opus",
        options: [
          {
            value: "anthropic/claude-opus",
            name: "anthropic/claude-opus",
            description: null,
          },
          { value: "openai/gpt-4o", name: "openai/gpt-4o", description: null },
        ],
        groups: [],
      },
    }
    const { container } = renderInput({
      configOptions: [groupedModel],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    const settingsLabel = enMessages.Folder.chat.messageInput.agentSettings
    await user.click(screen.getByRole("button", { name: settingsLabel }))
    const popover = await screen.findByRole("dialog", { name: settingsLabel })

    // The detail pane carries one header per provider namespace…
    expect(within(popover).getByText("anthropic")).toBeInTheDocument()
    expect(within(popover).getByText("openai")).toBeInTheDocument()

    // …and the option label drops the redundant `openai/` prefix, while the
    // committed value stays the full id. (Pick the non-current model so its
    // label is unique to the detail pane, not echoed in the left-rail summary.)
    await user.click(within(popover).getByRole("button", { name: /gpt-4o/ }))
    expect(onConfigOptionChange).toHaveBeenCalledWith("model", "openai/gpt-4o")
  })

  it("uses a searchable virtualized list for a long model list", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const options = Array.from({ length: 30 }, (_, i) => ({
      value: `openrouter/model-${i}`,
      name: `openrouter/model-${i}`,
      description: null,
    }))
    const bigModel: SessionConfigOptionInfo = {
      id: "model",
      name: "Model",
      description: null,
      category: null,
      kind: {
        type: "select",
        current_value: "openrouter/model-0",
        options,
        groups: [],
      },
    }
    const { container } = renderInput({
      configOptions: [bigModel],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    const settingsLabel = enMessages.Folder.chat.messageInput.agentSettings
    await user.click(screen.getByRole("button", { name: settingsLabel }))
    const popover = await screen.findByRole("dialog", { name: settingsLabel })

    // A long list (> threshold) renders the searchable combobox, not plain rows.
    const search = within(popover).getByRole("combobox")
    await user.type(search, "model-17")
    // Filtering narrows to the one match; the full id is committed on click.
    await user.click(within(popover).getByRole("option", { name: /model-17/ }))
    expect(onConfigOptionChange).toHaveBeenCalledWith(
      "model",
      "openrouter/model-17"
    )
  })

  it("selects a mode from the cog Popover and closes it", async () => {
    const user = userEvent.setup()
    const onModeChange = vi.fn()
    const { container } = renderInput({
      modes: [
        { id: "plan", name: "Plan", description: "Plan first" },
        { id: "act", name: "Act", description: "Act now" },
      ],
      selectedModeId: "plan",
      onModeChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    const settingsLabel = enMessages.Folder.chat.messageInput.agentSettings
    await user.click(screen.getByRole("button", { name: settingsLabel }))

    const popover = await screen.findByRole("dialog", { name: settingsLabel })
    await user.click(within(popover).getByRole("button", { name: /Act/ }))
    expect(onModeChange).toHaveBeenCalledWith("act")

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: settingsLabel })).toBeNull()
    )
  })
})

// The model picker's free-text escape hatch: a brand-new model is often live on
// the wire before the agent's curated list catches up, so every model-picker
// surface offers a trailing "Use custom model ID..." row that opens a small
// entry dialog. The typed id then takes the exact same
// `onConfigOptionChange` path as picking an advertised option.
describe("MessageInput custom model id entry", () => {
  afterEach(() => cleanup())

  const LONG_MODEL_OPTION: SessionConfigOptionInfo = {
    id: "model",
    name: "Model",
    description: null,
    category: null,
    kind: {
      type: "select",
      current_value: "openrouter/model-0",
      options: Array.from({ length: 30 }, (_, i) => ({
        value: `openrouter/model-${i}`,
        name: `openrouter/model-${i}`,
        description: null,
      })),
      groups: [],
    },
  }

  it("routes a typed id from the inline dropdown through the option path", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [MODEL_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    await user.click(screen.getByRole("button", { name: "Model: Default" }))
    await user.click(
      await screen.findByRole("menuitem", { name: MSGS.customModelEntry })
    )

    const dialog = await screen.findByRole("dialog", {
      name: MSGS.customModelTitle,
    })
    await user.type(
      within(dialog).getByLabelText(MSGS.customModelInputLabel),
      "claude-fable-5-1"
    )
    await user.click(
      within(dialog).getByRole("button", { name: MSGS.customModelApply })
    )

    expect(onConfigOptionChange).toHaveBeenCalledWith(
      "model",
      "claude-fable-5-1"
    )
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: MSGS.customModelTitle })
      ).toBeNull()
    )
  })

  it("submits on Enter with the id trimmed, and blocks blank ids", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [MODEL_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    await user.click(screen.getByRole("button", { name: "Model: Default" }))
    await user.click(
      await screen.findByRole("menuitem", { name: MSGS.customModelEntry })
    )
    const dialog = await screen.findByRole("dialog", {
      name: MSGS.customModelTitle,
    })
    const input = within(dialog).getByLabelText(MSGS.customModelInputLabel)
    const apply = within(dialog).getByRole("button", {
      name: MSGS.customModelApply,
    })

    // Whitespace is not an id: the submit stays disabled and Enter is inert.
    expect(apply).toBeDisabled()
    await user.type(input, "   ")
    expect(apply).toBeDisabled()
    await user.keyboard("{Enter}")
    expect(onConfigOptionChange).not.toHaveBeenCalled()

    // A real id submits on Enter, trimmed of the stray whitespace.
    await user.clear(input)
    await user.type(input, "  claude-fable-5-1  ")
    await user.keyboard("{Enter}")
    expect(onConfigOptionChange).toHaveBeenCalledWith(
      "model",
      "claude-fable-5-1"
    )
  })

  it("keeps Cancel side-effect-free and reopens with a blank field", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [MODEL_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    await user.click(screen.getByRole("button", { name: "Model: Default" }))
    await user.click(
      await screen.findByRole("menuitem", { name: MSGS.customModelEntry })
    )
    let dialog = await screen.findByRole("dialog", {
      name: MSGS.customModelTitle,
    })
    await user.type(
      within(dialog).getByLabelText(MSGS.customModelInputLabel),
      "half-typed"
    )
    await user.click(within(dialog).getByRole("button", { name: MSGS.cancel }))
    expect(onConfigOptionChange).not.toHaveBeenCalled()

    // The abandoned draft does not survive into the next open.
    await user.click(screen.getByRole("button", { name: "Model: Default" }))
    await user.click(
      await screen.findByRole("menuitem", { name: MSGS.customModelEntry })
    )
    dialog = await screen.findByRole("dialog", {
      name: MSGS.customModelTitle,
    })
    expect(
      within(dialog).getByLabelText(MSGS.customModelInputLabel)
    ).toHaveValue("")
  })

  it("offers the entry only on the MODEL option, never other selects", async () => {
    const user = userEvent.setup()
    const effortOption: SessionConfigOptionInfo = {
      id: "effort",
      name: "Effort",
      description: null,
      category: "thought_level",
      kind: {
        type: "select",
        current_value: "default",
        options: [
          { value: "default", name: "Default", description: null },
          { value: "high", name: "High", description: null },
        ],
        groups: [],
      },
    }
    const { container } = renderInput({
      configOptions: [effortOption],
      onConfigOptionChange: vi.fn(),
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    await user.click(screen.getByRole("button", { name: "Effort: Default" }))
    // The rows themselves render (it's a real select)…
    expect(
      await screen.findByRole("menuitemradio", { name: /High/ })
    ).toBeInTheDocument()
    // …but the free-text escape hatch is a model-picker affordance only.
    expect(
      screen.queryByRole("menuitem", { name: MSGS.customModelEntry })
    ).toBeNull()
  })

  it("pins the entry under the searchable popover, beyond any filter", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [LONG_MODEL_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    // The wide surface for a long list is the searchable popover. (The shared
    // `openrouter/` prefix is stripped from the trigger label — the provider
    // is implied by the group the model sits in.)
    await user.click(screen.getByRole("button", { name: "Model: model-0" }))
    const search = await screen.findByRole("combobox")
    // A brand-new id matches nothing — exactly when the entry must survive.
    await user.type(search, "claude-fable-5-1")
    expect(screen.getByText(MSGS.noModels)).toBeInTheDocument()
    await user.click(
      screen.getByRole("button", { name: MSGS.customModelEntry })
    )

    const dialog = await screen.findByRole("dialog", {
      name: MSGS.customModelTitle,
    })
    await user.type(
      within(dialog).getByLabelText(MSGS.customModelInputLabel),
      "claude-fable-5-1"
    )
    await user.click(
      within(dialog).getByRole("button", { name: MSGS.customModelApply })
    )
    expect(onConfigOptionChange).toHaveBeenCalledWith(
      "model",
      "claude-fable-5-1"
    )
  })

  it("offers the entry in the collapsed cog panel too", async () => {
    const user = userEvent.setup()
    const onConfigOptionChange = vi.fn()
    const { container } = renderInput({
      configOptions: [MODEL_OPTION],
      onConfigOptionChange,
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    const settingsLabel = MSGS.agentSettings
    await user.click(screen.getByRole("button", { name: settingsLabel }))
    const popover = await screen.findByRole("dialog", { name: settingsLabel })
    await user.click(
      within(popover).getByRole("button", { name: MSGS.customModelEntry })
    )

    // Picking the entry closes the cog popover, like a selection would…
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: settingsLabel })).toBeNull()
    )
    // …and hands over to the shared dialog.
    expect(
      await screen.findByRole("dialog", { name: MSGS.customModelTitle })
    ).toBeInTheDocument()
  })

  it("shows the raw current id when it is not among the options", async () => {
    // An agent can track a model OUTSIDE its advertised options (a custom
    // pick it adopted, a refusal fallback, a resumed allowlist-excluded
    // model). Every trigger falls back to the raw id rather than a blank or
    // a stale advertised label.
    const user = userEvent.setup()
    const rawCurrent: SessionConfigOptionInfo = {
      id: "model",
      name: "Model",
      description: "Pick the model",
      category: null,
      kind: {
        type: "select",
        current_value: "claude-fable-5-1",
        options: [
          { value: "default", name: "Default", description: null },
          { value: "opus", name: "Opus", description: null },
        ],
        groups: [],
      },
    }
    const { container } = renderInput({
      configOptions: [rawCurrent],
      onConfigOptionChange: vi.fn(),
    })
    await waitFor(() =>
      expect(container.querySelector('[role="textbox"]')).not.toBeNull()
    )

    // The wide inline trigger names the raw id…
    expect(
      screen.getByRole("button", { name: "Model: claude-fable-5-1" })
    ).toBeInTheDocument()

    // …and so does the collapsed rail's summary row.
    const settingsLabel = MSGS.agentSettings
    await user.click(screen.getByRole("button", { name: settingsLabel }))
    const popover = await screen.findByRole("dialog", { name: settingsLabel })
    expect(
      within(popover).getByRole("button", { name: /claude-fable-5-1/ })
    ).toBeInTheDocument()
  })
})

// The `/` list is the agent's own `availableCommands`, which only arrive with
// the connection. The editor is typable throughout that wait, so a `/` typed
// mid-connect opens the panel on a loading row instead of doing nothing.
describe("MessageInput slash menu while the agent connects", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
  })

  const LOADING = enMessages.Folder.chat.messageInput.slashLoading
  const COMMANDS = [{ name: "compact", description: "Compact the thread" }]

  async function mountAndType(
    props: Partial<React.ComponentProps<typeof MessageInput>>,
    text = "/"
  ) {
    const view = renderInput(props)
    await waitFor(
      () => expect(composerHandle.current?.getEditor()).toBeTruthy(),
      { timeout: 5000 }
    )
    const editor = composerHandle.current?.getEditor()
    if (!editor) throw new Error("composer editor not mounted")
    act(() => {
      editor.commands.insertContent(text)
    })
    return { editor, view }
  }

  function rerenderInput(
    view: ReturnType<typeof renderInput>,
    props: Partial<React.ComponentProps<typeof MessageInput>>
  ) {
    view.rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <MessageInput onSend={vi.fn()} promptCapabilities={CAPS} {...props} />
      </NextIntlClientProvider>
    )
  }

  it("opens the panel on a loading row while commands are still on their way", async () => {
    await mountAndType({ commandsLoading: true, availableCommands: [] })
    const menu = await screen.findByTestId("slash-menu")
    expect(within(menu).getByText(LOADING)).toBeInTheDocument()
  })

  it("replaces the loading row with the commands once they land", async () => {
    const { view } = await mountAndType({
      commandsLoading: true,
      availableCommands: [],
    })
    await screen.findByTestId("slash-menu")

    // Connection completes: the same open panel fills in, no retyping.
    rerenderInput(view, { commandsLoading: false, availableCommands: COMMANDS })
    const menu = await screen.findByTestId("slash-menu")
    await waitFor(() =>
      expect(within(menu).getByText("/compact")).toBeInTheDocument()
    )
    expect(within(menu).queryByText(LOADING)).toBeNull()
  })

  // The wait has two legs — the transport connecting, then the session
  // initializing behind an already-`connected` status. The panel must not blink
  // out at the seam between them (ChatInput spans both; see its own test).
  it("stays open across the whole wait, then fills in", async () => {
    const { view } = await mountAndType({
      commandsLoading: true,
      availableCommands: [],
    })
    expect(
      within(await screen.findByTestId("slash-menu")).getByText(LOADING)
    ).toBeInTheDocument()

    // Leg two: still loading, still nothing to show — panel stays put.
    rerenderInput(view, { commandsLoading: true, availableCommands: [] })
    expect(
      within(await screen.findByTestId("slash-menu")).getByText(LOADING)
    ).toBeInTheDocument()

    // Commands land.
    rerenderInput(view, { commandsLoading: false, availableCommands: COMMANDS })
    await waitFor(() =>
      expect(
        within(screen.getByTestId("slash-menu")).getByText("/compact")
      ).toBeInTheDocument()
    )
  })

  it("closes the loading row when the session comes up with no commands", async () => {
    const { view } = await mountAndType({
      commandsLoading: true,
      availableCommands: [],
    })
    await screen.findByTestId("slash-menu")

    // A commandless agent: the wait ends, so the panel must not spin forever.
    rerenderInput(view, { commandsLoading: false, availableCommands: [] })
    await waitFor(() => expect(screen.queryByTestId("slash-menu")).toBeNull())
  })

  // `useAgentSkills` reports an in-flight disk scan as an empty list, so `$`
  // must not be gated on it: a `$` typed before the scan lands has to fill in on
  // its own rather than wait for another keystroke.
  it("fills in a `$` panel when the Codex skill scan lands late", async () => {
    const { view } = await mountAndType(
      { agentType: "codex", availableCommands: COMMANDS },
      "$"
    )
    // Scan still running (an in-flight scan reads as an empty list): the panel
    // has nothing to show yet, but the trigger must stay armed.
    expect(screen.queryByTestId("slash-menu")).toBeNull()

    // Scan lands. No new keystroke — only a re-render.
    agentSkills.mockReturnValue([
      {
        id: "brainstorm",
        name: "Brainstorm",
        description: "Ideas",
        scope: "global",
      },
    ])
    try {
      rerenderInput(view, { agentType: "codex", availableCommands: COMMANDS })
      const menu = await screen.findByTestId("slash-menu")
      expect(within(menu).getByText("Brainstorm")).toBeInTheDocument()
    } finally {
      agentSkills.mockReturnValue([])
    }
  })

  it("stays closed when nothing is loading and the agent has no commands", async () => {
    await mountAndType({ commandsLoading: false, availableCommands: [] })
    // Give the trigger detection a turn to run before asserting the absence.
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(screen.queryByTestId("slash-menu")).toBeNull()
  })

  it("does not send on Enter while the loading panel owns the keys", async () => {
    const onSend = vi.fn()
    const { editor } = await mountAndType({
      onSend,
      commandsLoading: true,
      availableCommands: [],
    })
    await screen.findByTestId("slash-menu")
    act(() => {
      ;(editor.view.dom as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
        })
      )
    })
    expect(onSend).not.toHaveBeenCalled()
    // Escape dismisses it, restoring normal editing.
    act(() => {
      ;(editor.view.dom as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Escape",
          bubbles: true,
          cancelable: true,
        })
      )
    })
    await waitFor(() => expect(screen.queryByTestId("slash-menu")).toBeNull())
  })
})

describe("MessageInput mid-turn send (live-feedback channel)", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
    attachmentsOverride.current = null
    vi.clearAllMocks()
  })

  const MI = enMessages.Folder.chat.messageInput

  async function mountPrompting(
    props: Partial<React.ComponentProps<typeof MessageInput>> = {}
  ) {
    renderInput({
      isPrompting: true,
      disabled: true,
      onCancel: vi.fn(),
      onEnqueue: vi.fn(),
      // The prop defaults to the weaker `pull` promise, so the native cases
      // below have to say so explicitly; the pull case overrides it back.
      steerChannel: "native",
      ...props,
    })
    await waitFor(
      () => expect(composerHandle.current?.getEditor()).toBeTruthy(),
      { timeout: 5000 }
    )
    const editor = composerHandle.current?.getEditor()
    if (!editor) throw new Error("composer editor not mounted")
    return editor
  }

  function typeDraft(editor: Editor, text: string) {
    // insertContent dispatches a real transaction, so the composer's
    // empty-tracking flips (plain setContent doesn't emit an update).
    act(() => {
      editor.commands.insertContent(text)
    })
  }

  it("keeps the historical Stop-only form when onSteer is absent", async () => {
    const editor = await mountPrompting()
    typeDraft(editor, "draft text")
    // Stop is there; none of the split-button chrome is.
    expect(screen.getByTitle(MI.cancel)).toBeInTheDocument()
    expect(screen.queryByTitle(MI.queueMessage)).toBeNull()
    expect(screen.queryByLabelText(MI.steerIntoTurn)).toBeNull()
  })

  it("shows the queue/steer split next to Stop once there is content", async () => {
    const editor = await mountPrompting({ onSteer: vi.fn() })
    // Empty draft: nothing to queue or steer — still Stop-only.
    expect(screen.queryByTitle(MI.queueMessage)).toBeNull()

    typeDraft(editor, "go left")
    await waitFor(() =>
      expect(screen.getByTitle(MI.queueMessage)).toBeInTheDocument()
    )
    expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    expect(screen.getByTitle(MI.cancel)).toBeInTheDocument()
  })

  it("steers the draft text and clears the composer on success", async () => {
    const user = userEvent.setup()
    let resolveSteer: () => void = () => {}
    const onSteer = vi.fn(
      () =>
        new Promise<void>((r) => {
          resolveSteer = r
        })
    )
    const editor = await mountPrompting({ onSteer })
    typeDraft(editor, "go left")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerIntoTurn))
    const item = await screen.findByRole("menuitem", { name: MI.steerIntoTurn })
    // The glyph promises what the label does — the bolt is the instant insert.
    expect(item.querySelector(".lucide-zap")).not.toBeNull()
    await user.click(item)
    // A plain-text draft steers as text alone — no blocks payload.
    await waitFor(() =>
      expect(onSteer).toHaveBeenCalledWith("go left", undefined)
    )
    // Unsettled: the draft must survive until the backend confirms.
    expect(serializeDocToText(editor.state.doc)).toContain("go left")

    await act(async () => {
      resolveSteer()
    })
    // Confirmed: the composer clears (the split collapses back to Stop-only).
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).not.toContain("go left")
    )
    await waitFor(() => expect(screen.queryByTitle(MI.queueMessage)).toBeNull())
  })

  it("falls back to the queue when the turn ends in the race window", async () => {
    const user = userEvent.setup()
    const { isNoActiveTurnRejection } = await import("@/lib/turn-busy")
    vi.mocked(isNoActiveTurnRejection).mockReturnValue(true)
    const onSteer = vi.fn().mockRejectedValue(new Error("no active turn"))
    const onEnqueue = vi.fn()
    const editor = await mountPrompting({ onSteer, onEnqueue })
    typeDraft(editor, "late note")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerIntoTurn))
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerIntoTurn })
    )

    await waitFor(() => expect(onEnqueue).toHaveBeenCalled())
    const [draft] = onEnqueue.mock.calls[0]
    expect(draft.blocks).toEqual([{ type: "text", text: "late note" }])
    // Draft consumed by the queue, not lost and not duplicated.
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).not.toContain("late note")
    )
  })

  it("keeps the draft on a non-turn-end failure", async () => {
    const user = userEvent.setup()
    const { isNoActiveTurnRejection } = await import("@/lib/turn-busy")
    vi.mocked(isNoActiveTurnRejection).mockReturnValue(false)
    const onSteer = vi.fn().mockRejectedValue(new Error("boom"))
    const onEnqueue = vi.fn()
    const editor = await mountPrompting({ onSteer, onEnqueue })
    typeDraft(editor, "keep me")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerIntoTurn))
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerIntoTurn })
    )

    await waitFor(() => expect(onSteer).toHaveBeenCalled())
    // Real failure: nothing queued, draft intact for retry.
    expect(onEnqueue).not.toHaveBeenCalled()
    expect(serializeDocToText(editor.state.doc)).toContain("keep me")
  })

  it("labels the mid-turn action honestly on the pull channel", async () => {
    // A pull-tool session gets the same split, but its action must never
    // promise an instant insert: the note is recorded as waiting and read on
    // the agent's next check, so the copy says exactly that.
    const user = userEvent.setup()
    const onSteer = vi.fn().mockResolvedValue(undefined)
    const editor = await mountPrompting({ onSteer, steerChannel: "pull" })
    typeDraft(editor, "check the tests")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerAsNote)).toBeInTheDocument()
    )
    expect(screen.queryByLabelText(MI.steerIntoTurn)).toBeNull()

    // The action itself rides the same steer path — only the copy differs.
    await user.click(screen.getByLabelText(MI.steerAsNote))
    const pullItem = await screen.findByRole("menuitem", {
      name: MI.steerAsNote,
    })
    // ...and so does the glyph: the notes strip's waiting clock, never the
    // instant-insert bolt.
    expect(pullItem.querySelector(".lucide-clock")).not.toBeNull()
    expect(pullItem.querySelector(".lucide-zap")).toBeNull()
    await user.click(pullItem)
    await waitFor(() =>
      expect(onSteer).toHaveBeenCalledWith("check the tests", undefined)
    )
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).not.toContain(
        "check the tests"
      )
    )
  })

  const stagedImage = {
    id: "att-1",
    type: "image" as const,
    data: "aGk=",
    uri: null,
    name: "mock.png",
    mimeType: "image/png",
  }
  const stagedImageBlock = {
    type: "image" as const,
    data: "aGk=",
    mime_type: "image/png",
  }

  it("steers a draft with an image attachment, blocks included", async () => {
    // The whole point of block steering: the entry stays enabled with an
    // image staged, and the handler ships the SAME block list a normal send
    // would build — text prose plus the attachment — with the prose as the
    // recorded note.
    const user = userEvent.setup()
    attachmentsOverride.current = {
      attachments: [stagedImage],
      imagePromptBlocks: () => [stagedImageBlock],
    }
    const onSteer = vi.fn().mockResolvedValue(undefined)
    const editor = await mountPrompting({ onSteer })
    typeDraft(editor, "match this mock")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerIntoTurn))
    const item = await screen.findByRole("menuitem", {
      name: MI.steerIntoTurn,
    })
    expect(item).not.toHaveAttribute("aria-disabled", "true")
    await user.click(item)
    await waitFor(() =>
      expect(onSteer).toHaveBeenCalledWith("match this mock", [
        { type: "text", text: "match this mock" },
        stagedImageBlock,
      ])
    )
    // Confirmed: the draft clears like any successful steer.
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).not.toContain(
        "match this mock"
      )
    )
  })

  it("defaults to the pull copy when no channel is declared", async () => {
    // The weaker promise is the default: a call site that wires `onSteer` but
    // forgets `steerChannel` must understate delivery, never claim an insert.
    const editor = await mountPrompting({
      onSteer: vi.fn(),
      steerChannel: undefined,
    })
    typeDraft(editor, "no channel declared")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerAsNote)).toBeInTheDocument()
    )
    expect(screen.queryByLabelText(MI.steerIntoTurn)).toBeNull()
    // The menu item, not just the trigger: they read from `steerChannel`
    // independently, so a default that leaked into only one of them would
    // still promise an insert somewhere.
    await userEvent.setup().click(screen.getByLabelText(MI.steerAsNote))
    expect(
      await screen.findByRole("menuitem", { name: MI.steerAsNote })
    ).toBeInTheDocument()
    expect(
      screen.queryByRole("menuitem", { name: MI.steerIntoTurn })
    ).toBeNull()
  })

  it("names the note, not an insert, when a pull send fails", async () => {
    // The failure toast is the last place the pull channel could overpromise:
    // "couldn't insert into the current turn" would describe a delivery this
    // session never attempted.
    const user = userEvent.setup()
    const { isNoActiveTurnRejection } = await import("@/lib/turn-busy")
    vi.mocked(isNoActiveTurnRejection).mockReturnValue(false)
    const onSteer = vi.fn().mockRejectedValue(new Error("boom"))
    const editor = await mountPrompting({ onSteer, steerChannel: "pull" })
    typeDraft(editor, "keep me")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerAsNote)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerAsNote))
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerAsNote })
    )

    await waitFor(() =>
      expect(vi.mocked(toast.error)).toHaveBeenCalledWith(
        MI.steerNoteFailed,
        expect.anything()
      )
    )
    // Exactly one toast: raising the insert copy alongside the note copy is
    // the same overpromise, just louder.
    expect(vi.mocked(toast.error)).toHaveBeenCalledTimes(1)
    // Same draft policy as native: a real failure keeps the text for retry.
    expect(serializeDocToText(editor.state.doc)).toContain("keep me")
  })

  it("steers an image-only draft with the attachment summary as the note", async () => {
    // No prose to record, so the note falls back to the draft's display text
    // (what the queue chip would have shown) while the wire still carries the
    // real image block.
    const user = userEvent.setup()
    attachmentsOverride.current = {
      attachments: [stagedImage],
      imagePromptBlocks: () => [stagedImageBlock],
    }
    const onSteer = vi.fn().mockResolvedValue(undefined)
    await mountPrompting({ onSteer })
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerIntoTurn))
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerIntoTurn })
    )
    await waitFor(() =>
      expect(onSteer).toHaveBeenCalledWith("Attached 1 attachment", [
        stagedImageBlock,
      ])
    )
  })

  it("blocks steering while an image upload is still settling", async () => {
    // Same guard as a plain send: an unsettled upload has no server-side uri
    // to hydrate from. The gate must be its own honest toast — the enqueue
    // fallback would otherwise ship a bytes-less marker block.
    const user = userEvent.setup()
    const { toast } = await import("sonner")
    attachmentsOverride.current = {
      attachments: [{ ...stagedImage, uploading: true }],
      hasUploadingImage: true,
      imagePromptBlocks: () => [stagedImageBlock],
    }
    const onSteer = vi.fn()
    const editor = await mountPrompting({ onSteer })
    typeDraft(editor, "wait for it")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerIntoTurn))
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerIntoTurn })
    )
    expect(onSteer).not.toHaveBeenCalled()
    expect(vi.mocked(toast.error)).toHaveBeenCalledWith(
      enMessages.Folder.chat.messageInput.attachUploadInProgress
    )
    // Draft and attachment stay put for the retry.
    expect(serializeDocToText(editor.state.doc)).toContain("wait for it")
  })

  it("queues the attachment too when the blocks steer is rejected", async () => {
    // The load-bearing half of "attachments are never silently dropped": the
    // backend rejects a blocks-bearing note on the pull path (and on the
    // turn-end race) with NoActiveTurn, and this fallback has to re-route the
    // WHOLE draft — image included — not just the prose.
    const user = userEvent.setup()
    const { isNoActiveTurnRejection } = await import("@/lib/turn-busy")
    vi.mocked(isNoActiveTurnRejection).mockReturnValue(true)
    attachmentsOverride.current = {
      attachments: [stagedImage],
      imagePromptBlocks: () => [stagedImageBlock],
    }
    const onSteer = vi.fn().mockRejectedValue(new Error("no active turn"))
    const onEnqueue = vi.fn()
    const editor = await mountPrompting({ onSteer, onEnqueue })
    typeDraft(editor, "late note")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerIntoTurn)).toBeInTheDocument()
    )

    await user.click(screen.getByLabelText(MI.steerIntoTurn))
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerIntoTurn })
    )

    await waitFor(() => expect(onEnqueue).toHaveBeenCalled())
    const [draft] = onEnqueue.mock.calls[0]
    expect(draft.blocks).toEqual([
      { type: "text", text: "late note" },
      stagedImageBlock,
    ])
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).not.toContain("late note")
    )
  })

  it("labels the mid-turn action honestly on the pull channel", async () => {
    // A pull-tool session gets the same split, but its action must never
    // promise an instant insert: the note is recorded as waiting and read on
    // the agent's next check, so the copy says exactly that.
    const user = userEvent.setup()
    const onSteer = vi.fn().mockResolvedValue(undefined)
    const editor = await mountPrompting({ onSteer, steerChannel: "pull" })
    typeDraft(editor, "check the tests")
    await waitFor(() =>
      expect(screen.getByLabelText(MI.steerAsNote)).toBeInTheDocument()
    )
    expect(screen.queryByLabelText(MI.steerIntoTurn)).toBeNull()

    // The action itself rides the same steer path — only the copy differs.
    await user.click(screen.getByLabelText(MI.steerAsNote))
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerAsNote })
    )
    await waitFor(() =>
      expect(onSteer).toHaveBeenCalledWith("check the tests", undefined)
    )
    await waitFor(() =>
      expect(serializeDocToText(editor.state.doc)).not.toContain(
        "check the tests"
      )
    )
  })
})
