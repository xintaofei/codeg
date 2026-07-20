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

import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { cn } from "@/lib/utils"

import { buildComposerExtensions } from "./editor-config"
import { textToDoc, textToInlineContent } from "./plain-text-content"
import { serializeDocToText } from "./to-prompt-blocks"
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
import type { ReferenceAttrs, ReferenceKind } from "./types"

/**
 * Imperative handle exposed to the parent (e.g. the message input that owns
 * attachments, queue and send orchestration). The parent reads/writes plain text
 * and controls focus without re-rendering the editor.
 */
export interface RichComposerHandle {
  /** Serialize the current document to plain text (references → their inline
   *  token, hard breaks → newlines). */
  getText: () => string
  /** Replace the whole document from a plain-text string. */
  setText: (text: string) => void
  /**
   * Replace the whole document from a Tiptap JSON doc — used to hydrate a v2
   * draft or a queue-edit payload, preserving reference badges that a plain-text
   * round-trip would downgrade to their token.
   */
  setDoc: (doc: JSONContent) => void
  /** Clear the document. */
  clear: () => void
  /** Focus the editor at the end of the document. */
  focus: () => void
  /**
   * Focus the editor and place the caret at the document position nearest the
   * given viewport coordinates (native-textarea behavior). Falls back to the
   * end of the document when the point can't be mapped (e.g. it lands outside
   * the editing surface). Used by the host to honor where a user clicks in the
   * composer's blank chrome instead of always jumping to the end.
   */
  focusAtCoords: (clientX: number, clientY: number) => void
  /** Whether the document is empty (no text, no nodes). */
  isEmpty: () => boolean
  /** Serialize the current document to Tiptap JSON (for draft persistence). */
  getJSON: () => JSONContent
  /** Insert plain text at the current selection (quick messages, appended text). */
  insertTextAtCursor: (text: string) => void
  /** Insert an inline reference badge at the current selection. */
  insertReference: (attrs: ReferenceAttrs) => void
  /** Escape hatch to the underlying editor (null until initialized). */
  getEditor: () => Editor | null
}

export interface RichComposerProps {
  /** Initial content, inserted as plain text. Applied once on creation. */
  defaultText?: string
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
   * Fires on every document change with the serialized plain text. Serialization
   * runs once per keystroke *only when a handler is attached* (the call is
   * skipped entirely otherwise). Callers that persist drafts must debounce —
   * the draft layer owns that.
   */
  onChange?: (text: string) => void
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
   * `defaultText` has been applied. The host uses this to hydrate a draft /
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
   * Localized per-kind tab labels for the `@` panel (Agents/Files/Sessions/
   * Commits/Skills). English fallbacks apply when omitted. Render-only.
   */
  tabLabels?: Record<ReferenceKind, string>
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
  /**
   * Called on drop before the editor handles it. Return true when the drop was
   * consumed out-of-band (e.g. a dragged file-tree entry became an inline
   * reference) so ProseMirror does not also insert the drag's `text/plain`
   * fallback as literal text. The host must `stopPropagation()` itself if it
   * needs to keep the drop from bubbling to an ancestor container handler.
   */
  onDropFiles?: (event: DragEvent) => boolean
  /**
   * Paste-without-formatting intent: fired when `Ctrl/⌘+Shift+V` is pressed. The
   * host owns the clipboard read (and its non-secure fallback). Return true when
   * the host took over so the editor consumes the key and the browser's native
   * rich paste is suppressed; return false (or omit the prop) to let the native
   * "paste and match style" proceed (e.g. in a non-secure context where the async
   * clipboard read is unavailable).
   */
  onPlainPaste?: () => boolean
}

/**
 * Plain-text message composer: a Tiptap editor with IME-safe Enter-to-submit,
 * inline reference badges (the five built-in reference kinds), and an optional
 * unified `@` mention panel (enabled by `referenceSearch`). No Markdown — typed
 * formatting stays literal; see {@link buildComposerExtensions}.
 */
