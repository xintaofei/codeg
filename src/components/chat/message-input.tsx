"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { isDesktop } from "@/lib/platform"
import Image from "next/image"
import { useLocale, useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import {
  BookOpenText,
  Check,
  ChevronUp,
  ClipboardPaste,
  Cog,
  Copy,
  FileStack,
  FlaskConical,
  FolderSearch,
  GitFork,
  Lock,
  MessageSquarePlus,
  MessageSquareText,
  Paperclip,
  Plus,
  Scissors,
  Search,
  Send,
  Command,
  Sparkles,
  Square,
  TextSelect,
  Upload,
  X,
} from "lucide-react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
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
import { ImagePreviewDialog } from "@/components/ui/image-preview-dialog"
import { AgentIcon } from "@/components/agent-icon"
import { cn, copyTextFromMenu, randomUUID } from "@/lib/utils"
import {
  buildFileUri,
  buildFileUriWithRange,
  formatFileRangeLabel,
} from "@/lib/reference-link"
import {
  hasFileTreeDragType,
  readFileTreeDragPayload,
} from "@/lib/file-tree-dnd"
import {
  filesFromClipboard,
  clipboardHasText,
  imageFilesFromClipboardApi,
} from "@/lib/clipboard-images"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import {
  readFileBase64,
  quickMessagesList,
  uploadAttachment,
  uploadLocalPathToRemote,
  isEmptyAttachmentError,
  openSettingsWindow,
  type SettingsSection,
  UPLOAD_MAX_BYTES,
  UPLOAD_I18N_KEY_TOO_LARGE,
  UPLOAD_I18N_KEY_NOT_A_FILE,
  UPLOAD_I18N_KEY_QUOTA_EXCEEDED,
} from "@/lib/api"
import { extractAppCommandError } from "@/lib/app-error"
import { openFileDialog } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { ServerFileBrowserDialog } from "@/components/shared/server-file-browser-dialog"
import { toast } from "sonner"
import { disposeTauriListener } from "@/lib/tauri-listener"
import { AGENT_LABELS } from "@/lib/types"
import type {
  AgentSkillItem,
  AgentType,
  AvailableCommandInfo,
  ExpertListItem,
  ScienceListItem,
  PromptCapabilitiesInfo,
  PromptDraft,
  PromptInputBlock,
  QuickMessage,
  SessionConfigOptionInfo,
  SessionModeInfo,
} from "@/lib/types"
import {
  ATTACH_FILE_TO_SESSION_EVENT,
  APPEND_TEXT_TO_SESSION_EVENT,
  type AttachFileToSessionDetail,
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
import { InlineSessionConfigSelector } from "@/components/chat/session-config-selector"
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
import { DropdownRadioItemContent } from "@/components/chat/dropdown-radio-item-content"
import { useAgentSkills } from "@/hooks/use-agent-skills"
import { useBuiltInExperts } from "@/hooks/use-built-in-experts"
import { useBuiltInScience } from "@/hooks/use-built-in-science"
import { useEnabledSkillIds } from "@/hooks/use-enabled-skill-ids"
import { useScrollbarSafeDismiss } from "@/hooks/use-scrollbar-safe-dismiss"
import { getExpertIcon, pickLocalized } from "@/lib/expert-presentation"
import { getScienceIcon } from "@/lib/science-presentation"
import { OFFICE_ACTIONS, type OfficeAction } from "@/lib/office-actions"
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
import {
  buildEmbeddedReferenceUri,
  isEmbeddedReferenceUri,
} from "@/components/chat/composer/reference-uri"
import {
  applyExpertReference,
  isComposerChromeClick,
  isComposerEmpty,
  restampSkillPrefixes,
  restoreBlocksIntoEditor,
} from "@/components/chat/composer/composer-commands"
import {
  commandToReference,
  skillToReference,
} from "@/components/chat/composer/invocation-reference"
import { cutSelectionToClipboard } from "@/components/chat/composer/clipboard-actions"
import type { ReferenceAttrs } from "@/components/chat/composer/types"
import type { Editor, JSONContent } from "@tiptap/core"
import {
  useReferenceSearch,
  type ReferenceGroupLabels,
} from "@/components/chat/composer/use-reference-search"
import type { MentionUiLabels } from "@/components/chat/composer/suggestion/types"
import {
  imageAttachmentToPromptBlock,
  type ImageInputAttachment,
  type InputAttachment,
  type ResourceInputAttachment,
} from "./message-input-attachments"

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
  /** Open the live-feedback dialog (from the "+" menu). When omitted the entry
   *  is hidden (feature off). */
  onAddFeedback?: () => void
  /** Grey out the live-feedback "+" entry when a note can't be sent right now
   *  (no active turn / agent lacks the tool). */
  feedbackAddDisabled?: boolean
  injectContent?: ComposerInjectContent | null
  onInjectConsumed?: () => void
}

const MIME_BY_EXT: Record<string, string> = {
  txt: "text/plain",
  md: "text/markdown",
  json: "application/json",
  yaml: "application/yaml",
  yml: "application/yaml",
  csv: "text/csv",
  html: "text/html",
  css: "text/css",
  js: "text/javascript",
  mjs: "text/javascript",
  cjs: "text/javascript",
  ts: "text/typescript",
  tsx: "text/tsx",
  jsx: "text/jsx",
  py: "text/x-python",
  rs: "text/rust",
  go: "text/x-go",
  java: "text/x-java-source",
  xml: "application/xml",
  toml: "application/toml",
  pdf: "application/pdf",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
}

function fileNameFromPath(path: string): string {
  return path.split(/[/\\]/).pop() || path
}

function mimeTypeFromPath(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? ""
  return MIME_BY_EXT[ext] ?? null
}

function hasDragFiles(dataTransfer: DataTransfer | null): boolean {
  if (!dataTransfer?.types) return false
  return Array.from(dataTransfer.types).includes("Files")
}

function pointWithinElement(
  position: { x: number; y: number },
  element: HTMLElement
): boolean {
  // Inactive conversation tabs are kept mounted at `absolute inset-0` with
  // `visibility: hidden` (see ConversationDetailPanel), so their bounding rect
  // overlaps the active tab's. Without this guard every tab's Tauri drag
  // listener would treat the same OS drop as falling inside its own input,
  // and dropped files would silently fan out across every open conversation.
  const style = element.ownerDocument?.defaultView?.getComputedStyle(element)
  if (style) {
    if (
      style.visibility === "hidden" ||
      style.display === "none" ||
      style.pointerEvents === "none"
    ) {
      return false
    }
  }
  const rect = element.getBoundingClientRect()
  if (rect.width === 0 || rect.height === 0) return false
  const dpr = window.devicePixelRatio || 1
  const candidates = [
    { x: position.x, y: position.y },
    { x: position.x / dpr, y: position.y / dpr },
  ]
  return candidates.some(
    (point) =>
      point.x >= rect.left &&
      point.x <= rect.right &&
      point.y >= rect.top &&
      point.y <= rect.bottom
  )
}

function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => {
      reject(reader.error ?? new Error("Failed to read blob"))
    }
    reader.onload = () => {
      if (typeof reader.result !== "string") {
        reject(new Error("Unexpected non-string blob reader result"))
        return
      }
      const markerIndex = reader.result.indexOf(",")
      resolve(
        markerIndex >= 0 ? reader.result.slice(markerIndex + 1) : reader.result
      )
    }
    reader.readAsDataURL(blob)
  })
}

function getFilePath(file: File): string | null {
  const withPath = file as File & { path?: string; webkitRelativePath?: string }
  if (typeof withPath.path === "string" && withPath.path.trim().length > 0) {
    return withPath.path
  }
  if (
    typeof withPath.webkitRelativePath === "string" &&
    withPath.webkitRelativePath.trim().length > 0
  ) {
    return withPath.webkitRelativePath
  }
  return null
}

const TEXT_LIKE_MIME_PREFIXES = [
  "text/",
  "application/json",
  "application/xml",
  "application/yaml",
  "application/x-yaml",
  "application/toml",
  "application/javascript",
  "application/typescript",
]
const DRAG_DROP_IMAGE_MAX_BYTES = 20_000_000

function isTextLikeFile(file: File): boolean {
  const mime = file.type.toLowerCase()
  if (mime) {
    if (TEXT_LIKE_MIME_PREFIXES.some((prefix) => mime.startsWith(prefix))) {
      return true
    }
  }
  const ext = file.name.split(".").pop()?.toLowerCase()
  if (!ext) return false
  return Boolean(
    MIME_BY_EXT[ext]?.startsWith("text/") ||
    ["json", "yaml", "yml", "xml", "toml", "md", "csv"].includes(ext)
  )
}

