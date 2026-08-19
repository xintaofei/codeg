"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { isImeCompositionKey } from "@/lib/ime-composition"
import { Button } from "@/components/ui/button"
import {
  BookOpenText,
  Check,
  ChevronUp,
  ClipboardPaste,
  Cog,
  Copy,
  GitFork,
  MessageSquareText,
  Scissors,
  Send,
  Square,
  TextSelect,
  X,
  Zap,
} from "lucide-react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import { AgentIcon } from "@/components/agent-icon"
import { cn, copyTextFromMenu } from "@/lib/utils"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { imageFilesFromClipboardApi } from "@/lib/clipboard-images"
import { toErrorMessage } from "@/lib/app-error"
import { isNoActiveTurnRejection } from "@/lib/turn-busy"
import { ServerFileBrowserDialog } from "@/components/shared/server-file-browser-dialog"
import { toast } from "sonner"
import type {
  AgentSkillItem,
  AgentType,
  AvailableCommandInfo,
  PromptCapabilitiesInfo,
  PromptDraft,
  PromptInputBlock,
  SessionConfigOptionInfo,
  SessionModeInfo,
} from "@/lib/types"
import {
  ATTACH_FILE_TO_SESSION_EVENT,
  ATTACH_SESSION_TO_SESSION_EVENT,
  APPEND_TEXT_TO_SESSION_EVENT,
  type AttachFileToSessionDetail,
  type AttachSessionToSessionDetail,
  type AppendTextToSessionDetail,
} from "@/lib/session-attachment-events"
import {
  ConversationContextBar,
  ConversationFolderBranchPicker,
  useConversationFolderBranchPickerVisible,
} from "@/components/chat/conversation-context-bar"
import { ComposerContextUsage } from "@/components/chat/composer-context-usage"
import { ComposerConnectionStatus } from "@/components/chat/composer-connection-status"
import { InlineModeSelector } from "@/components/chat/mode-selector"
import {
  InlineSessionConfigSelector,
  InlineSessionConfigToggle,
} from "@/components/chat/session-config-selector"
import { ModelOptionPicker } from "@/components/chat/model-option-picker"
import {
  SessionSelectorsPanel,
  type SessionSelectorGroup,
  type SessionSelectorSetting,
} from "@/components/chat/session-selectors-panel"
import {
  deriveModelGroups,
  isModelConfigOption,
  modelListGroups,
  MODEL_LIST_VIRTUALIZE_THRESHOLD,
  type ModelOptionGroup,
} from "@/lib/model-config-groups"
import { useAgentSkills } from "@/hooks/use-agent-skills"
import { useScrollbarSafeDismiss } from "@/hooks/use-scrollbar-safe-dismiss"
import {
  clearMessageInputDraftV2,
  loadMessageInputDraftV2,
  saveMessageInputDraftV2,
} from "@/lib/message-input-draft"
import { rankByTextMatch } from "@/lib/fuzzy-text-match"
import {
  RichComposer,
  type RichComposerHandle,
} from "@/components/chat/composer/rich-composer"
import {
  composerLeafText,
  docToPromptBlocks,
  serializeDocToDisplayText,
} from "@/components/chat/composer/to-prompt-blocks"
import { isEmbeddedReferenceUri } from "@/components/chat/composer/reference-uri"
import {
  applyExpertReference,
  isComposerChromeClick,
  isComposerEmpty,
  restampSkillPrefixes,
} from "@/components/chat/composer/composer-commands"
import {
  commandInvocationToken,
  commandToReference,
  skillToReference,
} from "@/components/chat/composer/invocation-reference"
import { cutSelectionToClipboard } from "@/components/chat/composer/clipboard-actions"
import { sessionToSuggestion } from "@/components/chat/composer/suggestion/adapters"
import { editorHasReference } from "@/components/chat/composer/attachment-files"
import type { ReferenceAttrs } from "@/components/chat/composer/types"
import type { Editor, JSONContent } from "@tiptap/core"
import { useReferenceSearch } from "@/components/chat/composer/use-reference-search"
import { useComposerMentionLabels } from "@/components/chat/composer/use-composer-mention-labels"
import { ComposerAddMenu } from "@/components/chat/composer/composer-add-menu"
import { ComposerImageThumbnails } from "@/components/chat/composer/composer-image-thumbnails"
import { useComposerAttachments } from "@/components/chat/composer/use-composer-attachments"
import { useComposerShortcuts } from "@/components/chat/composer/use-composer-shortcuts"

/**
 * Payload pushed into the composer from outside (e.g. a welcome-page quick
 * action). `text` replaces the document; `skill`, when present, is prepended as
 * the leading invocation badge (serializes to `${prefix}${id}` as the first
 * token).
 */
export interface ComposerInjectContent {
  text: string
  skill?: { id: string; label: string }
}

interface MessageInputProps {
  onSend: (draft: PromptDraft, modeId?: string | null) => void
  placeholder?: string
  defaultPath?: string
  disabled?: boolean
  autoFocus?: boolean
  onFocus?: () => void
  className?: string
  isPrompting?: boolean
  onCancel?: () => void
  modes?: SessionModeInfo[]
  configOptions?: SessionConfigOptionInfo[]
  modeLoading?: boolean
  configOptionsLoading?: boolean
  selectedModeId?: string | null
  onModeChange?: (modeId: string) => void
  onConfigOptionChange?: (configId: string, valueId: string) => void
  agentType?: AgentType | null
  availableCommands?: AvailableCommandInfo[] | null
  /**
   * The agent's command list is still on its way (the connection is being
   * established). The editor stays typable throughout, so `/` opens its panel
   * on a loading row instead of silently doing nothing, and fills in the moment
   * `availableCommands` lands.
   */
  commandsLoading?: boolean
  promptCapabilities: PromptCapabilitiesInfo
  attachmentTabId?: string | null
  draftStorageKey?: string | null
  isActive?: boolean
  /** Paint the flowing active-session gradient on the composer border. Set only
   *  for the active tab while tiled across multiple sessions; a lone or
   *  non-tiled session keeps the plain default border. Independent of
   *  `isActive` (which still drives auto-focus/connect). */
  showActiveFlow?: boolean
  onEnqueue?: (draft: PromptDraft, modeId: string | null) => void
  /** Id of the queue item being edited — the stable key for (re)hydration, so
   *  switching between two items with identical display text still reloads. */
  editingItemId?: string | null
  editingDraftText?: string | null
  /**
   * The queued message's full `draft.blocks`, when editing a queue item. Lets
   * the composer restore inline reference badges + attachments (not just text);
   * falls back to {@link editingDraftText} when absent.
   */
  editingDraftBlocks?: PromptInputBlock[] | null
  isEditingQueueItem?: boolean
  onSaveQueueEdit?: (draft: PromptDraft) => void
  onCancelQueueEdit?: () => void
  /** Fork the session and send `draft`. Fire-and-forget: the input consumes the
   *  draft synchronously (clears on click); the parent re-queues it if the fork
   *  can't run, so it is never lost. */
  onForkSend?: (draft: PromptDraft, modeId?: string | null) => void
  /** Inject the draft's TEXT into the RUNNING turn (native live-feedback
   *  steering). Present only on sessions whose feedback channel is native —
   *  when absent, the prompting branch renders its historical Stop-only form.
   *  Awaited: resolve = injected + recorded (clear the draft); reject =
   *  failure, where a turn-end `NoActiveTurn` race falls back to the queue
   *  and anything else keeps the draft. */
  onSteer?: (text: string) => Promise<void>
  /** Open the live-feedback dialog (from the "+" menu). When omitted the entry
   *  is hidden (feature off). */
  onAddFeedback?: () => void
  /** Grey out the live-feedback "+" entry when a note can't be sent right now
   *  (no active turn / agent lacks the tool). */
  feedbackAddDisabled?: boolean
  injectContent?: ComposerInjectContent | null
  onInjectConsumed?: () => void
}

// Non-image files attach as inline file badges in the editor (like `@`-file
// references), not as out-of-band chips. A file with a real `file://` path uses
// that uri directly (it serializes to a ResourceLink and round-trips through the
// draft doc untouched). A path-less file (a local-desktop paste/drop carrying
// inline bytes — an embedded resource or a `data:` link) can't live in the doc,
// so its badge carries an inert `codeg://embedded/<uuid>` display uri
// (`buildEmbeddedReferenceUri`) while the real bytes-bearing block is held in the
// `embeddedPayloadsRef` map keyed by that uri. `docToPromptBlocks` drops the
// embedded badge from the prose; `buildDraft` appends the mapped block for every
// embedded badge still in the document. The `codeg://` scheme is never a real
// path (no collision with a genuine attachment) and survives the transcript's
// sanitize/harden pipeline, so it renders as an inert file badge, not a blocked
// link — see {@link buildEmbeddedReferenceUri} / {@link isEmbeddedReferenceUri}.

/** Drop embedded-attachment reference badges from a draft document before it is
 *  persisted: their bytes live only in the in-memory `embeddedPayloadsRef` map
 *  (never serialized into the draft), so a restored badge would send nothing.
 *  Identified purely by the unambiguous `codeg://embedded/…` display uri (no map
 *  needed) — a real `file://` attachment is never matched. Stripping at save
 *  keeps the live badge visible this session but matches the pre-existing
 *  behavior where out-of-band pasted bytes don't survive a draft round-trip. */
function stripEmbeddedReferences(doc: JSONContent): JSONContent {
  if (!doc.content) return doc
  const content: JSONContent[] = []
  for (const child of doc.content) {
    if (
      child.type === "reference" &&
      typeof child.attrs?.uri === "string" &&
      isEmbeddedReferenceUri(child.attrs.uri)
    ) {
      continue
    }
    content.push(stripEmbeddedReferences(child))
  }
  return { ...doc, content }
}

