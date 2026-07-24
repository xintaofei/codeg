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
import { emitAttachFileToSession } from "@/lib/session-attachment-events"

// MessageInput holds its RichComposer handle internally and does not forward a
// ref, so capture that handle through a partial mock that still renders the real
// composer. The "insertion position" tests below drive the very Tiptap editor
// the attach-to-chat event writes into — setting its content + caret — then
// assert where the badge lands.
const composerHandle = vi.hoisted(() => ({
  current: null as RichComposerHandle | null,
}))
const uploadAttachmentMock = vi.hoisted(() => vi.fn())
const readFileBase64Mock = vi.hoisted(() => vi.fn())
const readLocalPathForAttachmentMock = vi.hoisted(() => vi.fn())
const readLocalImagePathForAttachmentMock = vi.hoisted(() => vi.fn())
const uploadLocalPathToRemoteMock = vi.hoisted(() => vi.fn())
const toastErrorMock = vi.hoisted(() => vi.fn())
const platformMock = vi.hoisted(() => ({
  desktop: false,
  openFileDialog: vi.fn(),
}))
const transportMock = vi.hoisted(() => ({
  remoteId: null as string | null,
}))
const tauriListenerMock = vi.hoisted(() => ({
  listeners: new Map<string, Array<(event: { payload: unknown }) => void>>(),
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
vi.mock("@/hooks/use-agent-skills", () => ({ useAgentSkills: () => [] }))
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
  isDesktop: () => platformMock.desktop,
  openFileDialog: platformMock.openFileDialog,
}))
vi.mock("@/lib/transport", () => ({
  getActiveRemoteConnectionId: () => transportMock.remoteId,
}))
vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>()
  return {
    ...actual,
    quickMessagesList: vi.fn().mockResolvedValue([]),
    readFileBase64: readFileBase64Mock,
    readLocalImagePathForAttachment: readLocalImagePathForAttachmentMock,
    readLocalPathForAttachment: readLocalPathForAttachmentMock,
    uploadAttachment: uploadAttachmentMock,
    uploadLocalPathToRemote: uploadLocalPathToRemoteMock,
  }
})
vi.mock("sonner", () => ({
  toast: { error: toastErrorMock, success: vi.fn() },
}))
vi.mock("@/components/shared/server-file-browser-dialog", () => ({
  ServerFileBrowserDialog: ({
    open,
    onSelect,
  }: {
    open: boolean
    onSelect: (paths: string[]) => void
  }) =>
    open ? (
      <button type="button" onClick={() => onSelect(["/server/outside.png"])}>
        Select server image
      </button>
    ) : null,
}))
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    listen: vi.fn(
      async (
        event: string,
        callback: (event: { payload: unknown }) => void
      ) => {
        const listeners = tauriListenerMock.listeners.get(event) ?? []
        listeners.push(callback)
        tauriListenerMock.listeners.set(event, listeners)
        return () => {
          const current = tauriListenerMock.listeners.get(event) ?? []
          tauriListenerMock.listeners.set(
            event,
            current.filter((item) => item !== callback)
          )
        }
      }
    ),
  }),
}))
vi.mock("@tauri-apps/api/event", () => ({
  TauriEvent: {
    DRAG_ENTER: "tauri://drag-enter",
    DRAG_OVER: "tauri://drag-over",
    DRAG_DROP: "tauri://drag-drop",
    DRAG_LEAVE: "tauri://drag-leave",
  },
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

describe("MessageInput local file upload", () => {
  afterEach(() => {
    cleanup()
    uploadAttachmentMock.mockReset()
  })

  it("sends an uploaded image as inline image data", async () => {
    const user = userEvent.setup()
    const onSend = vi.fn()
    const png = new File(["pixels"], "outside.png", { type: "image/png" })
    const inputClick = vi
      .spyOn(HTMLInputElement.prototype, "click")
      .mockImplementation(function (this: HTMLInputElement) {
        Object.defineProperty(this, "files", {
          configurable: true,
          value: [png],
        })
        this.dispatchEvent(new Event("change"))
      })

    renderInput({ onSend })
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageInput.addActions,
      })
    )
    await user.click(
      await screen.findByRole("menuitem", {
        name: enMessages.Folder.chat.messageInput.attachLocalUpload,
      })
    )

    const send = screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    await waitFor(() => expect(send).toBeEnabled())
    await user.click(send)

    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({
        blocks: [
          expect.objectContaining({
            type: "image",
            mime_type: "image/png",
            data: "cGl4ZWxz",
          }),
        ],
      }),
      null
    )
    expect(uploadAttachmentMock).not.toHaveBeenCalled()
    expect(inputClick).toHaveBeenCalledOnce()
    inputClick.mockRestore()
  })

  it("rejects an oversized uploaded image before it enters composer state", async () => {
    const user = userEvent.setup()
    const png = new File(["pixels"], "oversized.png", { type: "image/png" })
    Object.defineProperty(png, "size", {
      configurable: true,
      value: 20_000_001,
    })
    const inputClick = vi
      .spyOn(HTMLInputElement.prototype, "click")
      .mockImplementation(function (this: HTMLInputElement) {
        Object.defineProperty(this, "files", {
          configurable: true,
          value: [png],
        })
        this.dispatchEvent(new Event("change"))
      })

    renderInput({})
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageInput.addActions,
      })
    )
    await user.click(
      await screen.findByRole("menuitem", {
        name: enMessages.Folder.chat.messageInput.attachLocalUpload,
      })
    )

    await waitFor(() => expect(inputClick).toHaveBeenCalledOnce())
    expect(
      screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    ).toBeDisabled()
    expect(uploadAttachmentMock).not.toHaveBeenCalled()
    inputClick.mockRestore()
  })

  it("keeps successful attachments when another image read fails", async () => {
    const user = userEvent.setup()
    const onSend = vi.fn()
    const good = new File(["good"], "good.png", { type: "image/png" })
    const broken = new File(["broken"], "broken.png", {
      type: "image/png",
    })
    const notes = new File(["notes"], "notes.txt", { type: "text/plain" })
    uploadAttachmentMock.mockResolvedValue({ path: "/uploads/notes.txt" })
    const nativeReadAsDataUrl = FileReader.prototype.readAsDataURL
    const fileReaderSpy = vi
      .spyOn(FileReader.prototype, "readAsDataURL")
      .mockImplementation(function (this: FileReader, blob: Blob) {
        if ((blob as File).name === "broken.png") {
          queueMicrotask(() => {
            this.onerror?.(new ProgressEvent("error"))
          })
          return
        }
        nativeReadAsDataUrl.call(this, blob)
      })
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {})
    const inputClick = vi
      .spyOn(HTMLInputElement.prototype, "click")
      .mockImplementation(function (this: HTMLInputElement) {
        Object.defineProperty(this, "files", {
          configurable: true,
          value: [good, broken, notes],
        })
        this.dispatchEvent(new Event("change"))
      })

    renderInput({ onSend })
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageInput.addActions,
      })
    )
    await user.click(
      await screen.findByRole("menuitem", {
        name: enMessages.Folder.chat.messageInput.attachLocalUpload,
      })
    )

    await waitFor(() =>
      expect(uploadAttachmentMock).toHaveBeenCalledWith(notes, null)
    )
    const send = screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    await waitFor(() => expect(send).toBeEnabled())
    await user.click(send)

    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({
        blocks: expect.arrayContaining([
          expect.objectContaining({
            type: "image",
            data: "Z29vZA==",
          }),
          expect.objectContaining({
            type: "text",
            text: "[notes.txt](file:///uploads/notes.txt)",
          }),
        ]),
      }),
      null
    )
    expect(consoleError).toHaveBeenCalledWith(
      "[MessageInput] image attachment read failed (broken.png):",
      expect.any(Error)
    )
    inputClick.mockRestore()
    consoleError.mockRestore()
    fileReaderSpy.mockRestore()
  })

  it("keeps uploaded non-image files on the resource path", async () => {
    const user = userEvent.setup()
    const onSend = vi.fn()
    const text = new File(["notes"], "notes.txt", { type: "text/plain" })
    uploadAttachmentMock.mockResolvedValue({ path: "/uploads/notes.txt" })
    const inputClick = vi
      .spyOn(HTMLInputElement.prototype, "click")
      .mockImplementation(function (this: HTMLInputElement) {
        Object.defineProperty(this, "files", {
          configurable: true,
          value: [text],
        })
        this.dispatchEvent(new Event("change"))
      })

    renderInput({ onSend })
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageInput.addActions,
      })
    )
    await user.click(
      await screen.findByRole("menuitem", {
        name: enMessages.Folder.chat.messageInput.attachLocalUpload,
      })
    )

    await waitFor(() =>
      expect(uploadAttachmentMock).toHaveBeenCalledWith(text, null)
    )
    const send = screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    await waitFor(() => expect(send).toBeEnabled())
    await user.click(send)

    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({
        blocks: [
          expect.objectContaining({
            type: "text",
            text: "[notes.txt](file:///uploads/notes.txt)",
          }),
        ],
      }),
      null
    )
    inputClick.mockRestore()
  })
})

