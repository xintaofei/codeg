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

describe("MessageInput native steering (insert into current turn)", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
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
    await user.click(
      await screen.findByRole("menuitem", { name: MI.steerIntoTurn })
    )
    await waitFor(() => expect(onSteer).toHaveBeenCalledWith("go left"))
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
})