function SelectorLoadingChip({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground">
      <span className="h-1.5 w-1.5 rounded-full bg-primary animate-pulse" />
      <span>{label}</span>
    </div>
  )
}

// Groups for the searchable + virtualized model picker, or `null` when the
// option should keep the lightweight selectors. Only the MODEL option, and only
// when its list is long enough to jank, qualifies. Falls back to a single
// headerless group for a long flat (un-prefixed) list.
function modelPickerGroups(
  option: SessionConfigOptionInfo
): ModelOptionGroup[] | null {
  if (!isModelConfigOption(option)) return null
  if (option.kind.type !== "select") return null
  if (option.kind.options.length <= MODEL_LIST_VIRTUALIZE_THRESHOLD) return null
  // Preserve derived `provider/` groups, server-provided groups, or a flat list
  // (never silently flatten server groups — keeps wide/collapsed consistent).
  return modelListGroups(option)
}

export function MessageInput({
  onSend,
  placeholder,
  defaultPath,
  disabled = false,
  autoFocus = false,
  onFocus,
  className,
  isPrompting = false,
  onCancel,
  modes,
  configOptions,
  modeLoading = false,
  configOptionsLoading = false,
  selectedModeId,
  onModeChange,
  onConfigOptionChange,
  agentType,
  availableCommands,
  commandsLoading = false,
  promptCapabilities,
  attachmentTabId,
  draftStorageKey,
  isActive = false,
  showActiveFlow = false,
  onEnqueue,
  editingItemId,
  editingDraftText,
  editingDraftBlocks,
  isEditingQueueItem = false,
  onSaveQueueEdit,
  onCancelQueueEdit,
  onForkSend,
  onSteer,
  onAddFeedback,
  feedbackAddDisabled,
  injectContent,
  onInjectConsumed,
}: MessageInputProps) {
  const t = useTranslations("Folder.chat.messageInput")
  const tQueue = useTranslations("Folder.chat.messageQueue")
  // Kept as a separate binding from `t` so its call sites — exclusively
  // upload / attachment toasts — read as a single coherent group when
  // scanning the file. Same namespace, no extra runtime cost.
  const tAttach = useTranslations("Folder.chat.messageInput")
  // The `$` prefix autocomplete is Codex-only: Codex advertises very few
  // native slash commands, so we augment the dropdown with the agent's
  // skills read from disk. Other agents already surface their full command
  // set through ACP `availableCommands`, so injecting skills there would
  // be duplicate/extra UI noise — skip the skills fetch for them entirely.
  const skillAgentType = agentType === "codex" ? "codex" : null
  // Pass the working dir so we see both global skills and folder-scoped
  // project skills (e.g. `{folder}/.codex/skills`). Without this, users
  // only ever saw global skills in the `$` autocomplete.
  const availableSkills = useAgentSkills(skillAgentType, defaultPath ?? null)
  const skillPrefix = agentType === "codex" ? "$" : "/"
  const { shortcuts } = useShortcutSettings()
  const effectiveDraftStorageKey = draftStorageKey ?? null
  const resolvedPlaceholder = placeholder ?? t("askAnything")
  const editorRef = useRef<RichComposerHandle>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  // The editor owns the content now; this mirror of its empty state drives the
  // send button and `hasSendableContent`.
  const [composerEmpty, setComposerEmpty] = useState(true)
  // Flips true once the RichComposer's async (immediatelyRender:false) editor has
  // mounted, so the hydration effect can use the imperative handle.
  const [composerReady, setComposerReady] = useState(false)

  const syncComposerEmpty = useCallback(() => {
    const ed = editorRef.current?.getEditor()
    setComposerEmpty(ed ? isComposerEmpty(ed) : true)
  }, [])

  // Attachments (images → thumbnail strip, files → inline badges) and the "+"
  // menu's insertable shortcuts. Both are shared with the to-do task composers,
  // so paste/drop/pick and the skill/quick-message entries behave identically
  // wherever a prompt is written.
  const attach = useComposerAttachments({
    editorRef,
    containerRef,
    disabled,
    promptCapabilities,
    attachmentTabId,
    defaultPath,
    logLabel: "MessageInput",
  })
  const {
    attachments,
    embeddedPayloadsRef,
    clearAttachments,
    hasUploadingImage,
    hydrateFromBlocks,
    imagePromptBlocks,
  } = attach
  const menuShortcuts = useComposerShortcuts({
    editorRef,
    agentType: agentType ?? null,
    onAfterInsert: syncComposerEmpty,
    logLabel: "MessageInput",
  })

  // Collapsed (narrow) selectors live in a controlled Popover holding a
  // master–detail panel (`SessionSelectorsPanel`). It's controlled so a value
  // pick closes it explicitly — matching the prior cog menu, which also closed
  // on every selection.
  const [collapsedSelectorsOpen, setCollapsedSelectorsOpen] = useState(false)
  // Keep the collapsed settings popover open while dragging the (virtualized)
  // model list's native scrollbar — see `useScrollbarSafeDismiss`.
  const collapsedSelectorsGuard = useScrollbarSafeDismiss()
  // Whether the async Clipboard read API is usable here. It's absent in
  // non-secure web deployments served over HTTP/LAN (see installClipboardFallback
  // in lib/utils, which only shims writeText), so the composer's custom
  // right-click "Paste" can't work there. When false we keep the radix context
  // menu disabled and let the browser's native menu through — its Paste still
  // works over the editable text. Resolved on the client after mount so SSR and
  // the first client render agree (no hydration mismatch on the trigger).
  const [clipboardReadSupported, setClipboardReadSupported] = useState(false)
  // Snapshotted when the custom right-click menu opens: whether the editor holds
  // a non-empty selection, which gates the Cut/Copy items. Read from the editor's
  // ProseMirror state (not the DOM Selection) so it stays correct after the radix
  // menu takes focus.
  const [contextSelectionActive, setContextSelectionActive] = useState(false)
  const isPromptingRef = useRef(isPrompting)
  const hydratedRef = useRef(false)
  // Tracks the last queue-item id hydrated, so a re-edit of the *same* item
  // doesn't clobber the user's in-progress changes — keyed on id, not display
  // text (two attachment-only items share the text "Attached 1 attachment").
  const prevEditingItemIdRef = useRef<string | null>(null)
  // Bridge so the early `onChange` handler can call the editor-driven slash
  // detection that is defined further down (after the slash state).
  const detectSlashTriggerRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    isPromptingRef.current = isPrompting
  }, [isPrompting])

  useEffect(() => {
    // navigator.clipboard is undefined at runtime in non-secure contexts even
    // though the DOM types claim it is always present, so guard with typeof.
    setClipboardReadSupported(
      typeof navigator !== "undefined" &&
        typeof navigator.clipboard?.readText === "function"
    )
  }, [])

  // Localized group headings + panel chrome for the `@` mention panel.
  const { groupLabels: referenceGroupLabels, uiLabels: mentionUiLabels } =
    useComposerMentionLabels()

  // Live data sources for the unified `@` mention panel. Pre-warmed only while
  // this composer is the active one (`enabled`). Referentially stable.
  const referenceSearch = useReferenceSearch({
    defaultPath: defaultPath ?? null,
    enabled: isActive,
    labels: referenceGroupLabels,
  })

  // Debounced v2 draft persistence. We snapshot the Tiptap *document* (JSON, not
  // Markdown) ~300ms after the last change so inline reference badges survive a
  // reload — a Markdown round-trip would downgrade them to plain links.
  const draftSaveTimerRef = useRef<number | null>(null)
  const scheduleDraftSave = useCallback(() => {
    if (typeof window === "undefined") return
    if (!effectiveDraftStorageKey || isEditingQueueItem) return
    if (draftSaveTimerRef.current != null) {
      window.clearTimeout(draftSaveTimerRef.current)
    }
    draftSaveTimerRef.current = window.setTimeout(() => {
      draftSaveTimerRef.current = null
      const ed = editorRef.current
      if (!ed || !effectiveDraftStorageKey) return
      if (ed.isEmpty()) {
        clearMessageInputDraftV2(effectiveDraftStorageKey)
      } else {
        saveMessageInputDraftV2(
          effectiveDraftStorageKey,
          stripEmbeddedReferences(ed.getJSON())
        )
      }
    }, 300)
  }, [effectiveDraftStorageKey, isEditingQueueItem])

  useEffect(() => {
    return () => {
      if (draftSaveTimerRef.current != null && typeof window !== "undefined") {
        window.clearTimeout(draftSaveTimerRef.current)
      }
    }
  }, [])

  // One-time hydration once the editor is ready: a queue-edit payload, else a v2
  // draft document (or a legacy v1 Markdown draft migrated forward). Guarded so
  // it never re-runs and clobbers later user edits.
  useEffect(() => {
    if (!composerReady || hydratedRef.current) return
    hydratedRef.current = true
    if (!editorRef.current) return
    // Bookkeeping stays synchronous so the sibling re-hydrate effect below sees
    // the claimed item and doesn't double-hydrate; only the editor mutation is
    // deferred to the next frame. Restoring a draft/queue payload that contains
    // a reference badge inserts a React NodeView, which @tiptap/react renders
    // with a synchronous flushSync() — running that here in the effect body
    // trips React's "flushSync from inside a lifecycle method" warning.
    if (
      isEditingQueueItem &&
      (editingDraftBlocks != null || editingDraftText != null)
    ) {
      prevEditingItemIdRef.current = editingItemId ?? null
    }
    const raf = requestAnimationFrame(() => {
      const ed = editorRef.current
      if (!ed) return
      if (
        isEditingQueueItem &&
        (editingDraftBlocks != null || editingDraftText != null)
      ) {
        const editor = ed.getEditor()
        if (editingDraftBlocks && editingDraftBlocks.length > 0 && editor) {
          // Full fidelity: restore inline badges + images from the blocks.
          hydrateFromBlocks(editor, editingDraftBlocks)
        } else if (editingDraftText != null) {
          ed.setText(editingDraftText)
        }
      } else if (effectiveDraftStorageKey) {
        const loaded = loadMessageInputDraftV2(effectiveDraftStorageKey)
        if (loaded?.kind === "doc") {
          ed.setDoc(loaded.doc)
        } else if (loaded?.kind === "legacyMarkdown") {
          ed.setText(loaded.markdown)
        }
      }
      const editor = ed.getEditor()
      setComposerEmpty(editor ? isComposerEmpty(editor) : true)
    })
    return () => cancelAnimationFrame(raf)
  }, [
    composerReady,
    isEditingQueueItem,
    editingItemId,
    editingDraftText,
    editingDraftBlocks,
    effectiveDraftStorageKey,
    hydrateFromBlocks,
  ])

  // Focus the composer the moment the editor exists and this tab is active, so
  // the caret lands as soon as the chat opens — without waiting for the ACP
  // connection to come up. The editor is always editable (RichComposer receives
  // no `disabled`; sends are gated in `handleSend`, not editability), so the old
  // `!disabled` gate only postponed the caret until "connected" for no real
  // reason. Deliberately NOT keyed on `disabled`: once focus lands on open, a
  // later connect (disabled → false) must never re-run this and yank focus back.
  // Keyed on `composerReady` because `immediatelyRender: false` builds the
  // editor a tick after mount (mirrors the hydration effect's gate). Ordered
  // after that hydration effect so this rAF runs after its setContent, landing
  // the caret at the end of a restored draft rather than before it.
  useEffect(() => {
    if (isActive && composerReady && !isPrompting) {
      requestAnimationFrame(() => {
        editorRef.current?.focus()
      })
    }
  }, [isActive, composerReady, isPrompting])

  // Re-hydrate when the user (re)edits a *different* queue item after the
  // initial mount hydration above. Keyed on the item id (not display text) so
  // switching between two items with identical text still reloads.
  useEffect(() => {
    if (
      isEditingQueueItem &&
      editingItemId != null &&
      editingItemId !== prevEditingItemIdRef.current
    ) {
      prevEditingItemIdRef.current = editingItemId
      // Same flushSync deferral as the hydration effect above: hydrateFromBlocks
      // can insert reference-badge NodeViews (synchronous @tiptap/react
      // flushSync). Mutation + focus run next frame, off the commit phase.
      const raf = requestAnimationFrame(() => {
        const editor = editorRef.current?.getEditor()
        if (editingDraftBlocks && editingDraftBlocks.length > 0 && editor) {
          hydrateFromBlocks(editor, editingDraftBlocks)
        } else if (editingDraftText != null) {
          editorRef.current?.setText(editingDraftText)
        }
        setComposerEmpty(editor ? isComposerEmpty(editor) : true)
        editorRef.current?.focus()
      })
      return () => cancelAnimationFrame(raf)
    } else if (!isEditingQueueItem) {
      prevEditingItemIdRef.current = null
    }
  }, [
    isEditingQueueItem,
    editingItemId,
    editingDraftText,
    editingDraftBlocks,
    hydrateFromBlocks,
  ])

  useEffect(() => {
    if (!injectContent || !composerReady) return
    const payload = injectContent
    // Defer the editor mutation to the next frame. Inserting the skill badge
    // creates a React NodeView, which @tiptap/react renders with a synchronous
    // flushSync(); doing that here in the effect body runs flushSync during
    // React's commit phase and trips the "flushSync was called from inside a
    // lifecycle method" warning. Scheduling it out of the commit phase is the
    // same rAF pattern the hydration effects above use. onInjectConsumed fires
    // inside the frame so the synchronous body never flips injectContent → null
    // and lets the cleanup cancel our own rAF before it runs.
    const raf = requestAnimationFrame(() => {
      const handle = editorRef.current
      if (handle) {
        handle.setText(payload.text)
        // Prepend the skill as the leading invocation badge, so the sent
        // message opens with `${prefix}${id}`.
        if (payload.skill) {
          const editor = handle.getEditor()
          if (editor) {
            applyExpertReference(editor, {
              refType: "skill",
              id: payload.skill.id,
              label: payload.skill.label,
              uri: null,
              meta: { invocationPrefix: skillPrefix, scope: "expert" },
            })
          }
        }
        setComposerEmpty(false)
        handle.focus()
      }
      onInjectConsumed?.()
    })
    return () => cancelAnimationFrame(raf)
  }, [injectContent, composerReady, skillPrefix, onInjectConsumed])

  // A skill / expert badge freezes its invocation prefix (`$` for Codex, `/`
  // elsewhere) at insert time. On the welcome page users routinely click a
  // quick-skill card while the default agent is selected and only then switch to
  // Codex via the picker below — the badge would keep its `/` and Codex would
  // parse the leading `/skill` as a slash command and reject the turn. Re-stamp
  // the existing skill badges whenever the effective prefix changes so the
  // leading invocation always matches the selected agent (ACP slash commands
  // carry no scope and stay `/`). rAF-deferred like the sibling editor-mutation
  // effects to stay off React's commit phase — the badge NodeView re-renders via
  // a synchronous flushSync().
  useEffect(() => {
    if (!composerReady) return
    const raf = requestAnimationFrame(() => {
      const editor = editorRef.current?.getEditor()
      if (editor) restampSkillPrefixes(editor, skillPrefix)
    })
    return () => cancelAnimationFrame(raf)
  }, [skillPrefix, composerReady])

  const handleComposerChange = useCallback(() => {
    syncComposerEmpty()
    scheduleDraftSave()
    detectSlashTriggerRef.current?.()
  }, [syncComposerEmpty, scheduleDraftSave])

  const handleComposerReady = useCallback(() => {
    setComposerReady(true)
  }, [])

  const availableModes = useMemo(() => modes ?? [], [modes])
  const availableConfigOptions = useMemo(
    () => configOptions ?? [],
    [configOptions]
  )
  const hasConfigOptions = availableConfigOptions.length > 0
  const hasModes = availableModes.length > 0

  const effectiveModeId = useMemo(() => {
    if (!hasModes) return null
    if (
      selectedModeId &&
      availableModes.some((mode) => mode.id === selectedModeId)
    ) {
      return selectedModeId
    }
    return availableModes[0]?.id ?? null
  }, [hasModes, selectedModeId, availableModes])
  const showModeSelector =
    hasModes && Boolean(effectiveModeId) && !hasConfigOptions
  const showModeLoading = modeLoading && !hasConfigOptions && !showModeSelector
  const showConfigLoading = configOptionsLoading && !hasConfigOptions
  const hasAnySelector =
    showConfigLoading || hasConfigOptions || showModeLoading || showModeSelector
  const hasInlineSelectors = hasConfigOptions || showModeSelector
  const hasFolderBranchPicker =
    useConversationFolderBranchPickerVisible(attachmentTabId)
  const folderBranchPickerAttached = hasFolderBranchPicker
  const imageAttachments = attach.imageAttachments
  const hasAttachments = attachments.length > 0
  const hasSendableContent = !composerEmpty || hasAttachments

  // ── Slash command autocomplete ──
  //
  // The slash list shows the agent's own `availableCommands` verbatim —
  // experts are advertised as commands and now appear here alongside the
  // rest. Codex additionally gets a `$`-triggered skills list (experts are
  // symlinked skills, so they surface there) because its native command set
  // is very small.
  const [slashMenuOpen, setSlashMenuOpen] = useState(false)
  const [slashSelectedIndex, setSlashSelectedIndex] = useState(0)
  // The trigger char (`/` for agent commands, `$` for Codex skills) and the
  // typed filter token, both derived from the editor caret by
  // `detectSlashTrigger` rather than from a raw string offset.
  const [slashTriggerChar, setSlashTriggerChar] = useState<"/" | "$" | null>(
    null
  )
  const [slashFilter, setSlashFilter] = useState("")
  const slashCommands = useMemo(
    () => availableCommands ?? [],
    [availableCommands]
  )
  const filteredSlashCommands = useMemo(() => {
    if (!slashMenuOpen || slashCommands.length === 0) return []
    if (slashTriggerChar !== "/") return []
    return rankByTextMatch(slashFilter, slashCommands, (cmd) => cmd.name)
  }, [slashMenuOpen, slashCommands, slashTriggerChar, slashFilter])
  const filteredSlashSkills = useMemo(() => {
    // Skills autocomplete is Codex-only and triggered by `$`.
    if (agentType !== "codex") return []
    if (!slashMenuOpen || availableSkills.length === 0) return []
    if (slashTriggerChar !== "$") return []
    return rankByTextMatch(
      slashFilter,
      availableSkills,
      (skill) => skill.name,
      (skill) => skill.id
    )
  }, [slashMenuOpen, availableSkills, agentType, slashTriggerChar, slashFilter])
  const slashAutocompleteCount =
    filteredSlashCommands.length + filteredSlashSkills.length
  // `/` is fed by the agent's own command list, which only exists once the
  // connection is up — so while it is being established the panel shows a
  // loading row rather than nothing at all. `$` (Codex skills) is read from
  // disk and never waits on a connection.
  const slashLoading =
    slashMenuOpen &&
    slashTriggerChar === "/" &&
    commandsLoading &&
    slashCommands.length === 0
  // Whether the panel is actually on screen — it renders for either rows or the
  // loading row, and that is also what makes it own the editor's nav keys.
  const slashMenuVisible =
    slashMenuOpen && (slashAutocompleteCount > 0 || slashLoading)

  // Keep the highlighted row inside the current result window. As the user
  // types and the filter narrows, the previously-highlighted index can point
  // past the end of the merged list (commands + experts), which would make
  // Enter/Tab a silent no-op. Clamp back to the last available row whenever
  // the count changes.
  useEffect(() => {
    if (
      slashAutocompleteCount > 0 &&
      slashSelectedIndex >= slashAutocompleteCount
    ) {
      setSlashSelectedIndex(slashAutocompleteCount - 1)
    }
  }, [slashAutocompleteCount, slashSelectedIndex])

  // Keep the highlighted row visible inside the popup when keyboard navigation
  // pushes it past the scroll viewport. Without this the cursor silently runs
  // off the rendered area when the filtered list overflows `max-h`.
  const slashMenuListRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    // Nothing to keep in view while the panel holds only its loading row.
    if (!slashMenuOpen || slashAutocompleteCount === 0) return
    const container = slashMenuListRef.current
    if (!container) return
    const el = container.children[slashSelectedIndex] as HTMLElement | undefined
    if (!el) return
    const elTop = el.offsetTop
    const elBottom = elTop + el.offsetHeight
    const viewTop = container.scrollTop
    const viewBottom = viewTop + container.clientHeight
    if (elTop < viewTop) {
      container.scrollTop = elTop
    } else if (elBottom > viewBottom) {
      container.scrollTop = elBottom - container.clientHeight
    }
  }, [slashMenuOpen, slashSelectedIndex, slashAutocompleteCount])

  // ── Editor-driven `/` (commands) and `$` (Codex skills) trigger detection ──
  // The `@` mention panel is now owned by RichComposer; this only handles the
  // runtime-command menus. We inspect the text before the collapsed caret in the
  // current block: a `/` (any agent) or `$` (Codex) at the start or right after
  // whitespace, and not inside inline code / a code block, opens the menu.
  const detectSlashTrigger = useCallback(() => {
    const editor = editorRef.current?.getEditor()
    const close = () => {
      setSlashMenuOpen(false)
      setSlashTriggerChar(null)
    }
    if (!editor) return close()
    const { selection } = editor.state
    if (!selection.empty) return close()
    if (editor.isActive("code") || editor.isActive("codeBlock")) return close()
    const { $from } = selection
    const before = $from.parent.textBetween(
      0,
      $from.parentOffset,
      undefined,
      " "
    )
    const regex =
      agentType === "codex" ? /(^|\s)([/$])(\S*)$/ : /(^|\s)(\/)(\S*)$/
    const match = before.match(regex)
    if (!match) return close()
    const trigger = match[2] as "/" | "$"
    // Only `/` is gated here. Its source is the agent's own command list, which
    // exists only once the connection is up — so an empty list means "nothing to
    // show" unless the connection is still coming, where the panel opens on a
    // loading row instead. `$` (the on-disk Codex skills) is deliberately
    // ungated: `useAgentSkills` reports an in-flight scan as an empty list, so
    // closing on empty would strand a `$` typed before the scan lands. Left
    // open, the panel simply stays hidden until the skills arrive and then
    // fills itself in — no second keystroke needed.
    if (trigger === "/" && slashCommands.length === 0 && !commandsLoading) {
      return close()
    }
    setSlashTriggerChar(trigger)
    setSlashFilter(match[3])
    setSlashSelectedIndex(0)
    setSlashMenuOpen(true)
  }, [slashCommands.length, commandsLoading, agentType])

  useEffect(() => {
    detectSlashTriggerRef.current = detectSlashTrigger
  }, [detectSlashTrigger])

  useEffect(() => {
    if (!showModeSelector) return
    if (!effectiveModeId || !onModeChange) return
    if (effectiveModeId !== selectedModeId) {
      onModeChange(effectiveModeId)
    }
  }, [showModeSelector, effectiveModeId, selectedModeId, onModeChange])

  const handleModeSelect = useCallback(
    (modeId: string) => {
      onModeChange?.(modeId)
    },
    [onModeChange]
  )

  // Close the runtime-command menu and clear the trigger.
  const closeSlashMenu = useCallback(() => {
    setSlashMenuOpen(false)
    setSlashTriggerChar(null)
  }, [])

  // Replace the live `/`-or-`$` token immediately before the caret with
  // an inline reference badge (+ a trailing space unless one already follows),
  // then close the menu. Used by both the command (`/`) and Codex-skill (`$`)
  // selections — the badge serializes back to its literal `/cmd` / `$skill`
  // token on send (see invocation-reference / referenceToMarkdown).
  const replaceTriggerWithReference = useCallback(
    (ref: ReferenceAttrs) => {
      const editor = editorRef.current?.getEditor()
      if (!editor) return
      const { $from } = editor.state.selection
      const before = $from.parent.textBetween(
        0,
        $from.parentOffset,
        undefined,
        " "
      )
      const match = before.match(/(^|\s)([/$])(\S*)$/)
      const charAfter =
        $from.parentOffset < $from.parent.content.size
          ? $from.parent.textBetween(
              $from.parentOffset,
              $from.parentOffset + 1,
              undefined,
              " "
            )
          : ""
      const suffix = charAfter && /\s/.test(charAfter) ? "" : " "
      let chain = editor.chain().focus()
      if (match) {
        // Remove the live `/…` / `$…` token before the caret.
        const tokenLen = match[2].length + match[3].length
        chain = chain.deleteRange({ from: $from.pos - tokenLen, to: $from.pos })
      }
      chain = chain.insertReference(ref)
      if (suffix) chain = chain.insertContent(suffix)
      chain.run()
      closeSlashMenu()
    },
    [closeSlashMenu]
  )

  const handleSlashSelect = useCallback(
    (cmd: AvailableCommandInfo) => {
      replaceTriggerWithReference(commandToReference(cmd))
    },
    [replaceTriggerWithReference]
  )

  // Codex uses `$<id>`, other agents `/<id>` — matching the trigger prefix.
  const handleSkillAutocompleteSelect = useCallback(
    (skill: AgentSkillItem) => {
      replaceTriggerWithReference(skillToReference(skill, skillPrefix))
    },
    [replaceTriggerWithReference, skillPrefix]
  )

  // Plain-text rendering of the editor's current selection, for the right-click
  // Cut/Copy. Read straight from ProseMirror state (stable while the radix menu
  // holds DOM focus). Uses the same leaf mapping as send serialization
  // (`composerLeafText`: reference badges → their inline token, hard breaks →
  // newlines) so a copied selection reads back exactly like what is sent.
  const selectionPlainText = useCallback((editor: Editor): string => {
    const { from, to } = editor.state.selection
    if (from >= to) return ""
    return editor.state.doc.textBetween(from, to, "\n", composerLeafText)
  }, [])

  // The radix menu traps focus until it closes, so the clipboard write is
  // deferred (see copyTextFromMenu) — otherwise the non-secure execCommand
  // fallback can't focus its scratch textarea. Copy never mutates the document,
  // so a failed write loses nothing; we still surface it (the native menu was
  // suppressed) so the user can fall back to the keyboard.
  const handleContextCopy = useCallback(async () => {
    const editor = editorRef.current?.getEditor()
    if (!editor) return
    const text = selectionPlainText(editor)
    if (!text) return
    if (!(await copyTextFromMenu(text))) {
      toast.error(t("clipboardWriteFailed"))
    }
  }, [selectionPlainText, t])

  const handleContextCut = useCallback(async () => {
    if (disabled) return
    const editor = editorRef.current?.getEditor()
    if (!editor) return
    // Capture the range up front so the post-write delete targets exactly what
    // was copied. Cut is atomic: the deferred clipboard write can fail in a
    // non-secure context, so the range is removed only once the write succeeds —
    // otherwise the selection is kept and the failure is surfaced (no data loss).
    const { from, to } = editor.state.selection
    await cutSelectionToClipboard({
      text: selectionPlainText(editor),
      copy: copyTextFromMenu,
      remove: () => editor.chain().focus().deleteRange({ from, to }).run(),
      onWriteFailed: () => toast.error(t("clipboardWriteFailed")),
    })
  }, [disabled, selectionPlainText, t])

  const handleContextSelectAll = useCallback(() => {
    if (disabled) return
    const editor = editorRef.current?.getEditor()
    if (!editor) return
    editor.chain().focus().selectAll().run()
  }, [disabled])

  // Opening the custom right-click menu: snapshot whether there's a selection
  // (gates Cut/Copy) and refresh the quick-messages list. The editor keeps its
  // selection while the menu is open, so Paste / a quick message lands back at
  // the same caret.
  const handleContextMenuOpenChange = useCallback(
    (open: boolean) => {
      if (!open) return
      const editor = editorRef.current?.getEditor()
      setContextSelectionActive(editor ? !editor.state.selection.empty : false)
      menuShortcuts.refreshQuickMessages()
    },
    [menuShortcuts]
  )

  // Plain-text ("paste without formatting") paste, shared by the custom
  // right-click menu item and the Ctrl/⌘+Shift+V shortcut. Reads only the
  // clipboard's `text/plain` and inserts it verbatim via `pasteText` (no
  // Markdown/HTML re-parsing), so it strips any formatting a keyboard Ctrl+V
  // would preserve. The native context menu only appears over the
  // contenteditable text, so the blank chrome had no paste affordance — this
  // reproduces the shortcut everywhere in the box. Reading the clipboard happens
  // inside the menu-click / keydown user gesture, so the async Clipboard API has
  // the activation it needs.
  const handleContextPaste = useCallback(async () => {
    if (disabled) return
    const editor = editorRef.current?.getEditor()
    if (!editor) return
    let text = ""
    // The async clipboard read can be blocked at call time even though the API
    // exists (denied permission, browser policy), so track that: with the native
    // menu (and its Paste) suppressed to show this one, a silent failure would
    // leave the user with no feedback and no fallback.
    let readBlocked = false
    try {
      text = (await navigator.clipboard.readText()) ?? ""
    } catch {
      // Permission denied / unsupported / no activation — fall through to the
      // image path (a textless clipboard may still hold a screenshot).
      readBlocked = true
      text = ""
    }
    if (text) {
      // Route through ProseMirror's own text paste so newlines, marks and the
      // editor's paste pipeline behave exactly like a keyboard paste.
      editor.view.focus()
      editor.view.pasteText(text)
      return
    }
    // No text — try a pasted image (screenshot), mirroring `handlePasteFiles`.
    try {
      const imageFiles = await imageFilesFromClipboardApi()
      if (imageFiles.length > 0) {
        await attach.appendFilesFromInput(imageFiles)
        return
      }
    } catch (error) {
      console.error("[MessageInput] context menu paste failed:", error)
      readBlocked = true
    }
    // Nothing landed. A blocked read leaves no visible result and no native menu
    // to retry from, so point the user at the keyboard shortcut. A merely empty
    // clipboard (read succeeded, returned "") stays a silent no-op as before.
    if (readBlocked) {
      toast.error(t("pasteUnavailable"))
    }
  }, [disabled, attach, t])

  // Bridges the composer's Ctrl/⌘+Shift+V key to the plain-text paste above.
  // Returns whether the shortcut was consumed: when disabled or when the async
  // clipboard read is available we take over (return true) so the composer
  // suppresses the browser's native rich paste; in a non-secure context (no
  // `readText`) we return false so the browser's own "paste and match style"
  // still works. The read runs inside this keydown gesture, so its activation
  // is preserved.
  const handlePlainPasteShortcut = useCallback((): boolean => {
    if (disabled) return true
    if (!clipboardReadSupported) return false
    void handleContextPaste()
    return true
  }, [disabled, clipboardReadSupported, handleContextPaste])

  useEffect(() => {
    if (!attachmentTabId) return

    const handleAttachFile = (event: Event) => {
      const customEvent = event as CustomEvent<AttachFileToSessionDetail>
      if (!customEvent.detail) return
      if (customEvent.detail.tabId !== attachmentTabId) return
      const { path, range } = customEvent.detail
      // Drop the badge at the composer's current caret rather than the end, so
      // "add to chat" / "add file to chat" land where the user left off.
      if (range) {
        attach.appendFileRangeAttachment(path, range, { atCaret: true })
      } else {
        attach.appendResourceAttachments([path], { atCaret: true })
      }
    }

    window.addEventListener(ATTACH_FILE_TO_SESSION_EVENT, handleAttachFile)
    return () => {
      window.removeEventListener(ATTACH_FILE_TO_SESSION_EVENT, handleAttachFile)
    }
  }, [attach, attachmentTabId])

  // Sidebar "add to session": drop a session mention badge — the very same
  // reference the `@` panel's Sessions group inserts — at the caret. Deduped by
  // uri like the file badges, so repeated menu clicks can't stack the same
  // `codeg://session/<id>` twice; the focus still lands in the composer either
  // way so the user sees where the badge went.
  useEffect(() => {
    if (!attachmentTabId) return

    const handleAttachSession = (event: Event) => {
      const customEvent = event as CustomEvent<AttachSessionToSessionDetail>
      if (!customEvent.detail) return
      if (customEvent.detail.tabId !== attachmentTabId) return
      const editor = editorRef.current?.getEditor()
      if (!editor) return
      const { reference } = sessionToSuggestion(customEvent.detail.conversation)
      if (
        reference.uri &&
        editorHasReference(editor, "session", reference.uri)
      ) {
        editor.commands.focus()
        return
      }
      editor.chain().focus().insertReference(reference).insertContent(" ").run()
    }

    window.addEventListener(
      ATTACH_SESSION_TO_SESSION_EVENT,
      handleAttachSession
    )
    return () => {
      window.removeEventListener(
        ATTACH_SESSION_TO_SESSION_EVENT,
        handleAttachSession
      )
    }
  }, [attachmentTabId])

  useEffect(() => {
    if (!attachmentTabId) return

    const handleAppendText = (event: Event) => {
      const customEvent = event as CustomEvent<AppendTextToSessionDetail>
      if (!customEvent.detail) return
      if (customEvent.detail.tabId !== attachmentTabId) return
      const appendText = customEvent.detail.text
      const editor = editorRef.current?.getEditor()
      if (!editor) return
      // Append at the very end, separated by a space when the document isn't
      // empty (and doesn't already end in whitespace).
      const ed = editorRef.current
      const needsSpace = ed != null && !ed.isEmpty()
      editor
        .chain()
        .focus("end")
        .insertContent(`${needsSpace ? " " : ""}${appendText}`)
        .run()
    }

    window.addEventListener(APPEND_TEXT_TO_SESSION_EVENT, handleAppendText)
    return () => {
      window.removeEventListener(APPEND_TEXT_TO_SESSION_EVENT, handleAppendText)
    }
  }, [attachmentTabId])

  const buildDraft = useCallback((): PromptDraft | null => {
    const editor = editorRef.current?.getEditor()
    // Authoritative prefix normalization at the send boundary. A skill / expert
    // badge freezes its `$`/`/` trigger at insert time and the agent can change
    // afterward; the agent-change effect re-stamps live, but doing it here too —
    // synchronously, before both the sent blocks and the display prose read the
    // doc — guarantees the wire text matches the current agent regardless of any
    // timing/ordering (Codex needs `$skill`, not the slash-command `/skill`).
    // Cheap: one small-doc walk, and no dispatch when nothing is stale.
    if (editor) restampSkillPrefixes(editor, skillPrefix)
    // Inline badges + prose → text/resource_link blocks (file mentions become
    // first-class ResourceLinks; agent/session/commit/skill stay inline text;
    // embedded badges are dropped here and re-added below from the payload map).
    const blocks: PromptInputBlock[] = editor ? docToPromptBlocks(editor) : []
    // Display/queue text is the SAME serialization as the sent prose, differing
    // only in that it KEEPS embedded-attachment badges inline (their real bytes
    // are appended below as a separate block; the send text drops the synthetic
    // uri, but the sender must still see the file they attached). For every other
    // kind of content it is byte-identical to what is sent, so the queue chip /
    // optimistic bubble can't diverge from the actual prose.
    const displayProse = editor
      ? serializeDocToDisplayText(editor.state.doc).trim()
      : ""
    // Append the real bytes-bearing block for every embedded-attachment badge
    // still present in the document, looked up by its `codeg://embedded/…` uri.
    // Walking the live doc (rather than a swap pass over a stored draft) means a
    // deleted badge's stale map entry is simply never emitted, and an undo that
    // resurrects a badge re-emits it — no pruning, and no orphan uri can leak.
    if (editor) {
      editor.state.doc.descendants((node) => {
        if (
          node.type.name === "reference" &&
          typeof node.attrs?.uri === "string" &&
          isEmbeddedReferenceUri(node.attrs.uri)
        ) {
          const real = embeddedPayloadsRef.current.get(node.attrs.uri)
          if (real) blocks.push(real)
        }
        return true
      })
    }
    if (blocks.length === 0 && attachments.length === 0) return null

    // `attachments` holds only images now — files live inline as badges above.
    // The wire encoding is capability-driven inside the hook (native `image`
    // block vs embedded `resource` blob), so an agent that advertises
    // `image: false` but `embedded_context: true` still receives the bytes it
    // accepts.
    blocks.push(...imagePromptBlocks())

    const displayText =
      displayProse ||
      `Attached ${attachments.length} attachment${attachments.length > 1 ? "s" : ""}`
    return { blocks, displayText }
  }, [attachments, skillPrefix, imagePromptBlocks, embeddedPayloadsRef])

  // Clear the editor + attachments after a send / enqueue / save.
  const resetComposer = useCallback(() => {
    editorRef.current?.clear()
    setComposerEmpty(true)
    clearAttachments()
    closeSlashMenu()
  }, [clearAttachments, closeSlashMenu])

  const handleSend = useCallback(() => {
    // The editor stays editable while `disabled` (the agent is busy) so the user
    // can keep typing, but a plain send is blocked — only enqueue / queue-edit
    // save go through. Mirrors the legacy textarea's keydown guard.
    if (disabled && !isPrompting && !isEditingQueueItem) return
    // An image whose web/remote upload hasn't settled has no server-side uri
    // yet — the transport would strip its base64 and the backend would have
    // nothing to hydrate. Block ALL three branches below (send / enqueue /
    // queue-edit save; a queued block is sent verbatim later) until uploads
    // finish. The draft stays intact, so this is a "wait a moment", not a loss.
    if (hasUploadingImage) {
      toast.error(tAttach("attachUploadInProgress"))
      return
    }
    const draft = buildDraft()
    if (!draft) return

    // Edit mode: save back to queue item
    if (isEditingQueueItem && onSaveQueueEdit) {
      onSaveQueueEdit(draft)
      resetComposer()
      return
    }

    // Prompting mode: enqueue instead of sending
    if (isPrompting && onEnqueue) {
      onEnqueue(draft, showModeSelector ? effectiveModeId : null)
      resetComposer()
      return
    }

    onSend(draft, showModeSelector ? effectiveModeId : null)
    if (effectiveDraftStorageKey) {
      clearMessageInputDraftV2(effectiveDraftStorageKey)
    }
    resetComposer()
  }, [
    disabled,
    hasUploadingImage,
    tAttach,
    buildDraft,
    isEditingQueueItem,
    isPrompting,
    onSaveQueueEdit,
    onEnqueue,
    onSend,
    effectiveModeId,
    showModeSelector,
    effectiveDraftStorageKey,
    resetComposer,
  ])

  const handleForkSendClick = useCallback(() => {
    if (!onForkSend) return
    // Same uploading gate as `handleSend`: a fork-send consumes the draft
    // (and its blocks) immediately, so an unsettled upload would strip to
    // nothing on the wire.
    if (hasUploadingImage) {
      toast.error(tAttach("attachUploadInProgress"))
      return
    }
    const draft = buildDraft()
    if (!draft) return
    // Fork-send consumes the draft synchronously, exactly like a normal send:
    // fire-and-forget and clear the input immediately, so there is no in-flight
    // editable window. If the fork can't run (queue non-empty / disconnected /
    // failure) the parent re-queues the draft, so it is never lost.
    onForkSend(draft, showModeSelector ? effectiveModeId : null)
    if (effectiveDraftStorageKey) {
      clearMessageInputDraftV2(effectiveDraftStorageKey)
    }
    resetComposer()
  }, [
    onForkSend,
    hasUploadingImage,
    tAttach,
    buildDraft,
    effectiveModeId,
    showModeSelector,
    effectiveDraftStorageKey,
    resetComposer,
  ])

  // Mid-turn "insert into current turn" (native steering). Awaited, unlike
  // the synchronous send/enqueue/fork paths: the draft clears ONLY once the
  // backend confirms the injection was recorded — a turn-end race falls back
  // to the queue (the note is never lost), any other failure keeps the draft
  // for retry. Steering is text-only: a draft carrying non-text blocks (file
  // badges) is queued whole instead of being silently stripped; image
  // attachments disable the menu entry at render (which also keeps unsettled
  // uploads out of this path — the enqueue fallback below bypasses
  // `handleSend`'s uploading gate).
  const [steering, setSteering] = useState(false)
  const handleSteerClick = useCallback(async () => {
    if (!onSteer || steering) return
    const draft = buildDraft()
    if (!draft) return
    const enqueueInstead = () => {
      if (!onEnqueue) return
      onEnqueue(draft, showModeSelector ? effectiveModeId : null)
      resetComposer()
      toast.info(t("steerQueuedInstead"))
    }
    if (draft.blocks.some((b) => b.type !== "text")) {
      enqueueInstead()
      return
    }
    const text = draft.blocks
      .map((b) => (b.type === "text" ? b.text : ""))
      .join("\n")
      .trim()
    if (!text) return
    setSteering(true)
    try {
      await onSteer(text)
      resetComposer()
    } catch (err) {
      if (isNoActiveTurnRejection(err)) {
        // The turn ended in the race window — reroute through the queue.
        enqueueInstead()
      } else {
        toast.error(t("steerFailed"), { description: toErrorMessage(err) })
      }
    } finally {
      setSteering(false)
    }
  }, [
    onSteer,
    steering,
    buildDraft,
    onEnqueue,
    showModeSelector,
    effectiveModeId,
    resetComposer,
    t,
  ])

  // Navigation/confirm/escape keys for the `/` (commands) and `$` (Codex skills)
  // runtime menu, routed from inside the editor (RichComposer.onExternalMenuKeyDown)
  // because ProseMirror's DOM handler fires before a host capture handler could.
  // Returns true for keys the menu consumed; false (e.g. a letter that filters)
  // lets normal editing proceed.
  const handleExternalMenuKeyDown = useCallback(
    (event: KeyboardEvent): boolean => {
      if (isImeCompositionKey(event)) return false
      if (!slashMenuVisible) return false
      if (event.key === "Escape") {
        closeSlashMenu()
        return true
      }
      if (slashAutocompleteCount === 0) {
        // Loading row: own the navigation/confirm keys so nothing submits or
        // moves the caret under an open panel, but insert nothing. Same
        // treatment the `@` panel gives its own stale state.
        return ["ArrowDown", "ArrowUp", "Enter", "Tab"].includes(event.key)
      }
      if (event.key === "ArrowDown") {
        setSlashSelectedIndex((i) =>
          i < slashAutocompleteCount - 1 ? i + 1 : 0
        )
        return true
      }
      if (event.key === "ArrowUp") {
        setSlashSelectedIndex((i) =>
          i > 0 ? i - 1 : slashAutocompleteCount - 1
        )
        return true
      }
      if (event.key === "Enter" || event.key === "Tab") {
        // The merged list is [commands, skills].
        if (slashSelectedIndex < filteredSlashCommands.length) {
          handleSlashSelect(filteredSlashCommands[slashSelectedIndex])
        } else {
          const skill =
            filteredSlashSkills[
              slashSelectedIndex - filteredSlashCommands.length
            ]
          if (skill) handleSkillAutocompleteSelect(skill)
        }
        return true
      }
      return false
    },
    [
      slashMenuVisible,
      slashAutocompleteCount,
      slashSelectedIndex,
      filteredSlashCommands,
      filteredSlashSkills,
      handleSlashSelect,
      handleSkillAutocompleteSelect,
      closeSlashMenu,
    ]
  )

  // Escape cancels a queue edit. ProseMirror doesn't consume Escape, so it
  // bubbles up to this container handler. Skipped while the slash menu is open
  // (the editor handles that Escape to close the menu first).
  const handleContainerKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (isImeCompositionKey(e)) return
      if (
        isEditingQueueItem &&
        e.key === "Escape" &&
        !slashMenuVisible &&
        onCancelQueueEdit
      ) {
        e.preventDefault()
        onCancelQueueEdit()
      }
    },
    [isEditingQueueItem, slashMenuVisible, onCancelQueueEdit]
  )

  // Clicking the input's empty chrome (its padding, the blank space below a
  // short message, the gaps in the action bar) focuses the editor — previously
  // only the editor surface itself was clickable. Interactive controls, inline
  // badges and the editor surface handle their own clicks, so they're excluded;
  // `preventDefault` keeps the editor from blurring before we refocus it. We
  // focus *at the click point* (not the end of the document) so clicking the
  // left/top padding next to existing text lands the caret there, like a native
  // textarea, instead of always jumping to the end.
  const handleChromeMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      // Not gated on `disabled`: the editor stays editable while connecting (see
      // `handleSend`), so chrome clicks must focus too — else only the existing
      // text line is clickable and the blank area below it is dead until ready.
      if (!isComposerChromeClick(e.target)) return
      // Keep the editor from blurring before we refocus it.
      e.preventDefault()
      editorRef.current?.focusAtCoords(e.clientX, e.clientY)
    },
    []
  )

  const hasImageAttachments = imageAttachments.length > 0
  const showDragActive = attach.isDragActive && !disabled

  const inlineSelectorItems = (
    <>
      {hasConfigOptions &&
        availableConfigOptions.map((option) => {
          // On/off options flip in place — a dropdown for a binary choice is a
          // wasted interaction.
          if (option.kind.type === "boolean") {
            return (
              <InlineSessionConfigToggle
                key={option.id}
                option={option}
                onLabel={t("toggleOn")}
                offLabel={t("toggleOff")}
                onSelect={(configId, value) =>
                  onConfigOptionChange?.(configId, value)
                }
              />
            )
          }
          // Long model lists get the searchable + virtualized popover (a Radix
          // menu of hundreds of items is the scroll jank); every other option —
          // and short model lists — keep the lightweight inline dropdown.
          const listGroups = modelPickerGroups(option)
          if (listGroups) {
            return (
              <ModelOptionPicker
                key={option.id}
                option={option}
                groups={listGroups}
                onSelect={(configId, valueId) =>
                  onConfigOptionChange?.(configId, valueId)
                }
              />
            )
          }
          return (
            <InlineSessionConfigSelector
              key={option.id}
              option={option}
              derivedGroups={deriveModelGroups(option)}
              onSelect={(configId, valueId) =>
                onConfigOptionChange?.(configId, valueId)
              }
            />
          )
        })}
      {showModeSelector && (
        <InlineModeSelector
          modes={availableModes}
          selectedModeId={effectiveModeId!}
          onSelect={handleModeSelect}
          label={t("modeLabel")}
        />
      )}
    </>
  )

  // Normalized settings for the collapsed (narrow) master–detail panel. Config
  // options and the mode picker are mutually exclusive in this UI (see
  // `showModeSelector`), but both are mapped so the panel stays agnostic.
  const collapsedSettings = useMemo<SessionSelectorSetting[]>(() => {
    const result: SessionSelectorSetting[] = []
    if (hasConfigOptions) {
      for (const option of availableConfigOptions) {
        // An on/off option becomes a two-item headerless group — the same shape
        // the mode picker below uses — so the panel needs no toggle affordance
        // of its own.
        if (option.kind.type === "boolean") {
          const checked = option.kind.current_value
          result.push({
            key: `config:${option.id}`,
            title: option.name,
            currentValue: checked ? "true" : "false",
            currentLabel: checked ? t("toggleOn") : t("toggleOff"),
            groups: [
              {
                key: "__boolean__",
                name: null,
                options: [
                  { value: "true", name: t("toggleOn"), description: null },
                  { value: "false", name: t("toggleOff"), description: null },
                ],
              },
            ],
            onSelect: (value) => onConfigOptionChange?.(option.id, value),
          })
          continue
        }
        if (option.kind.type !== "select") continue
        const kind = option.kind
        // Model values that carry a `provider/` prefix group by provider; every
        // other option keeps its server groups or stays flat (`null` derived).
        const derived = deriveModelGroups(option)
        const groups: SessionSelectorGroup[] = derived
          ? derived.map((group) => ({
              key: group.key,
              name: group.name,
              options: group.options.map((item) => ({
                value: item.value,
                name: item.name,
                description: item.description,
              })),
            }))
          : kind.groups.length > 0
            ? kind.groups.map((group) => ({
                key: group.group,
                name: group.name,
                options: group.options.map((item) => ({
                  value: item.value,
                  name: item.name,
                  description: item.description,
                })),
              }))
            : [
                {
                  key: "__flat__",
                  name: null,
                  options: kind.options.map((item) => ({
                    value: item.value,
                    name: item.name,
                    description: item.description,
                  })),
                },
              ]
        // Resolve the left-rail summary against the built groups so a grouped
        // model shows its prefix-stripped name (the provider is implied) rather
        // than repeating `provider/`.
        const current = groups
          .flatMap((group) => group.options)
          .find((item) => item.value === kind.current_value)
        // A long model list gets a searchable + virtualized detail pane (a plain
        // list of hundreds of buttons janks); short lists keep plain buttons.
        const searchable =
          isModelConfigOption(option) &&
          kind.options.length > MODEL_LIST_VIRTUALIZE_THRESHOLD
        result.push({
          key: `config:${option.id}`,
          title: option.name,
          currentValue: kind.current_value,
          currentLabel: current?.name ?? kind.current_value,
          groups,
          onSelect: (value) => onConfigOptionChange?.(option.id, value),
          ...(searchable && {
            search: {
              placeholder: t("searchModel"),
              inputLabel: t("searchModelAria"),
              listLabel: t("modelListLabel"),
              empty: t("noModels"),
            },
          }),
        })
      }
    }
    if (showModeSelector) {
      const selected = availableModes.find(
        (mode) => mode.id === effectiveModeId
      )
      result.push({
        key: "mode",
        title: t("modeLabel"),
        currentValue: effectiveModeId ?? "",
        currentLabel: selected?.name ?? effectiveModeId ?? "",
        groups: [
          {
            key: "__modes__",
            name: null,
            options: availableModes.map((mode) => ({
              value: mode.id,
              name: mode.name,
              description: mode.description,
            })),
          },
        ],
        onSelect: (value) => handleModeSelect(value),
      })
    }
    return result
  }, [
    hasConfigOptions,
    availableConfigOptions,
    showModeSelector,
    availableModes,
    effectiveModeId,
    onConfigOptionChange,
    handleModeSelect,
    t,
  ])

  const actionButtons = isEditingQueueItem ? (
    <div className="flex items-center gap-1">
      <Button
        onClick={onCancelQueueEdit}
        variant="ghost"
        size="icon"
        className="h-8 w-8"
        title={tQueue("cancelEdit")}
      >
        <X className="size-4" />
      </Button>
      <Button
        onClick={handleSend}
        disabled={!hasSendableContent}
        size="icon"
        className="h-8 w-8"
        title={tQueue("saveEdit")}
      >
        <Check className="size-4" />
      </Button>
    </div>
  ) : isPrompting && onCancel ? (
    onSteer && onEnqueue && hasSendableContent ? (
      // Native-steering sessions surface the mid-turn actions that already
      // exist but were keyboard-only/invisible: the primary half of the split
      // queues the draft (what Enter has always done here), the dropdown
      // injects it into the RUNNING turn. Without `onSteer` this branch stays
      // pixel-identical to the historical Stop-only form below.
      <div className="flex items-center gap-1">
        <Button
          onClick={onCancel}
          variant="destructive"
          size="icon"
          className="h-8 w-8"
          title={t("cancel")}
        >
          <Square className="size-4" />
        </Button>
        <div className="flex items-center">
          <Button
            onClick={handleSend}
            disabled={steering}
            size="icon"
            className="h-8 w-8 rounded-r-none"
            title={t("queueMessage")}
          >
            <Send className="size-4" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                disabled={steering}
                size="icon"
                className="h-8 w-5 rounded-l-none border-l border-primary-foreground/20"
                aria-label={t("steerIntoTurn")}
              >
                <ChevronUp className="size-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" side="top">
              <DropdownMenuItem
                onSelect={() => void handleSteerClick()}
                disabled={steering || attachments.length > 0}
                title={
                  attachments.length > 0
                    ? t("steerAttachmentsUnsupported")
                    : undefined
                }
              >
                <Zap className="h-4 w-4" />
                {t("steerIntoTurn")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    ) : (
      <Button
        onClick={onCancel}
        variant="destructive"
        size="icon"
        className="h-8 w-8"
        title={t("cancel")}
      >
        <Square className="size-4" />
      </Button>
    )
  ) : onForkSend ? (
    <div className="flex items-center">
      <Button
        onClick={handleSend}
        disabled={disabled || !hasSendableContent}
        size="icon"
        className="h-8 w-8 rounded-r-none"
        title={t("send")}
      >
        <Send className="size-4" />
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            disabled={disabled || !hasSendableContent}
            size="icon"
            className="h-8 w-5 rounded-l-none border-l border-primary-foreground/20"
            aria-label={t("forkAndSend")}
          >
            <ChevronUp className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" side="top">
          <DropdownMenuItem onSelect={handleForkSendClick}>
            <GitFork className="h-4 w-4" />
            {t("forkAndSend")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  ) : (
    <Button
      onClick={handleSend}
      disabled={disabled || !hasSendableContent}
      size="icon"
      className="h-8 w-8"
      title={t("send")}
    >
      <Send className="size-4" />
    </Button>
  )

  return (
    <div
      ref={containerRef}
      className="relative"
      // Marks this composer as a file-tree drop zone. On desktop Tauri's webview
      // swallows the HTML5 `drop`, so a dragged entry is committed from Tauri's
      // native drag-drop event by hit-testing the drop point; this attribute
      // lets that hit-test route the drop to this session's input (see the tree
      // tab's desktop commit). Absent when there's no tab to attach to.
      data-tree-drop-composer={attachmentTabId ?? undefined}
      onKeyDown={handleContainerKeyDown}
      {...attach.containerDragProps}
    >
      {slashMenuVisible && (
        <div
          data-testid="slash-menu"
          className="absolute bottom-full left-0 right-0 mb-1 z-50 flex max-h-[min(16rem,40dvh)] flex-col overflow-hidden rounded-xl border border-border bg-popover shadow-lg"
        >
          {/* No search box: the user types the filter inline after `/` (like the
              `@` panel); navigation is routed from the editor's keydown. */}
          <div ref={slashMenuListRef} className="flex-1 overflow-y-auto p-1">
            {slashLoading && (
              // The connection is still coming up, so the agent hasn't named its
              // commands yet. Holding the panel open (instead of ignoring the
              // `/`) tells the user the list is on its way; the rows replace
              // this the moment `availableCommands` lands. The panel takes no
              // focus, so the wait is announced through a polite live region —
              // the same treatment the `@` panel gives its own loading state.
              <div role="status" aria-live="polite">
                <SelectorLoadingChip label={t("slashLoading")} />
              </div>
            )}
            {filteredSlashCommands.map((cmd, i) => (
              <button
                key={`cmd-${cmd.name}`}
                type="button"
                className={cn(
                  "flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-sm",
                  i === slashSelectedIndex
                    ? "bg-accent text-accent-foreground"
                    : "hover:bg-muted"
                )}
                onMouseDown={(e) => {
                  e.preventDefault()
                  handleSlashSelect(cmd)
                }}
              >
                <span className="shrink-0 font-mono text-primary">
                  {commandInvocationToken(cmd.name)}
                </span>
                <span className="truncate text-xs text-muted-foreground">
                  {cmd.description}
                </span>
              </button>
            ))}
            {filteredSlashSkills.map((skill, i) => {
              const absoluteIndex = filteredSlashCommands.length + i
              return (
                <button
                  key={`skill-${skill.scope}-${skill.id}`}
                  type="button"
                  className={cn(
                    "flex w-full items-start gap-2 rounded-lg px-3 py-2 text-left text-sm",
                    absoluteIndex === slashSelectedIndex
                      ? "bg-accent text-accent-foreground"
                      : "hover:bg-muted"
                  )}
                  onMouseDown={(e) => {
                    e.preventDefault()
                    handleSkillAutocompleteSelect(skill)
                  }}
                >
                  <BookOpenText className="mt-0.5 size-4 shrink-0 text-primary/80" />
                  <div className="flex min-w-0 flex-1 items-center gap-2">
                    <span className="shrink-0 font-medium">{skill.name}</span>
                    <span
                      className="min-w-0 flex-1 truncate text-xs text-muted-foreground"
                      title={skill.description ?? undefined}
                    >
                      {skill.description ?? `${skillPrefix}${skill.id}`}
                    </span>
                  </div>
                </button>
              )
            })}
          </div>
        </div>
      )}
      {/* When the folder/branch row is attached below the composer, this group
          clips both into one rounded box (`overflow-hidden rounded-xl`); the
          drag-active ring rides the wrapper so it isn't clipped. Standalone
          (no row) it's layout-neutral (`display:contents`). */}
      <div
        className={cn(
          folderBranchPickerAttached
            ? "overflow-hidden rounded-xl transition-colors"
            : "contents",
          folderBranchPickerAttached &&
            showDragActive &&
            "ring-1 ring-primary/40"
        )}
      >
        <ContextMenu onOpenChange={handleContextMenuOpenChange}>
          {/* Disabled in non-secure web (no async clipboard read) so the native
              context menu — whose Paste still works over the editor text — is
              not suppressed. Desktop/secure-web get the full custom menu. */}
          <ContextMenuTrigger asChild disabled={!clipboardReadSupported}>
            <div
              onMouseDown={handleChromeMouseDown}
              className={cn(
                // `codeg-composer-chrome` paints the text I-beam across the box's
                // blank areas (padding, the dead space below a short message, the
                // action-bar gaps) so the whole input reads as clickable-to-type;
                // interactive controls re-assert their own cursor (see globals.css).
                // Resting border uses `border-foreground/20` (a touch darker than
                // the default `border-input`, which is near-invisible at rest and
                // vanishes over a workspace background image); it adapts per theme
                // (dark ink in light mode, light ink in dark) and stays legible.
                // Focus still swaps to `border-ring` below.
                "codeg-composer-chrome @container relative flex flex-col rounded-xl border border-foreground/20 bg-transparent transition-colors",
                // Standard focus ring — always shown when the composer is
                // focused (the plain default input style). `bg-background
                // ws-transparent-bg`: opaque surface normally, but with a
                // workspace-bg image the composer goes transparent to reveal the
                // real image like the rest of the canvas (no frosted treatment) —
                // the border stays. Off (no image) it's the plain background,
                // unchanged. When the folder/branch row is attached below, the
                // solid surface + an INSET focus ring live here so the shared
                // rounded box (clipped by the wrapper) reads as one control and
                // the ring isn't clipped away.
                folderBranchPickerAttached
                  ? "bg-background ws-transparent-bg focus-within:border-ring focus-within:ring-[3px] focus-within:ring-inset focus-within:ring-ring/50"
                  : "focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50",
                // Active session, tiled across multiple sessions: a gradient
                // flows around the border to mark which tile is active — but ONLY
                // while the composer itself is not focused. Focusing it hides the
                // flow (globals.css) so the default focus ring above takes over.
                // A lone/non-tiled session (showActiveFlow=false) and inactive
                // tiles show the plain default border.
                showActiveFlow && "codeg-composer-flow",
                !folderBranchPickerAttached &&
                  showDragActive &&
                  "ring-1 ring-primary/40",
                className
              )}
            >
              <ConversationContextBar
                hasExtraContent={hasImageAttachments}
                scrollEndTrigger={attachments.length}
                extraContent={
                  <ComposerImageThumbnails
                    attachments={imageAttachments}
                    onRemove={attach.removeAttachment}
                  />
                }
              />
              <RichComposer
                ref={editorRef}
                placeholder={resolvedPlaceholder}
                ariaLabel={resolvedPlaceholder}
                autoFocus={autoFocus}
                referenceSearch={referenceSearch}
                mentionUiLabels={mentionUiLabels}
                tabLabels={referenceGroupLabels}
                // The `@` panel spans the whole composer and opens above it —
                // the same box the `/` menu hangs off (this container), so the
                // two read as one affordance.
                mentionAnchorRef={containerRef}
                onChange={handleComposerChange}
                onReady={handleComposerReady}
                onSubmit={handleSend}
                onFocus={onFocus}
                onPasteFiles={attach.handlePasteFiles}
                onDropFiles={attach.handleEditorDrop}
                onPlainPaste={handlePlainPasteShortcut}
                submitShortcut={shortcuts.send_message}
                newlineShortcut={shortcuts.newline_in_message}
                isExternalMenuOpen={slashMenuVisible}
                onExternalMenuKeyDown={handleExternalMenuKeyDown}
                className="min-h-0 flex-1"
              />
              <div className="flex shrink-0 items-end justify-between gap-1 px-2 pb-2">
                <div className="flex min-w-0 items-end gap-1">
                  <ComposerAddMenu
                    disabled={disabled}
                    attachments={attach}
                    shortcuts={menuShortcuts}
                    slashCommands={slashCommands}
                    onAddFeedback={onAddFeedback}
                    feedbackAddDisabled={feedbackAddDisabled}
                  />
                  {hasInlineSelectors && (
                    <div className="hidden min-w-0 items-end gap-1 @[30rem]:flex">
                      {inlineSelectorItems}
                    </div>
                  )}
                  {hasAnySelector && (
                    <div
                      className={cn(
                        "flex",
                        hasInlineSelectors && "@[30rem]:hidden"
                      )}
                    >
                      <Popover
                        open={collapsedSelectorsOpen}
                        onOpenChange={setCollapsedSelectorsOpen}
                      >
                        <PopoverTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon-xs"
                            className="shrink-0"
                            title={t("agentSettings")}
                            aria-label={t("agentSettings")}
                          >
                            {agentType ? (
                              <AgentIcon
                                agentType={agentType}
                                className="size-3"
                              />
                            ) : (
                              <Cog className="size-3" />
                            )}
                          </Button>
                        </PopoverTrigger>
                        <PopoverContent
                          ref={collapsedSelectorsGuard.contentRef}
                          side="top"
                          align="start"
                          aria-label={t("agentSettings")}
                          onPointerDownOutside={
                            collapsedSelectorsGuard.onPointerDownOutside
                          }
                          onFocusOutside={
                            collapsedSelectorsGuard.onFocusOutside
                          }
                          className="w-[22rem] max-w-[calc(100vw-1rem)] p-1"
                        >
                          {showConfigLoading && (
                            <SelectorLoadingChip label={t("loadingSettings")} />
                          )}
                          {showModeLoading && (
                            <SelectorLoadingChip label={t("loadingMode")} />
                          )}
                          {collapsedSettings.length > 0 && (
                            <SessionSelectorsPanel
                              settings={collapsedSettings}
                              settingsLabel={t("agentSettings")}
                              onAfterSelect={() =>
                                setCollapsedSelectorsOpen(false)
                              }
                            />
                          )}
                        </PopoverContent>
                      </Popover>
                    </div>
                  )}
                </div>
                <div className="shrink-0">{actionButtons}</div>
              </div>
              {showDragActive && (
                <div className="pointer-events-none absolute inset-1 z-20 flex items-center justify-center rounded-md border border-dashed border-primary/50 bg-background/80 text-xs text-muted-foreground">
                  {t("dropFilesToAttach")}
                </div>
              )}
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem
              disabled={disabled || !contextSelectionActive}
              onSelect={() => void handleContextCut()}
            >
              <Scissors className="size-4" />
              {t("cut")}
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!contextSelectionActive}
              onSelect={() => void handleContextCopy()}
            >
              <Copy className="size-4" />
              {t("copy")}
            </ContextMenuItem>
            <ContextMenuItem
              disabled={disabled}
              onSelect={() => {
                void handleContextPaste()
              }}
            >
              <ClipboardPaste className="size-4" />
              {t("pasteAsPlainText")}
            </ContextMenuItem>
            <ContextMenuItem
              disabled={disabled}
              onSelect={() => handleContextSelectAll()}
            >
              <TextSelect className="size-4" />
              {t("selectAll")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuSub>
              <ContextMenuSubTrigger disabled={disabled}>
                <MessageSquareText className="size-4" />
                {t("quickMessages")}
              </ContextMenuSubTrigger>
              <ContextMenuSubContent
                className="min-w-40 overflow-y-auto"
                style={{
                  maxWidth: "min(20rem, calc(100vw - 1rem))",
                  maxHeight:
                    "min(32rem, var(--radix-context-menu-content-available-height))",
                }}
              >
                {menuShortcuts.quickMessagesLoading &&
                menuShortcuts.quickMessages.length === 0 ? (
                  <div className="px-3 py-4 text-center text-xs text-muted-foreground">
                    {t("quickMessagesLoading")}
                  </div>
                ) : menuShortcuts.quickMessages.length === 0 ? (
                  <div className="px-3 py-4 text-center text-xs text-muted-foreground">
                    {t("quickMessagesEmpty")}
                  </div>
                ) : (
                  menuShortcuts.quickMessages.map((message) => (
                    <ContextMenuItem
                      key={message.id}
                      onSelect={() => menuShortcuts.insertQuickMessage(message)}
                    >
                      <span className="truncate">
                        {message.title || (
                          <span className="italic text-muted-foreground">
                            {t("quickMessageUntitled")}
                          </span>
                        )}
                      </span>
                    </ContextMenuItem>
                  ))
                )}
              </ContextMenuSubContent>
            </ContextMenuSub>
          </ContextMenuContent>
        </ContextMenu>
        {hasFolderBranchPicker && (
          // `px-2` mirrors the action bar so this row lines up with the composer
          // above; the folder icon then aligns with the centered "+" icon (both
          // add the same 1px transparent border, paired with the picker buttons'
          // `px-1.5`). The row only renders while attached below the composer, so
          // it always takes the rounded-bottom box treatment. Pickers sit at the
          // left edge; the context-usage circle + agent connection status
          // right-align at the trailing edge.
          <div className="flex items-center justify-between gap-2 rounded-b-xl px-2 pt-1 text-xs text-muted-foreground">
            <div className="flex min-w-0 items-center gap-1">
              <ConversationFolderBranchPicker tabId={attachmentTabId} />
            </div>
            {/* `pr-px` offsets the composer chrome's 1px border: the send button
                sits INSIDE that border while this status row sits outside it, so
                without the 1px nudge the trailing icon hangs 1px past the button.
                With it, the connection icon's RIGHT edge is flush (0px) with the
                send button's right edge in the action bar above — no centring
                slot, which would inset the narrow icon and break the alignment. */}
            <div className="flex shrink-0 items-center gap-3 pr-px">
              <ComposerContextUsage tabId={attachmentTabId ?? null} />
              <ComposerConnectionStatus tabId={attachmentTabId ?? null} />
            </div>
          </div>
        )}
      </div>
      {!attach.showNativePaperclip && (
        <ServerFileBrowserDialog
          open={attach.serverFilePickerOpen}
          onOpenChange={attach.setServerFilePickerOpen}
          onSelect={attach.handleServerFilesSelected}
          initialPath={defaultPath ?? undefined}
        />
      )}
    </div>
  )
}