function buildClipboardResourceUri(name: string): string {
  const normalizedName = name.trim() || "clipboard-resource"
  return `clipboard://${encodeURIComponent(normalizedName)}-${randomUUID()}`
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

/** Whether the document already holds a file reference badge for `uri` (used to
 *  dedupe repeated drops/picks of the same path, mirroring the old seen-set). */
function editorHasFileReference(editor: Editor, uri: string): boolean {
  let found = false
  editor.state.doc.descendants((node) => {
    if (found) return false
    if (
      node.type.name === "reference" &&
      node.attrs?.refType === "file" &&
      node.attrs?.uri === uri
    ) {
      found = true
      return false
    }
    return true
  })
  return found
}

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

function buildDataUri(base64Data: string, mimeType: string | null): string {
  const safeMime =
    mimeType && mimeType.trim() ? mimeType : "application/octet-stream"
  return `data:${safeMime};base64,${base64Data}`
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
  const desktopMode = isDesktop()
  // Cached for the window's lifetime: `getActiveRemoteConnectionId()` is
  // configured once when a remote-workspace window is created and never
  // mutates afterwards. A desktop window bound to a remote codeg-server
  // has to behave like the web client for attachments — local OS paths
  // would be ENOENT on the remote agent. Only the truly local desktop
  // shows the native Paperclip picker.
  const showNativePaperclip = useMemo(
    () => desktopMode && getActiveRemoteConnectionId() === null,
    [desktopMode]
  )
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
  // The "+" menu's expert / daily-office / research skill shortcuts mirror the
  // welcome-page quick actions: localized labels, the bundled experts and
  // science skills, and per-agent skill-enabled gating. Experts and science load
  // their full lists from the backend (so every skill shows, not a curated
  // subset); office is a fixed static set. `tQa` supplies office labels/prompts.
  const locale = useLocale()
  const tQa = useTranslations("Folder.chat.welcomePanel.quickActions")
  const experts = useBuiltInExperts()
  const science = useBuiltInScience()
  const {
    enabledIds,
    ready: skillStatusReady,
    supported: skillManagementSupported,
  } = useEnabledSkillIds(agentType ?? null)
  const editorRef = useRef<RichComposerHandle>(null)
  // The editor owns the content now; this mirror of its empty state drives the
  // send button and `hasSendableContent`.
  const [composerEmpty, setComposerEmpty] = useState(true)
  // Flips true once the RichComposer's async (immediatelyRender:false) editor has
  // mounted, so the hydration effect can use the imperative handle.
  const [composerReady, setComposerReady] = useState(false)
  // `attachments` now holds only images; non-image files live inline as editor
  // reference badges. This map carries the real bytes-bearing block for each
  // embedded/data-uri badge, keyed by its synthetic `file://` sentinel uri, and
  // is reconciled into the outgoing blocks by `buildDraft`.
  const [attachments, setAttachments] = useState<InputAttachment[]>([])
  const embeddedPayloadsRef = useRef<Map<string, PromptInputBlock>>(new Map())
  const [isDragActive, setIsDragActive] = useState(false)
  // Collapsed (narrow) selectors live in a controlled Popover holding a
  // master–detail panel (`SessionSelectorsPanel`). It's controlled so a value
  // pick closes it explicitly — matching the prior cog menu, which also closed
  // on every selection.
  const [collapsedSelectorsOpen, setCollapsedSelectorsOpen] = useState(false)
  // Keep the collapsed settings popover open while dragging the (virtualized)
  // model list's native scrollbar — see `useScrollbarSafeDismiss`.
  const collapsedSelectorsGuard = useScrollbarSafeDismiss()
  const [quickMessages, setQuickMessages] = useState<QuickMessage[]>([])
  const [quickMessagesLoading, setQuickMessagesLoading] = useState(false)
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
  const [previewAttachmentId, setPreviewAttachmentId] = useState<string | null>(
    null
  )
  const containerRef = useRef<HTMLDivElement>(null)
  const lastDomDropAtRef = useRef(0)
  const disabledRef = useRef(disabled)
  const isPromptingRef = useRef(isPrompting)
  const hydratedRef = useRef(false)
  // Tracks the last queue-item id hydrated, so a re-edit of the *same* item
  // doesn't clobber the user's in-progress changes — keyed on id, not display
  // text (two attachment-only items share the text "Attached 1 attachment").
  const prevEditingItemIdRef = useRef<string | null>(null)
  const dragActiveRef = useRef(false)
  // Bridge so the early `onChange` handler can call the editor-driven slash
  // detection that is defined further down (after the slash state).
  const detectSlashTriggerRef = useRef<(() => void) | null>(null)
  // Route pasted / dropped / picked images to the top thumbnail strip whenever
  // the agent can receive them in ANY form — either as a native ACP image block
  // (`image`) or as an embedded resource blob (`embedded_context`, e.g. Grok,
  // which advertises `image: false` but `embeddedContext: true`). Without the
  // `embedded_context` arm, Grok's images fell through to the generic
  // file-resource path and rendered as an inline badge instead of a thumbnail.
  // `buildDraft` still picks the wire encoding per-capability, so the sent
  // payload is unchanged for each agent — this only unifies the presentation.
  const canAttachImages =
    promptCapabilities.image || promptCapabilities.embedded_context

  useEffect(() => {
    disabledRef.current = disabled
  }, [disabled])

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
  const referenceGroupLabels = useMemo<ReferenceGroupLabels>(
    () => ({
      file: t("mentionGroupFile"),
      agent: t("mentionGroupAgent"),
      session: t("mentionGroupSession"),
      commit: t("mentionGroupCommit"),
      skill: t("mentionGroupSkill"),
    }),
    [t]
  )
  const mentionUiLabels = useMemo<MentionUiLabels>(
    () => ({
      empty: t("mentionEmpty"),
      loading: t("mentionLoading"),
      listbox: t("mentionListLabel"),
      more: t("mentionMore"),
      count: (count: number) => t("mentionCount", { count }),
    }),
    [t]
  )

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

  // Replay a sent `PromptInputBlock[]` (a queued message being re-edited) into
  // the editor: prose + file badges inline, images into `attachments`, and any
  // embedded/data-uri resources re-inlined as sentinel badges with their
  // bytes-bearing blocks re-registered in the payload map.
  const hydrateFromBlocks = useCallback(
    (editor: Editor, blocks: PromptInputBlock[]) => {
      embeddedPayloadsRef.current.clear()
      const restored = restoreBlocksIntoEditor(editor, blocks)
      setAttachments(
        restored.filter((a): a is ImageInputAttachment => a.type === "image")
      )
      const resources = restored.filter(
        (a): a is ResourceInputAttachment => a.type === "resource"
      )
      if (resources.length === 0) return
      let chain = editor.chain().focus("end")
      for (const att of resources) {
        const refUri = buildEmbeddedReferenceUri()
        const block: PromptInputBlock =
          att.kind === "embedded"
            ? {
                type: "resource",
                uri: att.uri,
                mime_type: att.mimeType,
                text: att.text ?? null,
                blob: att.blob ?? null,
              }
            : {
                type: "resource_link",
                uri: att.uri,
                name: att.name,
                mime_type: att.mimeType,
                description: null,
              }
        embeddedPayloadsRef.current.set(refUri, block)
        chain = chain
          .insertReference({
            refType: "file",
            id: refUri,
            label: att.name,
            uri: refUri,
            meta: { fileKind: "file" },
          })
          .insertContent(" ")
      }
      chain.run()
    },
    []
  )

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

  const setDragActiveIfChanged = useCallback((next: boolean) => {
    if (dragActiveRef.current === next) return
    dragActiveRef.current = next
    setIsDragActive(next)
  }, [])

  const syncComposerEmpty = useCallback(() => {
    const ed = editorRef.current?.getEditor()
    setComposerEmpty(ed ? isComposerEmpty(ed) : true)
  }, [])

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
  const imageAttachments = useMemo(
    () =>
      attachments.filter(
        (attachment): attachment is ImageInputAttachment =>
          attachment.type === "image"
      ),
    [attachments]
  )
  const previewAttachment = useMemo(
    () =>
      previewAttachmentId
        ? (imageAttachments.find((a) => a.id === previewAttachmentId) ?? null)
        : null,
    [previewAttachmentId, imageAttachments]
  )
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
  const [slashDropdownOpen, setSlashDropdownOpen] = useState(false)
  const [slashDropdownSearch, setSlashDropdownSearch] = useState("")
  const slashDropdownInputRef = useRef<HTMLInputElement>(null)
  const filteredSlashDropdownCommands = useMemo(
    () =>
      rankByTextMatch(
        slashDropdownSearch,
        slashCommands,
        (cmd) => cmd.name,
        (cmd) => cmd.description
      ),
    [slashCommands, slashDropdownSearch]
  )
  const handleSlashDropdownOpenChange = useCallback((open: boolean) => {
    setSlashDropdownOpen(open)
    if (!open) setSlashDropdownSearch("")
  }, [])
  // Radix's MenuSubContent hardcodes its own onOpenAutoFocus that overwrites
  // any prop we pass in (see @radix-ui/react-menu MenuSubContent). To put the
  // search input in focus when the slash submenu opens, defer focus to a
  // microtask after Radix finishes its own focus dance.
  useEffect(() => {
    if (!slashDropdownOpen) return
    const id = requestAnimationFrame(() => {
      slashDropdownInputRef.current?.focus()
    })
    return () => cancelAnimationFrame(id)
  }, [slashDropdownOpen])
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
    if (!slashMenuOpen) return
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
    const hasSlashSource =
      slashCommands.length > 0 || availableSkills.length > 0
    const close = () => {
      setSlashMenuOpen(false)
      setSlashTriggerChar(null)
    }
    if (!editor || !hasSlashSource) return close()
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
    setSlashTriggerChar(match[2] as "/" | "$")
    setSlashFilter(match[3])
    setSlashSelectedIndex(0)
    setSlashMenuOpen(true)
  }, [slashCommands.length, availableSkills.length, agentType])

  useEffect(() => {
    detectSlashTriggerRef.current = detectSlashTrigger
  }, [detectSlashTrigger])

  // Insert one inline file reference badge per item, matching `@`-file mentions.
  // A genuine `file://` item uses its uri directly (deduped against the document);
  // an item carrying a `realBlock` (embedded bytes / `data:` link) gets an inert
  // `codeg://embedded/…` display uri and its block is stashed in
  // `embeddedPayloadsRef` for send-time reconciliation. Badges append at the doc
  // end by default; pass `atCaret` to drop them at the composer's current caret
  // (`focus()` keeps the retained selection even while the input is blurred —
  // e.g. focus sits in the file editor), so "add to chat" lands a reference
  // where the user left off instead of always at the end.
  const insertFileReferences = useCallback(
    (
      items: Array<{
        name: string
        uri?: string
        realBlock?: PromptInputBlock
      }>,
      opts: { atCaret?: boolean } = {}
    ) => {
      if (items.length === 0) return
      const editor = editorRef.current?.getEditor()
      if (!editor) return
      const seen = new Set<string>()
      let chain = opts.atCaret
        ? editor.chain().focus()
        : editor.chain().focus("end")
      let inserted = 0
      for (const item of items) {
        let refUri: string
        if (item.realBlock) {
          refUri = buildEmbeddedReferenceUri()
          embeddedPayloadsRef.current.set(refUri, item.realBlock)
        } else {
          if (!item.uri) continue
          refUri = item.uri
          if (seen.has(refUri) || editorHasFileReference(editor, refUri))
            continue
          seen.add(refUri)
        }
        chain = chain
          .insertReference({
            refType: "file",
            id: refUri,
            label: item.name,
            uri: refUri,
            meta: { fileKind: "file" },
          })
          .insertContent(" ")
        inserted++
      }
      if (inserted > 0) chain.run()
    },
    []
  )

  const appendResourceLinks = useCallback(
    (
      links: Array<{
        uri: string
        name: string
        mimeType: string | null
        dedupeKey: string
      }>,
      opts: { atCaret?: boolean } = {}
    ) => {
      // `file://` links the agent can read directly become inline file badges
      // (uri used as-is); a non-fetchable `data:` link keeps its real block out
      // of band behind a sentinel badge.
      insertFileReferences(
        links
          .filter((link) => link.uri)
          .map((link) =>
            link.uri.toLowerCase().startsWith("file://")
              ? { name: link.name, uri: link.uri }
              : {
                  name: link.name,
                  realBlock: {
                    type: "resource_link" as const,
                    uri: link.uri,
                    name: link.name,
                    mime_type: link.mimeType,
                    description: null,
                  },
                }
          ),
        opts
      )
    },
    [insertFileReferences]
  )

  const appendResourceAttachments = useCallback(
    (paths: string[], opts: { atCaret?: boolean } = {}) => {
      const normalized = paths
        .filter(
          (path): path is string => typeof path === "string" && path.length > 0
        )
        .map((path) => {
          const uri = buildFileUri(path)
          return {
            uri,
            name: fileNameFromPath(path),
            mimeType: mimeTypeFromPath(path),
            dedupeKey: uri,
          }
        })
      appendResourceLinks(normalized, opts)
    },
    [appendResourceLinks]
  )

  // Attach a single file as a ranged badge (`foo.ts:10-25`), used by the file
  // editor's "add selection to chat". The line span is encoded into both the
  // label and the uri fragment (`file://…#L10-25`), so distinct selections of
  // the same file stay distinct (the uri is the dedupe key in
  // `insertFileReferences`) and the range rides along to the agent in the
  // serialized `[label](uri)` link.
  const appendFileRangeAttachment = useCallback(
    (
      path: string,
      range: { start: number; end: number },
      opts: { atCaret?: boolean } = {}
    ) => {
      if (!path) return
      insertFileReferences(
        [
          {
            name: formatFileRangeLabel(fileNameFromPath(path), range),
            uri: buildFileUriWithRange(path, range),
          },
        ],
        opts
      )
    },
    [insertFileReferences]
  )

  // Shared upload pool used by the menu's "Upload local file" button,
  // browser drag-drop in web mode, paste in web mode, and the fallback
  // path of `appendFilesAsResources` for remote-desktop. Splits oversize
  // from acceptable, runs uploads with bounded concurrency, surfaces
  // failures via toast, and finally appends the successful paths as
  // ResourceLinks. Returns nothing — all state changes happen via the
  // existing setters / toast.
  const uploadAndAppendFiles = useCallback(
    async (files: File[]) => {
      if (files.length === 0) return
      const oversized = files.filter((f) => f.size > UPLOAD_MAX_BYTES)
      const accepted = files.filter((f) => f.size <= UPLOAD_MAX_BYTES)
      const limitMb = Math.round(UPLOAD_MAX_BYTES / (1024 * 1024))
      if (oversized.length > 0) {
        toast.error(
          tAttach("attachUploadTooLarge", {
            limit: limitMb,
            names: oversized.map((f) => f.name).join(", "),
          })
        )
      }
      if (accepted.length === 0) return

      // Concurrent uploads — one failure doesn't block the rest. Cap at 3:
      // small enough to keep server load predictable, large enough to feel
      // responsive for a handful of files.
      const uploaded: string[] = []
      const failed: Array<{ name: string; reason: unknown }> = []
      const quotaRejected: string[] = []
      const CONCURRENCY = 3
      let cursor = 0
      const workers = Array.from(
        { length: Math.min(CONCURRENCY, accepted.length) },
        async () => {
          while (cursor < accepted.length) {
            const idx = cursor++
            const file = accepted[idx]
            try {
              const r = await uploadAttachment(file, attachmentTabId ?? null)
              uploaded.push(r.path)
            } catch (error) {
              if (isEmptyAttachmentError(error)) {
                // Empty files are explicitly dropped on the floor — log
                // and move on without a user-facing error toast.
                console.warn(
                  `[MessageInput] skipping empty attachment: ${file.name}`
                )
                continue
              }
              const appError = extractAppCommandError(error)
              if (appError?.i18n_key === UPLOAD_I18N_KEY_QUOTA_EXCEEDED) {
                quotaRejected.push(file.name)
                continue
              }
              failed.push({ name: file.name, reason: error })
            }
          }
        }
      )
      await Promise.all(workers)

      if (quotaRejected.length > 0) {
        toast.error(
          tAttach("attachUploadQuotaExceeded", {
            names: quotaRejected.join(", "),
          })
        )
      }
      if (failed.length > 0) {
        for (const f of failed) {
          console.error(
            `[MessageInput] upload attachment failed (${f.name}):`,
            f.reason
          )
        }
        toast.error(
          tAttach("attachUploadFailed", {
            names: failed.map((r) => r.name).join(", "),
          })
        )
      }
      if (uploaded.length > 0) {
        appendResourceAttachments(uploaded)
      }
    },
    [appendResourceAttachments, attachmentTabId, tAttach]
  )

  const appendEmbeddedResources = useCallback(
    (
      resources: Array<{
        uri: string
        name: string
        mimeType: string | null
        text?: string | null
        blob?: string | null
      }>
    ) => {
      // Inline bytes (no real path): each becomes a sentinel file badge whose
      // embedded `resource` block is reconciled back in at send time.
      insertFileReferences(
        resources.map((resource) => ({
          name: resource.name,
          realBlock: {
            type: "resource" as const,
            uri: resource.uri,
            mime_type: resource.mimeType,
            text: resource.text ?? null,
            blob: resource.blob ?? null,
          },
        }))
      )
    },
    [insertFileReferences]
  )

  // Path-less files (browser `File` objects: drag-drop in web mode, paste,
  // or `<input type=file>` in any mode) need a real backing path before
  // the agent can read them. Only the truly local desktop keeps the legacy
  // base64/embedded fallback — web and remote-desktop both push through
  // `uploadAndAppendFiles` so the resulting ResourceLink points at a real
  // server-side file.
  const appendFilesAsResources = useCallback(
    async (files: File[]) => {
      if (files.length === 0) return
      const pathLinks: Array<{
        uri: string
        name: string
        mimeType: string | null
        dedupeKey: string
      }> = []
      const fallbackDataLinks: Array<{
        uri: string
        name: string
        mimeType: string | null
        dedupeKey: string
      }> = []
      const embeddedResources: Array<{
        uri: string
        name: string
        mimeType: string | null
        text?: string | null
        blob?: string | null
      }> = []
      const uploadCandidates: File[] = []

      for (const file of files) {
        const path = getFilePath(file)
        const name = file.name || `resource-${randomUUID()}`
        const mimeType = file.type || mimeTypeFromPath(name)
        if (path) {
          const uri = buildFileUri(path)
          pathLinks.push({
            uri,
            name: fileNameFromPath(path),
            mimeType: mimeTypeFromPath(path) ?? mimeType ?? null,
            dedupeKey: uri,
          })
          continue
        }

        if (!showNativePaperclip) {
          uploadCandidates.push(file)
          continue
        }

        if (!promptCapabilities.embedded_context) {
          const base64 = await blobToBase64(file)
          const dataUri = buildDataUri(base64, mimeType ?? null)
          fallbackDataLinks.push({
            uri: dataUri,
            name,
            mimeType: mimeType ?? null,
            dedupeKey: `${name}:${file.size}:${file.lastModified}`,
          })
          continue
        }

        const uri = buildClipboardResourceUri(name)
        if (isTextLikeFile(file)) {
          const textContent = await file.text()
          embeddedResources.push({
            uri,
            name,
            mimeType: mimeType ?? null,
            text: textContent,
          })
        } else {
          const blobContent = await blobToBase64(file)
          embeddedResources.push({
            uri,
            name,
            mimeType: mimeType ?? null,
            blob: blobContent,
          })
        }
      }

      appendResourceLinks(pathLinks)
      appendResourceLinks(fallbackDataLinks)
      appendEmbeddedResources(embeddedResources)
      if (uploadCandidates.length > 0) {
        await uploadAndAppendFiles(uploadCandidates)
      }
    },
    [
      appendEmbeddedResources,
      appendResourceLinks,
      promptCapabilities.embedded_context,
      showNativePaperclip,
      uploadAndAppendFiles,
    ]
  )

  const appendImageAttachments = useCallback(async (files: File[]) => {
    if (files.length === 0) return
    const parsed = await Promise.all(
      files.map(async (file, index) => {
        const mimeType =
          file.type && file.type.startsWith("image/")
            ? file.type
            : (mimeTypeFromPath(file.name) ?? "image/png")
        const base64Data = await blobToBase64(file)
        return {
          id: `image:${Date.now()}:${index}:${randomUUID()}`,
          type: "image" as const,
          data: base64Data,
          uri: null,
          name: file.name || `image-${Date.now()}-${index + 1}`,
          mimeType,
        }
      })
    )
    setAttachments((prev) => [...prev, ...parsed])
  }, [])

  const appendImagePathAttachments = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0 || !canAttachImages) return
      const settled = await Promise.allSettled(
        paths.map(async (path, index) => {
          const data = await readFileBase64(path, DRAG_DROP_IMAGE_MAX_BYTES)
          return {
            id: `image:${Date.now()}:${index}:${randomUUID()}`,
            type: "image" as const,
            data,
            uri: buildFileUri(path),
            name: fileNameFromPath(path),
            mimeType: mimeTypeFromPath(path) ?? "image/png",
          }
        })
      )

      const parsed: ImageInputAttachment[] = []
      settled.forEach((result, index) => {
        if (result.status === "fulfilled") {
          parsed.push(result.value)
          return
        }
        console.error(
          `[MessageInput] drop image path failed (${paths[index]}):`,
          result.reason
        )
      })
      if (parsed.length === 0) return
      setAttachments((prev) => [...prev, ...parsed])
    },
    [canAttachImages]
  )

  const appendPathsFromDrop = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return
      const normalized = paths.filter(
        (path): path is string => typeof path === "string" && path.length > 0
      )
      if (normalized.length === 0) return

      const imagePaths: string[] = []
      const resourcePaths: string[] = []
      for (const path of normalized) {
        const mimeType = mimeTypeFromPath(path) ?? ""
        if (canAttachImages && mimeType.startsWith("image/")) {
          imagePaths.push(path)
        } else {
          resourcePaths.push(path)
        }
      }

      if (imagePaths.length > 0) {
        await appendImagePathAttachments(imagePaths)
      }
      if (resourcePaths.length > 0) {
        appendResourceAttachments(resourcePaths)
      }
    },
    [appendImagePathAttachments, appendResourceAttachments, canAttachImages]
  )

  const appendPathsFromDropRef = useRef(appendPathsFromDrop)
  useEffect(() => {
    appendPathsFromDropRef.current = appendPathsFromDrop
  }, [appendPathsFromDrop])

  // Remote-workspace counterpart of `appendPathsFromDrop`. Reads each
  // local path through Rust, ships the bytes via the upload proxy, then
  // appends the resulting server-side paths as ResourceLinks. Failures
  // (oversize, ENOENT, network) are reported in a single aggregated toast
  // matching `uploadAndAppendFiles`.
  const uploadPathsToRemote = useCallback(
    async (paths: string[]) => {
      const normalized = paths.filter(
        (p): p is string => typeof p === "string" && p.length > 0
      )
      if (normalized.length === 0) return

      const limitMb = Math.round(UPLOAD_MAX_BYTES / (1024 * 1024))
      const succeeded: string[] = []
      const failed: Array<{ name: string; reason: unknown }> = []
      const oversize: string[] = []
      const directories: string[] = []
      const quotaRejected: string[] = []

      const CONCURRENCY = 3
      let cursor = 0
      const workers = Array.from(
        { length: Math.min(CONCURRENCY, normalized.length) },
        async () => {
          while (cursor < normalized.length) {
            const idx = cursor++
            const path = normalized[idx]
            const name = path.split(/[/\\]/).pop() || path
            try {
              const r = await uploadLocalPathToRemote(
                path,
                attachmentTabId ?? null
              )
              succeeded.push(r.path)
            } catch (error) {
              if (isEmptyAttachmentError(error)) {
                console.warn(
                  `[MessageInput] skipping empty remote-drop attachment: ${name}`
                )
                continue
              }
              // The Rust side tags structured upload errors with an
              // `i18n_key` (see `app_error::UPLOAD_I18N_KEY_*`); branch
              // on the key so each user-visible category lands in its own
              // toast instead of the generic "upload failed" bucket.
              // Falling back to the bare message would couple us to the
              // exact English phrasing in `remote_proxy.rs`.
              const appError = extractAppCommandError(error)
              const i18nKey = appError?.i18n_key ?? null
              if (i18nKey === UPLOAD_I18N_KEY_TOO_LARGE) {
                oversize.push(name)
              } else if (i18nKey === UPLOAD_I18N_KEY_NOT_A_FILE) {
                // Dragging a directory or a special file (FIFO, device
                // node) lands here. The Rust guard short-circuits before
                // we even read bytes; surface a dedicated toast so the
                // user understands why nothing was attached.
                directories.push(name)
              } else if (i18nKey === UPLOAD_I18N_KEY_QUOTA_EXCEEDED) {
                quotaRejected.push(name)
              } else {
                failed.push({ name, reason: error })
              }
            }
          }
        }
      )
      await Promise.all(workers)

      if (oversize.length > 0) {
        toast.error(
          tAttach("attachUploadTooLarge", {
            limit: limitMb,
            names: oversize.join(", "),
          })
        )
      }
      if (directories.length > 0) {
        toast.error(
          tAttach("attachUploadNotAFile", {
            names: directories.join(", "),
          })
        )
      }
      if (quotaRejected.length > 0) {
        toast.error(
          tAttach("attachUploadQuotaExceeded", {
            names: quotaRejected.join(", "),
          })
        )
      }
      if (failed.length > 0) {
        for (const f of failed) {
          console.error(
            `[MessageInput] remote path upload failed (${f.name}):`,
            f.reason
          )
        }
        toast.error(
          tAttach("attachUploadFailed", {
            names: failed.map((f) => f.name).join(", "),
          })
        )
      }
      if (succeeded.length > 0) {
        appendResourceAttachments(succeeded)
      }
    },
    [appendResourceAttachments, attachmentTabId, tAttach]
  )

  const uploadPathsToRemoteRef = useRef(uploadPathsToRemote)
  useEffect(() => {
    uploadPathsToRemoteRef.current = uploadPathsToRemote
  }, [uploadPathsToRemote])

  const appendFilesFromInput = useCallback(
    async (files: File[]) => {
      if (files.length === 0) return
      const imageFiles: File[] = []
      const resourceFiles: File[] = []
      for (const file of files) {
        const mimeType = file.type || mimeTypeFromPath(file.name) || ""
        if (canAttachImages && mimeType.startsWith("image/")) {
          imageFiles.push(file)
        } else {
          resourceFiles.push(file)
        }
      }

      if (imageFiles.length > 0) {
        await appendImageAttachments(imageFiles)
      }
      if (resourceFiles.length > 0) {
        await appendFilesAsResources(resourceFiles)
      }
    },
    [appendFilesAsResources, appendImageAttachments, canAttachImages]
  )

  // Routed from RichComposer's `onPasteFiles`. Returns true when the paste was
  // consumed as an attachment (so the editor doesn't also insert it as text).
  const handlePasteFiles = useCallback(
    (event: ClipboardEvent): boolean => {
      if (disabled) return false
      // The context-menu "Paste" drives text through `view.pasteText`, which
      // runs this handler with a synthetic `new ClipboardEvent("paste")` whose
      // `clipboardData` is null. There's nothing to attach from it (and the
      // image fallback below would otherwise fire a stray async clipboard read),
      // so let the editor's own text paste proceed. Real pastes always carry a
      // (non-null) DataTransfer, so this never short-circuits a genuine paste.
      if (!event.clipboardData) return false
      const files = filesFromClipboard(event.clipboardData)
      if (files.length > 0) {
        void appendFilesFromInput(files).catch((error) => {
          console.error("[MessageInput] paste files failed:", error)
        })
        return true
      }

      // Linux/Tauri (WebKitGTK) fallback: screenshot tools (e.g. WeChat) write
      // the image to the clipboard in a form the synchronous DataTransfer API
      // can't read, so retry through the async Clipboard API. Only for a pure-
      // image clipboard — when text is present we let the default paste run
      // (mirroring `filesFromClipboard`) so copying a spreadsheet cell or rich
      // web content isn't hijacked into an image attachment. Kept synchronous
      // so `imageFilesFromClipboardApi` runs inside the paste user gesture.
      if (clipboardHasText(event.clipboardData)) return false
      void imageFilesFromClipboardApi()
        .then((imageFiles) => {
          if (imageFiles.length === 0) return
          return appendFilesFromInput(imageFiles)
        })
        .catch((error) => {
          console.error("[MessageInput] clipboard image paste failed:", error)
        })
      // The default paste of a textless clipboard is a no-op, so don't claim it.
      return false
    },
    [appendFilesFromInput, disabled]
  )

  // Insert an inline file reference for a file-tree entry dropped onto the
  // composer, placing the caret at the drop point first so the badge lands where
  // the user released (native-textarea feel). Shared by the editor-level drop
  // (`onDropFiles`) and the container-chrome drop (`handleContainerDrop`).
  const insertTreeDropAtPoint = useCallback(
    (absPath: string, clientX: number, clientY: number) => {
      editorRef.current?.focusAtCoords(clientX, clientY)
      appendResourceAttachments([absPath], { atCaret: true })
    },
    [appendResourceAttachments]
  )

  // Routed from RichComposer's `onDropFiles` (ProseMirror's `handleDrop`).
  // Consumes a file-tree drag so PM does not insert the drag's `text/plain`
  // absolute-path fallback as literal text, and stops propagation so the
  // container's own drop handler doesn't double-insert. Returns false for every
  // other drop (OS files, editor text moves) so existing behavior is untouched.
  const handleEditorDrop = useCallback(
    (event: DragEvent): boolean => {
      if (disabled) return false
      if (!hasFileTreeDragType(event.dataTransfer)) return false
      const payload = readFileTreeDragPayload(event.dataTransfer)
      if (!payload) return false
      event.preventDefault()
      event.stopPropagation()
      // `stopPropagation` keeps the container's `onDrop` from double-inserting,
      // but that handler is also what clears the drag overlay — and a completed
      // drop doesn't reliably emit `dragleave`. Clear it here so the overlay
      // can't get stuck covering the composer.
      setDragActiveIfChanged(false)
      insertTreeDropAtPoint(payload.absPath, event.clientX, event.clientY)
      return true
    },
    [disabled, insertTreeDropAtPoint, setDragActiveIfChanged]
  )

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

  // The "+" → Slash commands picker inserts a command badge at the current caret
  // (no trigger token to replace), adding a leading space if the caret isn't at
  // a boundary, and a trailing space after.
  const handleSlashPopoverSelect = useCallback((cmd: AvailableCommandInfo) => {
    const editor = editorRef.current?.getEditor()
    if (!editor) return
    const { $from } = editor.state.selection
    const charBefore =
      $from.parentOffset > 0
        ? $from.parent.textBetween(
            $from.parentOffset - 1,
            $from.parentOffset,
            undefined,
            " "
          )
        : ""
    const needsSpace = charBefore !== "" && !/\s/.test(charBefore)
    let chain = editor.chain().focus()
    if (needsSpace) chain = chain.insertContent(" ")
    chain.insertReference(commandToReference(cmd)).insertContent(" ").run()
  }, [])

  // ── "+" menu skill shortcuts (experts / daily office / research) ──
  //
  // Surface the welcome-page skill families inside an active conversation. Each
  // item drops that skill's leading invocation badge into the composer. A skill
  // not linked to the current agent is "locked": clicking it surfaces a hint
  // (and a jump to Settings) instead of injecting a badge the agent can't act on
  // — the same gating QuickActions applies, to avoid a wasted send.
  const expertsSorted = useMemo(
    () =>
      [...experts].sort(
        (a, b) =>
          (a.metadata.sort_order ?? 0) - (b.metadata.sort_order ?? 0) ||
          a.metadata.id.localeCompare(b.metadata.id)
      ),
    [experts]
  )

  const scienceSorted = useMemo(
    () =>
      [...science].sort(
        (a, b) =>
          (a.metadata.sort_order ?? 0) - (b.metadata.sort_order ?? 0) ||
          a.metadata.id.localeCompare(b.metadata.id)
      ),
    [science]
  )

  const isSkillLocked = useCallback(
    (id: string) => !!agentType && skillStatusReady && !enabledIds.has(id),
    [agentType, skillStatusReady, enabledIds]
  )

  const notifySkillNotEnabled = useCallback(
    (skillLabel: string, section: SettingsSection) => {
      const agentLabel = agentType ? AGENT_LABELS[agentType] : ""
      toast.warning(
        tQa("notEnabled.title", { skill: skillLabel, agent: agentLabel }),
        {
          description: tQa("notEnabled.description"),
          action: {
            label: tQa("notEnabled.action"),
            onClick: () => {
              void openSettingsWindow(section).catch((err) =>
                console.error("[MessageInput] failed to open settings:", err)
              )
            },
          },
        }
      )
    },
    [agentType, tQa]
  )

  // Insert a skill shortcut: seed the template only into an *empty* composer
  // (never clobber an in-progress draft), then prepend the skill as the leading
  // invocation badge. Deferred to the next frame — inserting the badge mounts a
  // React NodeView rendered with a synchronous flushSync(), which warns if run
  // during React's commit phase (same pattern as the inject/hydration effects).
  const insertSkillShortcut = useCallback(
    (skill: { id: string; label: string }, template: string) => {
      requestAnimationFrame(() => {
        const handle = editorRef.current
        const editor = handle?.getEditor()
        if (!handle || !editor) return
        if (template && isComposerEmpty(editor)) {
          handle.setText(template)
        }
        applyExpertReference(editor, {
          refType: "skill",
          id: skill.id,
          label: skill.label,
          uri: null,
          meta: { invocationPrefix: skillPrefix, scope: "expert" },
        })
        syncComposerEmpty()
        handle.focus()
      })
    },
    [skillPrefix, syncComposerEmpty]
  )

  const handleExpertShortcut = useCallback(
    (item: ExpertListItem) => {
      const label =
        pickLocalized(item.metadata.display_name, locale) || item.metadata.id
      if (isSkillLocked(item.metadata.id)) {
        notifySkillNotEnabled(label, "experts")
        return
      }
      // Experts are open-ended: just the leading badge, no canned template.
      insertSkillShortcut({ id: item.metadata.id, label }, "")
    },
    [locale, isSkillLocked, notifySkillNotEnabled, insertSkillShortcut]
  )

  const handleOfficeShortcut = useCallback(
    (action: OfficeAction) => {
      const label = tQa(action.id as Parameters<typeof tQa>[0])
      if (isSkillLocked(action.skillId)) {
        notifySkillNotEnabled(label, "office-tools")
        return
      }
      insertSkillShortcut(
        { id: action.skillId, label },
        tQa(action.promptKey as Parameters<typeof tQa>[0])
      )
    },
    [tQa, isSkillLocked, notifySkillNotEnabled, insertSkillShortcut]
  )

  const handleScienceShortcut = useCallback(
    (item: ScienceListItem) => {
      const label =
        pickLocalized(item.metadata.display_name, locale) || item.metadata.id
      if (isSkillLocked(item.metadata.id)) {
        notifySkillNotEnabled(label, "science")
        return
      }
      // Science skills are open-ended methodologies: just the leading badge,
      // no canned template (mirrors experts).
      insertSkillShortcut({ id: item.metadata.id, label }, "")
    },
    [locale, isSkillLocked, notifySkillNotEnabled, insertSkillShortcut]
  )

  const handlePickFiles = useCallback(async () => {
    if (disabled) return
    // Only wired up when `showNativePaperclip` is true (i.e. local desktop),
    // so we can hand raw OS paths to the local agent without a round-trip.
    try {
      const selected = await openFileDialog({
        multiple: true,
        directory: false,
        defaultPath,
      })
      if (!selected) return
      const picked = Array.isArray(selected) ? selected : [selected]
      appendResourceAttachments(picked.filter((item): item is string => !!item))
    } catch (error) {
      console.error("[MessageInput] pick files failed:", error)
    }
  }, [appendResourceAttachments, defaultPath, disabled])

  const [serverFilePickerOpen, setServerFilePickerOpen] = useState(false)

  const handleUploadLocalFiles = useCallback(async () => {
    if (disabled) return
    // Open a hidden <input type="file"> to grab File objects (browsers and
    // Tauri webviews both produce blob-style File objects from this control,
    // never raw OS paths), then upload each one — `uploadAttachment` picks
    // the right transport (direct fetch in web mode, IPC-proxied multipart
    // in remote-desktop mode).
    const input = document.createElement("input")
    input.type = "file"
    input.multiple = true
    input.onchange = async () => {
      const all = input.files ? Array.from(input.files) : []
      await uploadAndAppendFiles(all)
    }
    input.click()
  }, [disabled, uploadAndAppendFiles])

  const handleServerFilesSelected = useCallback(
    (paths: string[]) => {
      if (paths.length === 0) return
      appendResourceAttachments(paths)
    },
    [appendResourceAttachments]
  )

  const loadQuickMessages = useCallback(async () => {
    setQuickMessagesLoading(true)
    try {
      const list = await quickMessagesList()
      setQuickMessages(list)
    } catch (error) {
      console.error("[MessageInput] load quick messages failed:", error)
    } finally {
      setQuickMessagesLoading(false)
    }
  }, [])

  const handleAddMenuOpenChange = useCallback(
    (open: boolean) => {
      if (!open) return
      // The editor keeps its selection while the menu is open, so a quick
      // message inserts back at the same caret without tracking an offset.
      loadQuickMessages().catch((error) => {
        console.error("[MessageInput] quick messages refresh failed:", error)
      })
    },
    [loadQuickMessages]
  )

  const handleQuickMessageSelect = useCallback((message: QuickMessage) => {
    if (!message.content) return
    editorRef.current?.insertTextAtCursor(message.content)
  }, [])

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
      loadQuickMessages().catch((error) => {
        console.error("[MessageInput] quick messages refresh failed:", error)
      })
    },
    [loadQuickMessages]
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
        await appendFilesFromInput(imageFiles)
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
  }, [disabled, appendFilesFromInput, t])

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
        appendFileRangeAttachment(path, range, { atCaret: true })
      } else {
        appendResourceAttachments([path], { atCaret: true })
      }
    }

    window.addEventListener(ATTACH_FILE_TO_SESSION_EVENT, handleAttachFile)
    return () => {
      window.removeEventListener(ATTACH_FILE_TO_SESSION_EVENT, handleAttachFile)
    }
  }, [appendResourceAttachments, appendFileRangeAttachment, attachmentTabId])

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

  useEffect(() => {
    let cancelled = false
    const unlisteners: Array<() => void | Promise<void>> = []

    const cleanupListeners = () => {
      for (const fn of unlisteners.splice(0)) {
        disposeTauriListener(fn, "MessageInput.dragDrop")
      }
    }

    type DragDropPayload =
      | {
          type: "enter" | "drop"
          paths: string[]
          position: { x: number; y: number }
        }
      | {
          type: "over"
          position: { x: number; y: number }
        }
      | { type: "leave" }

    const handlePayload = (payload: DragDropPayload) => {
      const host = containerRef.current
      if (!host) return
      if (payload.type === "leave") {
        setDragActiveIfChanged(false)
        return
      }
      const inside = pointWithinElement(payload.position, host)
      if (payload.type === "drop") {
        setDragActiveIfChanged(false)
        if (Date.now() - lastDomDropAtRef.current < 250) return
        if (!inside || disabledRef.current) return
        if (getActiveRemoteConnectionId() !== null) {
          // Remote workspace: local OS paths are unreachable from the
          // remote agent, so stream the bytes through the upload proxy and
          // attach the resulting server-side paths instead.
          void uploadPathsToRemoteRef.current(payload.paths).catch((error) => {
            console.error(
              "[MessageInput] remote drag-drop upload failed:",
              error
            )
          })
          return
        }
        void appendPathsFromDropRef.current(payload.paths).catch((error) => {
          console.error("[MessageInput] drag drop paths failed:", error)
        })
        return
      }
      setDragActiveIfChanged(inside && !disabledRef.current)
    }

    const setup = async () => {
      if (!isDesktop()) return
      const { getCurrentWebview } = await import("@tauri-apps/api/webview")
      const { TauriEvent } = await import("@tauri-apps/api/event")
      const webview = getCurrentWebview()
      try {
        const unlistenEnter = await webview.listen<{
          paths: string[]
          position: { x: number; y: number }
        }>(TauriEvent.DRAG_ENTER, (event) => {
          if (cancelled) return
          handlePayload({
            type: "enter",
            paths: event.payload.paths,
            position: event.payload.position,
          })
        })
        unlisteners.push(unlistenEnter)

        const unlistenOver = await webview.listen<{
          position: { x: number; y: number }
        }>(TauriEvent.DRAG_OVER, (event) => {
          if (cancelled) return
          handlePayload({
            type: "over",
            position: event.payload.position,
          })
        })
        unlisteners.push(unlistenOver)

        const unlistenDrop = await webview.listen<{
          paths: string[]
          position: { x: number; y: number }
        }>(TauriEvent.DRAG_DROP, (event) => {
          if (cancelled) return
          handlePayload({
            type: "drop",
            paths: event.payload.paths,
            position: event.payload.position,
          })
        })
        unlisteners.push(unlistenDrop)

        const unlistenLeave = await webview.listen(
          TauriEvent.DRAG_LEAVE,
          () => {
            if (cancelled) return
            handlePayload({ type: "leave" })
          }
        )
        unlisteners.push(unlistenLeave)
      } catch {
        // Ignore non-Tauri environments.
      } finally {
        if (cancelled) {
          cleanupListeners()
        }
      }
    }

    void setup()

    return () => {
      cancelled = true
      cleanupListeners()
    }
  }, [setDragActiveIfChanged])

  const removeAttachment = useCallback((id: string) => {
    setAttachments((prev) => prev.filter((item) => item.id !== id))
  }, [])

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
    // The wire encoding is capability-driven (native `image` block vs embedded
    // `resource` blob) so an agent that advertises `image: false` but
    // `embedded_context: true` (e.g. Grok) still receives the bytes it accepts.
    for (const attachment of attachments) {
      if (attachment.type === "image") {
        blocks.push(
          imageAttachmentToPromptBlock(attachment, promptCapabilities)
        )
      }
    }

    const displayText =
      displayProse ||
      `Attached ${attachments.length} attachment${attachments.length > 1 ? "s" : ""}`
    return { blocks, displayText }
  }, [attachments, skillPrefix, promptCapabilities])

  // Clear the editor + attachments after a send / enqueue / save.
  const resetComposer = useCallback(() => {
    editorRef.current?.clear()
    setComposerEmpty(true)
    setAttachments([])
    embeddedPayloadsRef.current.clear()
    closeSlashMenu()
  }, [closeSlashMenu])

  const handleSend = useCallback(() => {
    // The editor stays editable while `disabled` (the agent is busy) so the user
    // can keep typing, but a plain send is blocked — only enqueue / queue-edit
    // save go through. Mirrors the legacy textarea's keydown guard.
    if (disabled && !isPrompting && !isEditingQueueItem) return
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
    buildDraft,
    effectiveModeId,
    showModeSelector,
    effectiveDraftStorageKey,
    resetComposer,
  ])

  // Navigation/confirm/escape keys for the `/` (commands) and `$` (Codex skills)
  // runtime menu, routed from inside the editor (RichComposer.onExternalMenuKeyDown)
  // because ProseMirror's DOM handler fires before a host capture handler could.
  // Returns true for keys the menu consumed; false (e.g. a letter that filters)
  // lets normal editing proceed.
  const handleExternalMenuKeyDown = useCallback(
    (event: KeyboardEvent): boolean => {
      if (event.isComposing) return false
      if (!slashMenuOpen || slashAutocompleteCount === 0) return false
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
      if (event.key === "Escape") {
        closeSlashMenu()
        return true
      }
      return false
    },
    [
      slashMenuOpen,
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
      if (e.nativeEvent.isComposing) return
      if (
        isEditingQueueItem &&
        e.key === "Escape" &&
        !slashMenuOpen &&
        onCancelQueueEdit
      ) {
        e.preventDefault()
        onCancelQueueEdit()
      }
    },
    [isEditingQueueItem, slashMenuOpen, onCancelQueueEdit]
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

  const handleContainerDragOver = useCallback(
    (event: React.DragEvent<HTMLDivElement>) => {
      const isTreeDrag = hasFileTreeDragType(event.dataTransfer)
      if (!hasDragFiles(event.dataTransfer) && !isTreeDrag) return
      event.preventDefault()
      if (!disabled) {
        // A file-tree entry is copied in as a reference, not moved.
        if (isTreeDrag) event.dataTransfer.dropEffect = "copy"
        setDragActiveIfChanged(true)
      }
    },
    [disabled, setDragActiveIfChanged]
  )

  const handleContainerDragLeave = useCallback(
    (event: React.DragEvent<HTMLDivElement>) => {
      const related = event.relatedTarget
      if (
        related &&
        related instanceof Node &&
        event.currentTarget.contains(related)
      ) {
        return
      }
      setDragActiveIfChanged(false)
    },
    [setDragActiveIfChanged]
  )

  const handleContainerDrop = useCallback(
    (event: React.DragEvent<HTMLDivElement>) => {
      const treePayload = hasFileTreeDragType(event.dataTransfer)
        ? readFileTreeDragPayload(event.dataTransfer)
        : null
      if (!hasDragFiles(event.dataTransfer) && !treePayload) return
      event.preventDefault()
      lastDomDropAtRef.current = Date.now()
      setDragActiveIfChanged(false)
      if (disabled) return
      // A file-tree entry dropped on the composer chrome (the editor's own drop
      // surface is handled first by `handleEditorDrop`) becomes an inline file
      // reference at the drop point.
      if (treePayload) {
        insertTreeDropAtPoint(treePayload.absPath, event.clientX, event.clientY)
        return
      }
      const files = Array.from(event.dataTransfer.files ?? [])
      if (files.length > 0) {
        void appendFilesFromInput(files).catch((error) => {
          console.error("[MessageInput] drop files failed:", error)
        })
      }
    },
    [
      appendFilesFromInput,
      disabled,
      insertTreeDropAtPoint,
      setDragActiveIfChanged,
    ]
  )

  const hasImageAttachments = imageAttachments.length > 0
  const showDragActive = isDragActive && !disabled

  const inlineSelectorItems = (
    <>
      {hasConfigOptions &&
        availableConfigOptions.map((option) => {
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
    <Button
      onClick={onCancel}
      variant="destructive"
      size="icon"
      className="h-8 w-8"
      title={t("cancel")}
    >
      <Square className="size-4" />
    </Button>
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
      onDragOver={handleContainerDragOver}
      onDragLeave={handleContainerDragLeave}
      onDrop={handleContainerDrop}
    >
      {slashMenuOpen && slashAutocompleteCount > 0 && (
        <div className="absolute bottom-full left-0 right-0 mb-1 z-50 flex max-h-[min(16rem,40dvh)] flex-col overflow-hidden rounded-xl border border-border bg-popover shadow-lg">
          {/* No search box: the user types the filter inline after `/` (like the
              `@` panel); navigation is routed from the editor's keydown. */}
          <div ref={slashMenuListRef} className="flex-1 overflow-y-auto p-1">
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
                  /{cmd.name}
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
                  <>
                    {imageAttachments.map((attachment) => (
                      <div
                        key={attachment.id}
                        className="relative shrink-0 overflow-hidden rounded-md border border-border/70 bg-muted/30"
                      >
                        <button
                          type="button"
                          onClick={() => setPreviewAttachmentId(attachment.id)}
                          className="cursor-pointer transition-opacity hover:opacity-80"
                        >
                          <Image
                            src={`data:${attachment.mimeType};base64,${attachment.data}`}
                            alt={attachment.name}
                            width={56}
                            height={56}
                            unoptimized
                            className="h-14 w-14 object-cover"
                          />
                        </button>
                        <button
                          type="button"
                          onClick={() => removeAttachment(attachment.id)}
                          className="absolute right-1 top-1 rounded-sm bg-background/70 p-0.5 hover:bg-background"
                          aria-label={t("removeAttachmentAria", {
                            name: attachment.name,
                          })}
                        >
                          <X className="h-3 w-3" />
                        </button>
                      </div>
                    ))}
                  </>
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
                onChange={handleComposerChange}
                onReady={handleComposerReady}
                onSubmit={handleSend}
                onFocus={onFocus}
                onPasteFiles={handlePasteFiles}
                onDropFiles={handleEditorDrop}
                onPlainPaste={handlePlainPasteShortcut}
                submitShortcut={shortcuts.send_message}
                newlineShortcut={shortcuts.newline_in_message}
                isExternalMenuOpen={slashMenuOpen && slashAutocompleteCount > 0}
                onExternalMenuKeyDown={handleExternalMenuKeyDown}
                className="min-h-0 flex-1"
              />
              <div className="flex shrink-0 items-end justify-between gap-1 px-2 pb-2">
                <div className="flex min-w-0 items-end gap-1">
                  <DropdownMenu onOpenChange={handleAddMenuOpenChange}>
                    <DropdownMenuTrigger asChild>
                      <Button
                        disabled={disabled}
                        variant="ghost"
                        size="icon-xs"
                        className="shrink-0 text-muted-foreground"
                        title={t("addActions")}
                        aria-label={t("addActions")}
                      >
                        <Plus className="size-4" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent
                      side="top"
                      align="start"
                      className="min-w-56 w-auto"
                    >
                      {showNativePaperclip ? (
                        <DropdownMenuItem
                          onClick={() => {
                            handlePickFiles().catch((error) => {
                              console.error(
                                "[MessageInput] pick files from menu failed:",
                                error
                              )
                            })
                          }}
                        >
                          <Paperclip className="size-4" />
                          {t("attachFiles")}
                        </DropdownMenuItem>
                      ) : (
                        <>
                          <DropdownMenuItem
                            onClick={() => {
                              handleUploadLocalFiles().catch((error) => {
                                console.error(
                                  "[MessageInput] upload local files failed:",
                                  error
                                )
                              })
                            }}
                          >
                            <Upload className="size-4" />
                            {t("attachLocalUpload")}
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onClick={() => setServerFilePickerOpen(true)}
                          >
                            <FolderSearch className="size-4" />
                            {t("attachServerFile")}
                          </DropdownMenuItem>
                        </>
                      )}
                      <DropdownMenuSub>
                        <DropdownMenuSubTrigger>
                          <MessageSquareText className="size-4" />
                          {t("quickMessages")}
                        </DropdownMenuSubTrigger>
                        <DropdownMenuSubContent
                          className="min-w-40 overflow-y-auto"
                          style={{
                            maxWidth: "min(20rem, calc(100vw - 1rem))",
                            maxHeight:
                              "min(32rem, var(--radix-dropdown-menu-content-available-height))",
                          }}
                        >
                          {quickMessagesLoading &&
                          quickMessages.length === 0 ? (
                            <div className="px-3 py-4 text-center text-xs text-muted-foreground">
                              {t("quickMessagesLoading")}
                            </div>
                          ) : quickMessages.length === 0 ? (
                            <div className="px-3 py-4 text-center text-xs text-muted-foreground">
                              {t("quickMessagesEmpty")}
                            </div>
                          ) : (
                            quickMessages.map((message) => (
                              <DropdownMenuItem
                                key={message.id}
                                onClick={() =>
                                  handleQuickMessageSelect(message)
                                }
                              >
                                <span className="truncate">
                                  {message.title || (
                                    <span className="italic text-muted-foreground">
                                      {t("quickMessageUntitled")}
                                    </span>
                                  )}
                                </span>
                              </DropdownMenuItem>
                            ))
                          )}
                        </DropdownMenuSubContent>
                      </DropdownMenuSub>
                      {onAddFeedback && (
                        <DropdownMenuItem
                          disabled={feedbackAddDisabled}
                          onClick={onAddFeedback}
                          title={
                            feedbackAddDisabled
                              ? t("liveFeedbackDisabledHint")
                              : undefined
                          }
                        >
                          <MessageSquarePlus className="size-4" />
                          {t("liveFeedback")}
                        </DropdownMenuItem>
                      )}
                      <DropdownMenuSub
                        open={slashDropdownOpen}
                        onOpenChange={handleSlashDropdownOpenChange}
                      >
                        <DropdownMenuSubTrigger
                          disabled={slashCommands.length === 0}
                        >
                          <Command className="size-4" />
                          {t("slashCommands")}
                        </DropdownMenuSubTrigger>
                        <DropdownMenuSubContent
                          className="flex min-w-72 flex-col overflow-hidden p-0"
                          style={{
                            maxWidth: "min(20rem, calc(100vw - 1rem))",
                            maxHeight:
                              "min(32rem, var(--radix-dropdown-menu-content-available-height))",
                          }}
                        >
                          <div className="flex shrink-0 items-center gap-2 border-b border-border/60 px-3 py-2">
                            <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                            <input
                              ref={slashDropdownInputRef}
                              type="text"
                              role="searchbox"
                              aria-label={t("slashSearchPlaceholder")}
                              value={slashDropdownSearch}
                              onChange={(e) =>
                                setSlashDropdownSearch(e.target.value)
                              }
                              onKeyDown={(e) => {
                                if (e.key === "ArrowDown") {
                                  e.preventDefault()
                                  const container = e.currentTarget.closest(
                                    '[data-slot="dropdown-menu-sub-content"]'
                                  )
                                  const firstItem =
                                    container?.querySelector<HTMLElement>(
                                      '[role="menuitem"]'
                                    )
                                  firstItem?.focus()
                                  return
                                }
                                if (e.key === "Enter") {
                                  e.preventDefault()
                                  const first = filteredSlashDropdownCommands[0]
                                  if (first) {
                                    handleSlashPopoverSelect(first)
                                    setSlashDropdownOpen(false)
                                  }
                                  return
                                }
                                if (e.key === "Escape" || e.key === "Tab")
                                  return
                                // Prevent radix DropdownMenu's built-in typeahead
                                // from hijacking letter keys while the user is
                                // typing.
                                e.stopPropagation()
                              }}
                              placeholder={t("slashSearchPlaceholder")}
                              className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
                              autoComplete="off"
                              spellCheck={false}
                            />
                          </div>
                          <div className="flex-1 overflow-y-auto p-1">
                            {filteredSlashDropdownCommands.length === 0 ? (
                              <div className="px-3 py-6 text-center text-xs text-muted-foreground">
                                {t("slashSearchEmpty")}
                              </div>
                            ) : (
                              filteredSlashDropdownCommands.map((cmd) => (
                                <DropdownMenuItem
                                  key={cmd.name}
                                  onClick={() => handleSlashPopoverSelect(cmd)}
                                  // Radix focuses the item on pointermove, which
                                  // fires while scrolling (items slide under the
                                  // cursor) and steals focus from the search input.
                                  // Short-circuit that default with preventDefault
                                  // so the search keeps focus until the user
                                  // explicitly clicks.
                                  onPointerMove={(e) => e.preventDefault()}
                                  onPointerLeave={(e) => e.preventDefault()}
                                  className="hover:bg-accent hover:text-accent-foreground"
                                >
                                  <DropdownRadioItemContent
                                    label={`/${cmd.name}`}
                                    description={cmd.description}
                                  />
                                </DropdownMenuItem>
                              ))
                            )}
                          </div>
                        </DropdownMenuSubContent>
                      </DropdownMenuSub>
                      {/* A custom-dir pi can't have skills managed by codeg's
                          default-dir store, so hide these shortcuts instead of
                          offering ones that lock with a Settings path the
                          Experts/Office matrices also hide for this agent. */}
                      {skillManagementSupported && (
                        <>
                          <DropdownMenuSub>
                            <DropdownMenuSubTrigger
                              disabled={expertsSorted.length === 0}
                            >
                              <Sparkles className="size-4" />
                              {t("experts")}
                            </DropdownMenuSubTrigger>
                            <DropdownMenuSubContent
                              className="min-w-44 overflow-y-auto"
                              style={{
                                maxWidth: "min(20rem, calc(100vw - 1rem))",
                                maxHeight:
                                  "min(32rem, var(--radix-dropdown-menu-content-available-height))",
                              }}
                            >
                              {expertsSorted.map((item) => {
                                const Icon = getExpertIcon(item.metadata.icon)
                                const label =
                                  pickLocalized(
                                    item.metadata.display_name,
                                    locale
                                  ) || item.metadata.id
                                return (
                                  <DropdownMenuItem
                                    key={item.metadata.id}
                                    onClick={() => handleExpertShortcut(item)}
                                  >
                                    <Icon className="size-4" />
                                    <span className="flex-1 truncate">
                                      {label}
                                    </span>
                                    {isSkillLocked(item.metadata.id) && (
                                      <Lock className="ml-auto size-3.5 shrink-0 text-muted-foreground/70" />
                                    )}
                                  </DropdownMenuItem>
                                )
                              })}
                            </DropdownMenuSubContent>
                          </DropdownMenuSub>
                          <DropdownMenuSub>
                            <DropdownMenuSubTrigger>
                              <FileStack className="size-4" />
                              {t("office")}
                            </DropdownMenuSubTrigger>
                            <DropdownMenuSubContent
                              className="min-w-44 overflow-y-auto"
                              style={{
                                maxWidth: "min(20rem, calc(100vw - 1rem))",
                                maxHeight:
                                  "min(32rem, var(--radix-dropdown-menu-content-available-height))",
                              }}
                            >
                              {OFFICE_ACTIONS.map((action) => {
                                const Icon = action.icon
                                const label = tQa(
                                  action.id as Parameters<typeof tQa>[0]
                                )
                                return (
                                  <DropdownMenuItem
                                    key={action.id}
                                    onClick={() => handleOfficeShortcut(action)}
                                  >
                                    <Icon className="size-4" />
                                    <span className="flex-1 truncate">
                                      {label}
                                    </span>
                                    {isSkillLocked(action.skillId) && (
                                      <Lock className="ml-auto size-3.5 shrink-0 text-muted-foreground/70" />
                                    )}
                                  </DropdownMenuItem>
                                )
                              })}
                            </DropdownMenuSubContent>
                          </DropdownMenuSub>
                          <DropdownMenuSub>
                            <DropdownMenuSubTrigger
                              disabled={scienceSorted.length === 0}
                            >
                              <FlaskConical className="size-4" />
                              {t("research")}
                            </DropdownMenuSubTrigger>
                            <DropdownMenuSubContent
                              className="min-w-44 overflow-y-auto"
                              style={{
                                maxWidth: "min(20rem, calc(100vw - 1rem))",
                                maxHeight:
                                  "min(32rem, var(--radix-dropdown-menu-content-available-height))",
                              }}
                            >
                              {scienceSorted.map((item) => {
                                const Icon = getScienceIcon(item.metadata.icon)
                                const label =
                                  pickLocalized(
                                    item.metadata.display_name,
                                    locale
                                  ) || item.metadata.id
                                return (
                                  <DropdownMenuItem
                                    key={item.metadata.id}
                                    onClick={() => handleScienceShortcut(item)}
                                  >
                                    <Icon className="size-4" />
                                    <span className="flex-1 truncate">
                                      {label}
                                    </span>
                                    {isSkillLocked(item.metadata.id) && (
                                      <Lock className="ml-auto size-3.5 shrink-0 text-muted-foreground/70" />
                                    )}
                                  </DropdownMenuItem>
                                )
                              })}
                            </DropdownMenuSubContent>
                          </DropdownMenuSub>
                        </>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
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
                {quickMessagesLoading && quickMessages.length === 0 ? (
                  <div className="px-3 py-4 text-center text-xs text-muted-foreground">
                    {t("quickMessagesLoading")}
                  </div>
                ) : quickMessages.length === 0 ? (
                  <div className="px-3 py-4 text-center text-xs text-muted-foreground">
                    {t("quickMessagesEmpty")}
                  </div>
                ) : (
                  quickMessages.map((message) => (
                    <ContextMenuItem
                      key={message.id}
                      onSelect={() => handleQuickMessageSelect(message)}
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
            <div className="flex shrink-0 items-center gap-3">
              <ComposerContextUsage tabId={attachmentTabId ?? null} />
              <ComposerConnectionStatus tabId={attachmentTabId ?? null} />
            </div>
          </div>
        )}
      </div>
      <ImagePreviewDialog
        src={
          previewAttachment
            ? `data:${previewAttachment.mimeType};base64,${previewAttachment.data}`
            : ""
        }
        alt={previewAttachment?.name ?? ""}
        open={previewAttachment !== null}
        onOpenChange={(open) => {
          if (!open) setPreviewAttachmentId(null)
        }}
      />
      {!showNativePaperclip && (
        <ServerFileBrowserDialog
          open={serverFilePickerOpen}
          onOpenChange={setServerFilePickerOpen}
          onSelect={handleServerFilesSelected}
          initialPath={defaultPath ?? undefined}
        />
      )}
    </div>
  )
}
