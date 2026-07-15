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

describe("MessageInput repeat intent", () => {
  afterEach(() => {
    cleanup()
    composerHandle.current = null
  })

  async function mountComposer(
    props: Partial<React.ComponentProps<typeof MessageInput>> = {}
  ) {
    const utils = renderInput(props)
    await waitFor(
      () => expect(composerHandle.current?.getEditor()).toBeTruthy(),
      { timeout: 5000 }
    )
    return utils
  }

  async function setComposerText(text: string) {
    await act(async () => {
      composerHandle.current?.setText(text)
    })
  }

  function clickSend(container: HTMLElement) {
    const sendButton = container.querySelector<HTMLButtonElement>(
      `button[title="${enMessages.Folder.chat.messageInput.send}"]`
    )
    expect(sendButton).not.toBeNull()
    expect(sendButton).not.toBeDisabled()
    fireEvent.click(sendButton!)
  }

  it("opens a confirm dialog for trailing xN and does not enqueue yet", async () => {
    const onEnqueueMany = vi.fn()
    const onSend = vi.fn()
    const { container } = await mountComposer({ onEnqueueMany, onSend })
    await setComposerText("继续x10")
    clickSend(container)

    expect(
      await screen.findByRole("alertdialog", {
        name: enMessages.Folder.chat.messageQueue.repeatIntentTitle,
      })
    ).toBeInTheDocument()
    expect(onEnqueueMany).not.toHaveBeenCalled()
    expect(onSend).not.toHaveBeenCalled()
  })

  it("enqueues N base drafts and clears the composer on confirm", async () => {
    const user = userEvent.setup()
    const onEnqueueMany = vi.fn()
    const { container } = await mountComposer({ onEnqueueMany })
    await setComposerText("继续 x 3")
    clickSend(container)

    await screen.findByRole("alertdialog", {
      name: enMessages.Folder.chat.messageQueue.repeatIntentTitle,
    })
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageQueue.repeatIntentConfirm.replace(
          "{count}",
          "3"
        ),
      })
    )

    await waitFor(() => expect(onEnqueueMany).toHaveBeenCalledTimes(1))
    const [draft, modeId, count] = onEnqueueMany.mock.calls[0]
    expect(draft.displayText).toBe("继续")
    expect(modeId).toBeNull()
    expect(count).toBe(3)
    await waitFor(() =>
      expect(composerHandle.current?.getText().trim() ?? "").toBe("")
    )
  })

  it("keeps the original text when the dialog is cancelled", async () => {
    const user = userEvent.setup()
    const onEnqueueMany = vi.fn()
    const { container } = await mountComposer({ onEnqueueMany })
    await setComposerText("continue X 4")
    clickSend(container)

    await screen.findByRole("alertdialog", {
      name: enMessages.Folder.chat.messageQueue.repeatIntentTitle,
    })
    await user.click(
      screen.getByRole("button", {
        name: enMessages.Folder.chat.messageQueue.repeatIntentCancel,
      })
    )

    await waitFor(() =>
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
    )
    expect(onEnqueueMany).not.toHaveBeenCalled()
    expect(composerHandle.current?.getText()).toContain("continue X 4")
  })

  it("skips detection while editing a queue item", async () => {
    const onSaveQueueEdit = vi.fn()
    const onEnqueueMany = vi.fn()
    const { container } = await mountComposer({
      isEditingQueueItem: true,
      editingItemId: "q1",
      editingDraftText: "继续x10",
      onSaveQueueEdit,
      onEnqueueMany,
    })
    await waitFor(() =>
      expect(composerHandle.current?.getText() ?? "").toContain("继续")
    )
    const saveButton = container.querySelector<HTMLButtonElement>(
      `button[title="${enMessages.Folder.chat.messageQueue.saveEdit}"]`
    )
    expect(saveButton).not.toBeNull()
    fireEvent.click(saveButton!)
    await waitFor(() => expect(onSaveQueueEdit).toHaveBeenCalledTimes(1))
    expect(onEnqueueMany).not.toHaveBeenCalled()
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
  })
})