describe("MessageInput selected image paths", () => {
  afterEach(() => {
    cleanup()
    platformMock.desktop = false
    platformMock.openFileDialog.mockReset()
    readFileBase64Mock.mockReset()
  })

  it("reads a native-picker image as bounded inline data", async () => {
    const user = userEvent.setup()
    const onSend = vi.fn()
    platformMock.desktop = true
    platformMock.openFileDialog.mockResolvedValue([
      "/outside/image.png",
      "/outside/notes.txt",
    ])
    readFileBase64Mock.mockResolvedValue("bmF0aXZlLWltYWdl")

    renderInput({ onSend })
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageInput.addActions,
      })
    )
    await user.click(
      await screen.findByRole("menuitem", {
        name: enMessages.Folder.chat.messageInput.attachFiles,
      })
    )

    await waitFor(() =>
      expect(readFileBase64Mock).toHaveBeenCalledWith(
        "/outside/image.png",
        20_000_000
      )
    )
    const send = screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    await waitFor(() => expect(send).toBeEnabled())
    await user.click(send)

    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({
        blocks: expect.arrayContaining([
          expect.objectContaining({
            type: "image",
            data: "bmF0aXZlLWltYWdl",
          }),
          expect.objectContaining({
            type: "text",
            text: "[notes.txt](file:///outside/notes.txt)",
          }),
        ]),
      }),
      null
    )
  })

  it("routes a server-picker image through the bounded reader", async () => {
    const user = userEvent.setup()
    readFileBase64Mock.mockResolvedValue("c2VydmVyLWltYWdl")

    renderInput({})
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageInput.addActions,
      })
    )
    await user.click(
      await screen.findByRole("menuitem", {
        name: enMessages.Folder.chat.messageInput.attachServerFile,
      })
    )
    await user.click(await screen.findByText("Select server image"))

    await waitFor(() =>
      expect(readFileBase64Mock).toHaveBeenCalledWith(
        "/server/outside.png",
        20_000_000
      )
    )
  })

  it("does not append an image when a selected path cannot be read", async () => {
    const user = userEvent.setup()
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {})
    platformMock.desktop = true
    platformMock.openFileDialog.mockResolvedValue(["/outside/unreadable.png"])
    readFileBase64Mock.mockRejectedValue(new Error("read denied"))

    renderInput({})
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageInput.addActions,
      })
    )
    await user.click(
      await screen.findByRole("menuitem", {
        name: enMessages.Folder.chat.messageInput.attachFiles,
      })
    )

    await waitFor(() => expect(readFileBase64Mock).toHaveBeenCalledOnce())
    expect(
      screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    ).toBeDisabled()
    expect(
      screen.queryByRole("button", { name: /Remove unreadable\.png/ })
    ).toBeNull()
    expect(consoleError).toHaveBeenCalledWith(
      "[MessageInput] drop image path failed (/outside/unreadable.png):",
      expect.any(Error)
    )
    consoleError.mockRestore()
  })

  it("routes a whole-file session image through the bounded reader", async () => {
    readFileBase64Mock.mockResolvedValue("c2Vzc2lvbi1pbWFnZQ==")
    renderInput({ attachmentTabId: "tab-image" })

    act(() => {
      emitAttachFileToSession({
        tabId: "tab-image",
        path: "/outside/session.png",
      })
    })

    await waitFor(() =>
      expect(readFileBase64Mock).toHaveBeenCalledWith(
        "/outside/session.png",
        20_000_000
      )
    )
  })
})

