"use client"

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react"
import { type Editor, type JSONContent } from "@tiptap/core"
import { EditorContent, useEditor } from "@tiptap/react"
import { exitSuggestion } from "@tiptap/suggestion"

import { cn } from "@/lib/utils"

import { buildComposerExtensions } from "./editor-config"
import { decideComposerKey } from "./submit-key"
import type {
  MentionController,
  MentionRenderState,
} from "./suggestion/mention-suggestion"
import {
  MENTION_LISTBOX_ID,
  SuggestionPopup,
} from "./suggestion/suggestion-popup"
import type {
  MentionUiLabels,
  ReferenceSearch,
  SuggestionPopupHandle,
} from "./suggestion/types"
import type { ReferenceAttrs } from "./types"

/**
 * Imperative handle exposed to the parent (e.g. the message input that owns
 * attachments, queue and send orchestration). The parent reads/writes Markdown
 * and controls focus without re-rendering the editor.
 */
export interface RichComposerHandle {
  /** Serialize the current document to Markdown. */
  getMarkdown: () => string
  /** Replace the whole document from a Markdown string. */
  setMarkdown: (markdown: string) => void
  /**
   * Replace the whole document from a Tiptap JSON doc — used to hydrate a v2
   * draft or a queue-edit payload, preserving reference badges that a Markdown
   * round-trip would downgrade to plain links.
   */
  setDoc: (doc: JSONContent) => void
  /** Clear the document. */
  clear: () => void
  /** Focus the editor at the end of the document. */
  focus: () => void
  /** Whether the document is empty (no text, no nodes). */
  isEmpty: () => boolean
  /** Serialize the current document to Tiptap JSON (for draft persistence). */
  getJSON: () => JSONContent
  /** Insert Markdown at the current selection (quick messages, appended text). */
  insertMarkdownAtCursor: (markdown: string) => void
  /** Insert an inline reference badge at the current selection. */
  insertReference: (attrs: ReferenceAttrs) => void
  /** Escape hatch to the underlying editor (null until initialized). */
  getEditor: () => Editor | null
}

export interface RichComposerProps {
  /** Initial content, parsed as Markdown. Applied once on creation. */
  defaultMarkdown?: string
  placeholder?: string
  autoFocus?: boolean
  disabled?: boolean
  /** Accessible label for the editing surface. */
  ariaLabel?: string
  /** Outer wrapper className (host controls border/ring/max-height). */
  className?: string
  /** Inline style for the outer wrapper (e.g. max-height). */
  style?: CSSProperties
  /**
   * Fires on every document change with the serialized Markdown. Serialization
   * runs once per keystroke *only when a handler is attached* (the call is
   * skipped entirely otherwise). Callers that persist drafts must debounce —
   * the Phase 3 draft layer owns that.
   */
  onChange?: (markdown: string) => void
  /**
   * Submit intent: fired when the `submitShortcut` binding is pressed while not
   * composing (IME-safe) and not on a structural bare Enter (code block / list).
   * The host decides what "submit" means.
   */
  onSubmit?: () => void
  onFocus?: () => void
  onBlur?: () => void
  /**
   * Fired once the (async, `immediatelyRender:false`) editor has mounted and any
   * `defaultMarkdown` has been applied. The host uses this to hydrate a draft /
   * queue-edit document via the imperative handle, which isn't usable earlier.
   */
  onReady?: () => void
  /**
   * Enables the unified `@` mention panel. Resolves the typed query into
   * grouped suggestions (files/agents/sessions/commits/skills). MUST be
   * referentially stable (memoize it) — it is a dependency of the panel's fetch
   * effect. Omit to disable mentions.
   */
  referenceSearch?: ReferenceSearch
  /**
   * Localized chrome for the `@` panel (empty / loading / listbox name / "more
   * results" hint / result-count announcement). English fallbacks apply when
   * omitted. Render-only — safe to pass a fresh object per render.
   */
  mentionUiLabels?: MentionUiLabels
  /**
   * Key binding (matchShortcutEvent form) that sends the message. Default
   * `"enter"`. When set to a non-Enter binding, a plain Enter inserts a newline.
   */
  submitShortcut?: string
  /** Key binding that inserts a line break instead of sending. Default `"shift+enter"`. */
  newlineShortcut?: string
  /**
   * When true, an external (parent-driven) menu — e.g. the `/` runtime command
   * list — owns navigation/confirm keys, so the composer never submits or breaks
   * while it is open. The internal `@` panel does not need this flag.
   */
  isExternalMenuOpen?: boolean
  /**
   * Called for every keydown while `isExternalMenuOpen` is true, BEFORE the
   * editor acts. ProseMirror's DOM handler fires before a host capture handler
   * could, so menu navigation has to be routed here. Return true for keys the
   * menu consumed (Arrow/Enter/Tab/Escape) so the editor does nothing; return
   * false (e.g. a letter that filters the list) to let normal editing proceed.
   */
  onExternalMenuKeyDown?: (event: KeyboardEvent) => boolean
  /**
   * Called on paste before the editor handles it. Return true when the paste was
   * consumed out-of-band (e.g. an image/file became an attachment) so the editor
   * does not also insert it as text.
   */
  onPasteFiles?: (event: ClipboardEvent) => boolean
}