export const RichComposer = forwardRef<RichComposerHandle, RichComposerProps>(
  function RichComposer(
    {
      defaultText,
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
      tabLabels,
      submitShortcut,
      newlineShortcut,
      isExternalMenuOpen,
      onExternalMenuKeyDown,
      onPasteFiles,
      onDropFiles,
      onPlainPaste,
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
    const onDropFilesRef = useRef(onDropFiles)
    const onPlainPasteRef = useRef(onPlainPaste)
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
      onDropFilesRef.current = onDropFiles
      onPlainPasteRef.current = onPlainPaste
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
          // Paste without formatting: Ctrl/⌘+Shift+V routes to the host, which
          // owns the clipboard read. Consume the key (suppressing the browser's
          // native rich paste) only when the host takes over; otherwise return
          // false so the native "paste and match style" proceeds — the correct
          // fallback in a non-secure context where the async read is unavailable.
          if (matchShortcutEvent(event, "mod+shift+v")) {
            return onPlainPasteRef.current?.() === true
          }
          // Bindings are free-form (Enter, Shift+Enter, Mod+Enter, Tab, …). The
          // composer is plain text, so there is no code block or list to carve
          // out — inCodeBlock/inList are always false and one decision suffices.
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
          const action = decideComposerKey(
            keyEvent,
            { composing: view.composing, inCodeBlock: false, inList: false },
            bindings
          )
          if (action === "submit") {
            // Only consume the key once a handler actually runs; otherwise let
            // the editor apply its default (Enter splits the paragraph).
            if (!onSubmitRef.current) return false
            onSubmitRef.current()
            return true
          }
          if (action === "newline") {
            const ed = editorInstanceRef.current
            if (!ed) return false
            ed.commands.setHardBreak()
            return true
          }
          return false
        },
        handlePaste: (_view, event) =>
          onPasteFilesRef.current?.(event) === true,
        handleDrop: (_view, event) => onDropFilesRef.current?.(event) === true,
      },
      onCreate: ({ editor }) => {
        editorInstanceRef.current = editor
        if (defaultText) {
          editor.commands.setContent(textToDoc(defaultText), {
            emitUpdate: false,
          })
        }
        // The imperative handle is now usable; let the host hydrate a draft /
        // queue-edit document that a plain `defaultText` can't represent.
        onReadyRef.current?.()
      },
      onDestroy: () => {
        editorInstanceRef.current = null
      },
      onUpdate: ({ editor }) => {
        onChangeRef.current?.(serializeDocToText(editor.state.doc))
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
        getText: () => (editor ? serializeDocToText(editor.state.doc) : ""),
        setText: (text) => editor?.commands.setContent(textToDoc(text)),
        setDoc: (doc) => editor?.commands.setContent(doc),
        clear: () => editor?.commands.clearContent(true),
        focus: () => editor?.commands.focus("end"),
        focusAtCoords: (clientX, clientY) => {
          if (!editor) return
          const view = editor.view
          // Map the click point to a document position. Chrome clicks land on
          // the composer's padding/dead space, which is *outside* the
          // contenteditable (`view.dom` is the inner `.ProseMirror`; the
          // `px-3 py-2` padding lives on the EditorContent wrapper), so
          // `posAtCoords` returns null there. Clamp the point onto the editor's
          // own box and retry, so left/top/bottom-padding clicks snap to the
          // nearest in-text position (native-textarea feel) instead of jumping
          // to the end. Only a point that maps nowhere even when clamped (e.g.
          // an empty editor edge case) falls through to end-of-doc.
          let hit = view.posAtCoords({ left: clientX, top: clientY })
          if (!hit) {
            const rect = view.dom.getBoundingClientRect()
            const left = Math.min(
              Math.max(clientX, rect.left + 1),
              rect.right - 1
            )
            const top = Math.min(
              Math.max(clientY, rect.top + 1),
              rect.bottom - 1
            )
            hit = view.posAtCoords({ left, top })
          }
          if (hit) {
            editor.chain().focus().setTextSelection(hit.pos).run()
          } else {
            editor.commands.focus("end")
          }
        },
        isEmpty: () => editor?.isEmpty ?? true,
        getJSON: () => editor?.getJSON() ?? { type: "doc", content: [] },
        insertTextAtCursor: (text) => {
          // Insert literal text with `\n` → hardBreak so line breaks survive in
          // the plain-text schema. No Markdown parsing (and thus no schema-
          // rejection throw) is possible, so no recovery path is needed.
          editor?.chain().focus().insertContent(textToInlineContent(text)).run()
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
            // Remount per `@` session so panel state (active/pinned tab,
            // selection) never leaks when one suggestion exits and another
            // starts in the same React update (onExit + onStart batched).
            key={mentionState.range.from}
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
            tabLabels={tabLabels}
          />
        )}
      </div>
    )
  }
)