describe("MessageInput remote desktop paths", () => {
  afterEach(() => {
    cleanup()
    platformMock.desktop = false
    transportMock.remoteId = null
    tauriListenerMock.listeners.clear()
    readLocalImagePathForAttachmentMock.mockReset()
    readLocalPathForAttachmentMock.mockReset()
    uploadLocalPathToRemoteMock.mockReset()
    toastErrorMock.mockReset()
  })

  async function renderRemoteAndDrop(
    paths: string[],
    props: Partial<ComponentProps<typeof MessageInput>> = {}
  ) {
    platformMock.desktop = true
    transportMock.remoteId = "remote-1"
    tauriListenerMock.listeners.clear()

    const rendered = renderInput(props)
    const host = rendered.container.firstElementChild as HTMLElement | null
    expect(host).not.toBeNull()
    vi.spyOn(host!, "getBoundingClientRect").mockReturnValue({
      left: 0,
      top: 0,
      right: 100,
      bottom: 100,
      width: 100,
      height: 100,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    })
    await waitFor(() =>
      expect(
        tauriListenerMock.listeners.get("tauri://drag-drop")?.length
      ).toBeGreaterThan(0)
    )

    act(() => {
      tauriListenerMock.listeners.get("tauri://drag-drop")?.at(-1)?.({
        payload: {
          paths,
          position: { x: 10, y: 10 },
        },
      })
    })
    return rendered
  }

  it("keeps remote desktop images inline and uploads only resources", async () => {
    const onSend = vi.fn()
    readLocalImagePathForAttachmentMock.mockResolvedValue({
      fileName: "outside.png",
      mimeType: "image/png",
      size: 10_000_000,
      dataBase64: "cmVtb3RlLWltYWdl",
    })
    uploadLocalPathToRemoteMock.mockResolvedValue({
      path: "/uploads/notes.txt",
    })

    await renderRemoteAndDrop(["/outside/outside.png", "/outside/notes.txt"], {
      onSend,
    })

    await waitFor(() =>
      expect(readLocalImagePathForAttachmentMock).toHaveBeenCalledWith(
        "/outside/outside.png"
      )
    )
    expect(readLocalPathForAttachmentMock).not.toHaveBeenCalled()
    expect(uploadLocalPathToRemoteMock).toHaveBeenCalledTimes(1)
    expect(uploadLocalPathToRemoteMock).toHaveBeenCalledWith(
      "/outside/notes.txt",
      null
    )

    const send = screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    await waitFor(() => expect(send).toBeEnabled())
    await userEvent.setup().click(send)
    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({
        blocks: expect.arrayContaining([
          expect.objectContaining({
            type: "image",
            mime_type: "image/png",
            data: "cmVtb3RlLWltYWdl",
          }),
          expect.objectContaining({
            type: "text",
            text: "[notes.txt](file:///uploads/notes.txt)",
          }),
        ]),
      }),
      null
    )
    expect(send).toBeDisabled()
    await userEvent.setup().click(send)
    expect(onSend).toHaveBeenCalledOnce()
  })

  it("removes remote image bytes from the draft", async () => {
    readLocalImagePathForAttachmentMock.mockResolvedValue({
      fileName: "outside.png",
      mimeType: "image/png",
      size: 6,
      dataBase64: "cmVtb3RlLWltYWdl",
    })

    await renderRemoteAndDrop(["/outside/outside.png"])
    const remove = await screen.findByRole("button", {
      name: "Remove outside.png",
    })
    await userEvent.setup().click(remove)

    expect(
      screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    ).toBeDisabled()
  })

  it("does not upload a remote image after its local read fails", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {})
    readLocalImagePathForAttachmentMock.mockRejectedValue(
      new Error("read denied")
    )

    await renderRemoteAndDrop(["/outside/unreadable.png"])
    await waitFor(() =>
      expect(readLocalImagePathForAttachmentMock).toHaveBeenCalledOnce()
    )

    expect(uploadLocalPathToRemoteMock).not.toHaveBeenCalled()
    expect(
      screen.getByTitle(enMessages.Folder.chat.messageInput.send)
    ).toBeDisabled()
    expect(consoleError).toHaveBeenCalledWith(
      "[MessageInput] remote path upload failed (unreadable.png):",
      expect.any(Error)
    )
    consoleError.mockRestore()
  })

  it("shows separate 20 MB image and 2 MB resource limits", async () => {
    const tooLarge = (limit: number) => ({
      code: "io_error",
      message: "Local file exceeds the size limit",
      i18n_key: "errors.upload.tooLarge",
      i18n_params: { limit: String(limit) },
    })
    readLocalImagePathForAttachmentMock.mockRejectedValue(tooLarge(20_000_000))
    uploadLocalPathToRemoteMock.mockRejectedValue(tooLarge(2 * 1024 * 1024))

    await renderRemoteAndDrop([
      "/outside/oversize.png",
      "/outside/oversize.txt",
    ])

    await waitFor(() => expect(toastErrorMock).toHaveBeenCalledTimes(2))
    expect(toastErrorMock).toHaveBeenCalledWith(
      "oversize.png exceeds the 20MB upload limit and was skipped."
    )
    expect(toastErrorMock).toHaveBeenCalledWith(
      "oversize.txt exceeds the 2MB upload limit and was skipped."
    )
  })
})