/**
 * Rich-text composer: a Tiptap editor with live WYSIWYG Markdown, IME-safe
 * Enter-to-submit, inline reference badges, and an optional unified `@` mention
 * panel (enabled by `referenceSearch`). Not yet wired into message-input — that
 * integration (drafts, attachments, real data sources) is Phase 3.
 */
export const RichComposer = forwardRef<RichComposerHandle, RichComposerProps>(
  function RichComposer(
    {
      defaultMarkdown,
      placeholder,
      autoFocus,
      disabled,
      ariaLabel,
      className,
      style,
      onChange,
      onSubmit,
      onFocus,
      onBlur,
      onReady,
      referenceSearch,
      mentionUiLabels,
      submitShortcut,
      newlineShortcut,
      isExternalMenuOpen,
      onExternalMenuKeyDown,
      onPasteFiles,
    },
    ref
  ) {
    // Keep callbacks in refs so the editor (and its keymap) is created once and
    // never torn down just because a parent re-renders with new closures.
    const onChangeRef = useRef(onChange)
    const onSubmitRef = useRef(onSubmit)
    const onFocusRef = useRef(onFocus)
    const onBlurRef = useRef(onBlur)
    const onReadyRef = useRef(onReady)
    // Latest referenceSearch, read at event time so the mention plugin (always
    // installed) is gated on whether mentions are currently enabled — robust to
    // the prop being added/removed after the editor is created once.
    const referenceSearchRef = useRef(referenceSearch)
    const submitShortcutRef = useRef(submitShortcut)
    const newlineShortcutRef = useRef(newlineShortcut)
    const isExternalMenuOpenRef = useRef(isExternalMenuOpen)
    const onExternalMenuKeyDownRef = useRef(onExternalMenuKeyDown)
    const onPasteFilesRef = useRef(onPasteFiles)
    // The live editor, captured for command access inside editorProps handlers
    // (which are created before `editor` is assigned in this closure).
    const editorInstanceRef = useRef<Editor | null>(null)
    useEffect(() => {
      onChangeRef.current = onChange
      onSubmitRef.current = onSubmit
      onFocusRef.current = onFocus
      onBlurRef.current = onBlur
      onReadyRef.current = onReady
      referenceSearchRef.current = referenceSearch
      submitShortcutRef.current = submitShortcut
      newlineShortcutRef.current = newlineShortcut
      isExternalMenuOpenRef.current = isExternalMenuOpen
      onExternalMenuKeyDownRef.current = onExternalMenuKeyDown
      onPasteFilesRef.current = onPasteFiles
    })

    // ── Unified `@` mention panel state bridge ──
    // The suggestion plugin lives in ProseMirror; its lifecycle is bridged to
    // this React state so the popup can render in-tree (where data hooks work).
    const [mentionState, setMentionState] = useState<MentionRenderState | null>(
      null
    )
    // Mirrors `mentionState != null` for synchronous reads inside handleKeyDown
    // (so Enter defers to the panel without waiting for a re-render).
    const mentionOpenRef = useRef(false)
    const popupRef = useRef<SuggestionPopupHandle>(null)
    // Stable controller created once (refs/setState are stable), so the editor
    // is built a single time with it.
    const mentionController = useMemo<MentionController>(
      () => ({
        onStart: (mention) => {
          // Inert unless mentions are enabled (no referenceSearch → no panel).
          if (!referenceSearchRef.current) return
          mentionOpenRef.current = true
          setMentionState(mention)
        },
        onUpdate: (mention) => {
          if (!referenceSearchRef.current) return
          setMentionState(mention)
        },
        onExit: () => {
          mentionOpenRef.current = false
          setMentionState(null)
        },
        onKeyDown: (event) => popupRef.current?.onKeyDown(event) ?? false,
      }),
      []
    )

    const editor = useEditor({
      // Static export / SSR safety: never render on the server.
      immediatelyRender: false,
      // The mention plugin is always installed (the editor is created once);
      // it stays inert until `referenceSearch` is set (checked at runtime in the
      // controller). `mentionController` (stable, from useMemo) captures refs
      // but only dereferences them inside event-time callbacks, never during
      // render — the React Compiler lint can't prove that. Mirrors Tiptap's own
      // React suggestion pattern (render() → component.ref.onKeyDown).
      // eslint-disable-next-line react-hooks/refs
      extensions: buildComposerExtensions({ placeholder, mentionController }),
      editable: !disabled,
      autofocus: autoFocus ? "end" : false,
      editorProps: {
        attributes: {
          class: "codeg-composer-content",
          role: "textbox",
          "aria-multiline": "true",
          ...(ariaLabel ? { "aria-label": ariaLabel } : {}),
        },
        handleKeyDown: (view, event) => {
          // The internal `@` panel's suggestion plugin owns its navigation keys;
          // never submit/break while it is open.
          if (mentionOpenRef.current) return false
          // An external (host) menu — e.g. the `/` runtime command list — gets
          // first refusal on keys while open. ProseMirror's DOM handler runs
          // before any host capture handler could, so routing happens here: the
          // host returns true for keys it consumed (Arrow/Enter/Tab/Escape) so
          // the editor does nothing, or false (e.g. a letter that filters the
          // list) to let the inline token keep growing.
          if (isExternalMenuOpenRef.current) {
            return onExternalMenuKeyDownRef.current?.(event) ?? false
          }
          // Bindings are free-form (Enter, Shift+Enter, Mod+Enter, Tab, …), so
          // we can't pre-filter by key. Instead, run a cheap first pass with no
          // structural context: if neither binding matches it's plain typing —
          // bail before resolving the (slightly costlier) editor structure.
          // (A bare Enter still matches the default submit binding here, so we
          // never wrongly skip it; the code-block/list carve-out is applied in
          // the second pass below, which only narrows the result.)
          const keyEvent = {
            key: event.key,
            shiftKey: event.shiftKey,
            altKey: event.altKey,
            ctrlKey: event.ctrlKey,
            metaKey: event.metaKey,
            isComposing: event.isComposing,
            keyCode: (event as { keyCode?: number }).keyCode ?? 0,
          }
          const bindings = {
            submit: submitShortcutRef.current ?? "enter",
            newline: newlineShortcutRef.current ?? "shift+enter",
          }
          if (
            decideComposerKey(
              keyEvent,
              { composing: view.composing, inCodeBlock: false, inList: false },
              bindings
            ) === null
          ) {
            return false
          }
          // A binding matched (or a bare Enter needing the structural carve-out):
          // resolve code-block / list context and decide for real.
          const { $from } = view.state.selection
          let inCodeBlock = $from.parent.type.spec.code === true
          let inList = false
          for (let depth = $from.depth; depth > 0; depth--) {
            const name = $from.node(depth).type.name
            if (name === "codeBlock") inCodeBlock = true
            if (name === "listItem" || name === "taskItem") inList = true
          }
          const action = decideComposerKey(
            keyEvent,
            { composing: view.composing, inCodeBlock, inList },
            bindings
          )
          if (action === "submit") {
            // Only consume the key once a handler actually runs; otherwise let
            // the editor apply its default (e.g. Enter splits the paragraph).
            if (!onSubmitRef.current) return false
            onSubmitRef.current()
            return true
          }
          if (action === "newline") {
            const ed = editorInstanceRef.current
            if (!ed) return false
            // Code blocks take a literal newline; everywhere else a hard break.
            if (inCodeBlock) ed.commands.insertContent("\n")
            else ed.commands.setHardBreak()
            return true
          }
          return false
        },
        handlePaste: (_view, event) =>
          onPasteFilesRef.current?.(event) === true,
      },
      onCreate: ({ editor }) => {
        editorInstanceRef.current = editor
        if (defaultMarkdown) {
          editor.commands.setContent(defaultMarkdown, {
            contentType: "markdown",
            emitUpdate: false,
          })
        }
        // The imperative handle is now usable; let the host hydrate a draft /
        // queue-edit document that a Markdown `defaultMarkdown` can't represent.
        onReadyRef.current?.()
      },
      onDestroy: () => {
        editorInstanceRef.current = null
      },
      onUpdate: ({ editor }) => {
        onChangeRef.current?.(editor.getMarkdown())
      },
      onFocus: () => onFocusRef.current?.(),
      onBlur: () => onBlurRef.current?.(),
    })

    // Reflect disabled changes onto the live editor. Pass emitUpdate=false so
    // toggling editability never fires onUpdate/onChange without a real edit.
    useEffect(() => {
      editor?.setEditable(!disabled, false)
    }, [editor, disabled])

    useImperativeHandle(
      ref,
      (): RichComposerHandle => ({
        getMarkdown: () => editor?.getMarkdown() ?? "",
        setMarkdown: (markdown) =>
          editor?.commands.setContent(markdown, { contentType: "markdown" }),
        setDoc: (doc) => editor?.commands.setContent(doc),
        clear: () => editor?.commands.clearContent(true),
        focus: () => editor?.commands.focus("end"),
        isEmpty: () => editor?.isEmpty ?? true,
        getJSON: () => editor?.getJSON() ?? { type: "doc", content: [] },
        insertMarkdownAtCursor: (markdown) => {
          editor
            ?.chain()
            .focus()
            .insertContent(markdown, { contentType: "markdown" })
            .run()
        },
        insertReference: (attrs) => {
          editor?.chain().focus().insertReference(attrs).run()
        },
        getEditor: () => editor ?? null,
      }),
      [editor]
    )

    const closeMention = useCallback(() => {
      mentionOpenRef.current = false
      setMentionState(null)
      // Also dismiss the Tiptap suggestion plugin so its state can't stay active
      // while React thinks the panel is closed (onExit will also fire).
      const view = editor?.view
      if (view) exitSuggestion(view)
    }, [editor])

    // If mentions get disabled while a panel is open, actively dismiss it so the
    // editor's Enter handling and the plugin state return to normal (the popup
    // also unmounts via the render guard below).
    useEffect(() => {
      if (!referenceSearch && mentionOpenRef.current) closeMention()
    }, [referenceSearch, closeMention])

    const handleReferenceSelect = useCallback(
      (reference: ReferenceAttrs, range: { from: number; to: number }) => {
        editor
          ?.chain()
          .focus()
          .deleteRange(range)
          .insertReference(reference)
          .insertContent(" ")
          .run()
        closeMention()
      },
      [editor, closeMention]
    )

    // Combobox ARIA on the editing surface: DOM focus stays in the editor while
    // the `@` panel is open, so the controlled-listbox relationship lives on the
    // contentEditable. `aria-activedescendant` is mirrored from the popup's
    // active row (below); here we toggle `aria-controls` and clear both when the
    // panel closes. (role stays "textbox" — a multiline editor that surfaces an
    // autocomplete list, the recognized textbox-autocomplete pattern.)
    const isMentionOpen = mentionState !== null
    useEffect(() => {
      const dom = editor?.view.dom
      if (!dom) return
      if (isMentionOpen) {
        // `aria-autocomplete="list"` tells AT this textbox offers a list of
        // completions; `aria-controls` names the listbox it drives.
        dom.setAttribute("aria-autocomplete", "list")
        dom.setAttribute("aria-controls", MENTION_LISTBOX_ID)
      } else {
        dom.removeAttribute("aria-autocomplete")
        dom.removeAttribute("aria-controls")
        dom.removeAttribute("aria-activedescendant")
      }
    }, [editor, isMentionOpen])

    const handleActiveOptionChange = useCallback(
      (optionId: string | null) => {
        const dom = editor?.view.dom
        if (!dom) return
        if (optionId) dom.setAttribute("aria-activedescendant", optionId)
        else dom.removeAttribute("aria-activedescendant")
      },
      [editor]
    )

    return (
      <div
        className={cn("codeg-composer flex min-h-0 flex-col", className)}
        style={style}
        data-disabled={disabled || undefined}
      >
        <EditorContent
          editor={editor}
          className="codeg-composer-scroll min-h-0 flex-1 overflow-y-auto px-3 py-2 text-base md:text-sm"
        />
        {referenceSearch && mentionState && (
          <SuggestionPopup
            ref={popupRef}
            state={mentionState}
            search={referenceSearch}
            onSelect={handleReferenceSelect}
            onClose={closeMention}
            onActiveOptionChange={handleActiveOptionChange}
            emptyLabel={mentionUiLabels?.empty}
            loadingLabel={mentionUiLabels?.loading}
            listboxLabel={mentionUiLabels?.listbox}
            moreLabel={mentionUiLabels?.more}
            countLabel={mentionUiLabels?.count}
          />
        )}
      </div>
    )
  }
)
