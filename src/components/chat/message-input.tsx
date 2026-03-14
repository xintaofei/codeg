"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { TauriEvent } from "@tauri-apps/api/event"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import { open } from "@tauri-apps/plugin-dialog"
import Image from "next/image"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { Textarea } from "@/components/ui/textarea"
import {
  Check,
  Ellipsis,
  FileSearch,
  ListPlus,
  Plus,
  Send,
  Square,
  X,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { readFileBase64 } from "@/lib/tauri"
import { disposeTauriListener } from "@/lib/tauri-listener"
import type {
  AvailableCommandInfo,
  PromptCapabilitiesInfo,
  PromptDraft,
  PromptInputBlock,
  SessionConfigOptionInfo,
  SessionModeInfo,
} from "@/lib/types"
import {
  ATTACH_FILE_TO_SESSION_EVENT,
  type AttachFileToSessionDetail,
} from "@/lib/session-attachment-events"
import { ModeSelector } from "@/components/chat/mode-selector"
import { SessionConfigSelector } from "@/components/chat/session-config-selector"
import { SlashCommandMenu } from "@/components/chat/slash-command-menu"
import {
  clearMessageInputDraft,
  loadMessageInputDraft,
  saveMessageInputDraft,
} from "@/lib/message-input-draft"

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
  availableCommands?: AvailableCommandInfo[] | null
  promptCapabilities: PromptCapabilitiesInfo
  attachmentTabId?: string | null
  draftStorageKey?: string | null
  isActive?: boolean
  onEnqueue?: (draft: PromptDraft, modeId: string | null) => void
  editingDraftText?: string | null
  isEditingQueueItem?: boolean
  onSaveQueueEdit?: (draft: PromptDraft) => void
  onCancelQueueEdit?: () => void
}

interface ResourceInputAttachment {
  id: string
  type: "resource"
  kind: "link" | "embedded"
  uri: string
  name: string
  mimeType: string | null
  text?: string | null
  blob?: string | null
}

interface ImageInputAttachment {
  id: string
  type: "image"
  data: string
  uri: string | null
  name: string
  mimeType: string
}

type InputAttachment = ResourceInputAttachment | ImageInputAttachment

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

const MESSAGE_INPUT_SEND_HISTORY_LIMIT = 50

function getMessageInputSendHistoryStorageKey(scopeKey: string | null): string {
  return `codeg:messageInputSendHistory:${scopeKey ?? "global"}`
}

function loadMessageInputSendHistory(scopeKey: string | null): string[] {
  if (typeof window === "undefined") return []
  try {
    const raw = window.localStorage.getItem(
      getMessageInputSendHistoryStorageKey(scopeKey)
    )
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter((item): item is string => typeof item === "string")
      .map((item) => item.trim())
      .filter((item) => item.length > 0)
  } catch {
    return []
  }
}

function saveMessageInputSendHistory(scopeKey: string | null, items: string[]) {
  if (typeof window === "undefined") return
  try {
    window.localStorage.setItem(
      getMessageInputSendHistoryStorageKey(scopeKey),
      JSON.stringify(items)
    )
  } catch {
    // ignore
  }
}

function fileNameFromPath(path: string): string {
  return path.split(/[/\\]/).pop() || path
}

function mimeTypeFromPath(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? ""
  return MIME_BY_EXT[ext] ?? null
}

function toFileUri(path: string): string {
  const normalized = path.replace(/\\/g, "/")
  const encoded = normalized.split("/").map(encodeURIComponent).join("/")
  if (normalized.startsWith("/")) {
    return `file://${encoded}`
  }
  return `file:///${encoded}`
}

function hasDragFiles(dataTransfer: DataTransfer | null): boolean {
  if (!dataTransfer?.types) return false
  return Array.from(dataTransfer.types).includes("Files")
}

function pointWithinElement(
  position: { x: number; y: number },
  element: HTMLElement
): boolean {
  const rect = element.getBoundingClientRect()
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
  return `clipboard://${encodeURIComponent(normalizedName)}-${crypto.randomUUID()}`
}

function buildDataUri(base64Data: string, mimeType: string | null): string {
  const safeMime =
    mimeType && mimeType.trim() ? mimeType : "application/octet-stream"
  return `data:${safeMime};base64,${base64Data}`
}

function SelectorLoadingChip({ label }: { label: string }) {
  return (
    <div className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-border/70 bg-muted/40 px-2 text-[11px] text-muted-foreground">
      <span className="h-1.5 w-1.5 rounded-full bg-primary animate-pulse" />
      <span>{label}</span>
    </div>
  )
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
  availableCommands,
  promptCapabilities,
  attachmentTabId,
  draftStorageKey,
  isActive = false,
  onEnqueue,
  editingDraftText,
  isEditingQueueItem = false,
  onSaveQueueEdit,
  onCancelQueueEdit,
}: MessageInputProps) {
  const t = useTranslations("Folder.chat.messageInput")
  const tQueue = useTranslations("Folder.chat.messageQueue")
  const { shortcuts } = useShortcutSettings()
  const effectiveDraftStorageKey = draftStorageKey ?? attachmentTabId ?? null
  const resolvedPlaceholder = placeholder ?? t("askAnything")
  const [text, setText] = useState(() => {
    if (!effectiveDraftStorageKey) return ""
    return loadMessageInputDraft(effectiveDraftStorageKey) ?? ""
  })
  const [attachments, setAttachments] = useState<InputAttachment[]>([])
  const [isDragActive, setIsDragActive] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const lastDomDropAtRef = useRef(0)
  const composingRef = useRef(false)
  const textRef = useRef(text)
  const disabledRef = useRef(disabled)
  const isPromptingRef = useRef(isPrompting)
  const sendHistoryRef = useRef<string[]>([])
  const [sendHistoryIndex, setSendHistoryIndex] = useState<number | null>(null)

  useEffect(() => {
    if (isActive && !disabled && !isPrompting) {
      requestAnimationFrame(() => {
        textareaRef.current?.focus()
      })
    }
  }, [isActive, disabled, isPrompting])

  useEffect(() => {
    sendHistoryRef.current = loadMessageInputSendHistory(
      effectiveDraftStorageKey
    )
    setSendHistoryIndex(null)
  }, [effectiveDraftStorageKey])
  const dragActiveRef = useRef(false)
  const canAttachImages = promptCapabilities.image

  useEffect(() => {
    textRef.current = text
  }, [text])

  useEffect(() => {
    disabledRef.current = disabled
  }, [disabled])

  useEffect(() => {
    isPromptingRef.current = isPrompting
  }, [isPrompting])

  // Load external draft text when editing a queue item
  const prevEditingDraftRef = useRef<string | null>(null)
  useEffect(() => {
    if (
      isEditingQueueItem &&
      editingDraftText != null &&
      editingDraftText !== prevEditingDraftRef.current
    ) {
      prevEditingDraftRef.current = editingDraftText
      setText(editingDraftText)
      requestAnimationFrame(() => {
        textareaRef.current?.focus()
      })
    } else if (!isEditingQueueItem) {
      prevEditingDraftRef.current = null
    }
  }, [isEditingQueueItem, editingDraftText])

  const setDragActiveIfChanged = useCallback((next: boolean) => {
    if (dragActiveRef.current === next) return
    dragActiveRef.current = next
    setIsDragActive(next)
  }, [])

  useEffect(() => {
    if (!effectiveDraftStorageKey || isEditingQueueItem) return
    saveMessageInputDraft(effectiveDraftStorageKey, text)
  }, [effectiveDraftStorageKey, text, isEditingQueueItem])

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
  const imageAttachments = useMemo(
    () =>
      attachments.filter(
        (attachment): attachment is ImageInputAttachment =>
          attachment.type === "image"
      ),
    [attachments]
  )
  const resourceAttachments = useMemo(
    () =>
      attachments.filter(
        (attachment): attachment is ResourceInputAttachment =>
          attachment.type === "resource"
      ),
    [attachments]
  )
  const hasAttachments = attachments.length > 0
  const hasSendableContent = text.trim().length > 0 || hasAttachments

  // ── Slash command autocomplete ──
  const [slashMenuOpen, setSlashMenuOpen] = useState(false)
  const [slashSelectedIndex, setSlashSelectedIndex] = useState(0)
  const slashCommands = useMemo(
    () => availableCommands ?? [],
    [availableCommands]
  )
  const filteredSlashCommands = useMemo(() => {
    if (!slashMenuOpen || slashCommands.length === 0) return []
    const match = text.match(/^\/(\S*)$/)
    if (!match) return []
    const filter = match[1].toLowerCase()
    return slashCommands.filter((cmd) =>
      cmd.name.toLowerCase().startsWith(filter)
    )
  }, [slashMenuOpen, slashCommands, text])

  const appendResourceLinks = useCallback(
    (
      links: Array<{
        uri: string
        name: string
        mimeType: string | null
        dedupeKey: string
      }>
    ) => {
      if (links.length === 0) return
      setAttachments((prev) => {
        const seen = new Set(
          prev.flatMap((item) =>
            item.type === "resource" && item.kind === "link" ? [item.uri] : []
          )
        )
        const next = [...prev]
        for (const link of links) {
          if (!link.uri || seen.has(link.dedupeKey)) continue
          seen.add(link.dedupeKey)
          next.push({
            id: `resource-link:${link.dedupeKey}`,
            type: "resource",
            kind: "link",
            uri: link.uri,
            name: link.name,
            mimeType: link.mimeType,
          })
        }
        return next
      })
    },
    []
  )

  const appendResourceAttachments = useCallback(
    (paths: string[]) => {
      const normalized = paths
        .filter(
          (path): path is string => typeof path === "string" && path.length > 0
        )
        .map((path) => {
          const uri = toFileUri(path)
          return {
            uri,
            name: fileNameFromPath(path),
            mimeType: mimeTypeFromPath(path),
            dedupeKey: uri,
          }
        })
      appendResourceLinks(normalized)
    },
    [appendResourceLinks]
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
      if (resources.length === 0) return
      setAttachments((prev) => [
        ...prev,
        ...resources.map((resource) => ({
          id: `resource-embedded:${crypto.randomUUID()}`,
          type: "resource" as const,
          kind: "embedded" as const,
          uri: resource.uri,
          name: resource.name,
          mimeType: resource.mimeType,
          text: resource.text ?? null,
          blob: resource.blob ?? null,
        })),
      ])
    },
    []
  )

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

      for (const file of files) {
        const path = getFilePath(file)
        const name = file.name || `resource-${crypto.randomUUID()}`
        const mimeType = file.type || mimeTypeFromPath(name)
        if (path) {
          const uri = toFileUri(path)
          pathLinks.push({
            uri,
            name: fileNameFromPath(path),
            mimeType: mimeTypeFromPath(path) ?? mimeType ?? null,
            dedupeKey: uri,
          })
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
    },
    [
      appendEmbeddedResources,
      appendResourceLinks,
      promptCapabilities.embedded_context,
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
          id: `image:${Date.now()}:${index}:${crypto.randomUUID()}`,
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
            id: `image:${Date.now()}:${index}:${crypto.randomUUID()}`,
            type: "image" as const,
            data,
            uri: toFileUri(path),
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

  const handlePaste = useCallback(
    (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
      if (disabled) return
      const files = Array.from(event.clipboardData?.files ?? [])
      if (files.length === 0) return
      event.preventDefault()
      void appendFilesFromInput(files).catch((error) => {
        console.error("[MessageInput] paste files failed:", error)
      })
    },
    [appendFilesFromInput, disabled]
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

  const handleSlashSelect = useCallback((cmd: AvailableCommandInfo) => {
    setText(`/${cmd.name} `)
    setSlashMenuOpen(false)
  }, [])

  const handleTextChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      const value = e.target.value
      setText(value)
      if (value.length === 0 && sendHistoryIndex !== null) {
        setSendHistoryIndex(null)
      }
      if (slashCommands.length > 0 && /^\/(\S*)$/.test(value)) {
        setSlashSelectedIndex(0)
        setSlashMenuOpen(true)
      } else {
        setSlashMenuOpen(false)
      }
    },
    [sendHistoryIndex, slashCommands.length]
  )

  const pushMessageToSendHistory = useCallback(
    (value: string) => {
      const trimmed = value.trim()
      if (!trimmed) return

      const prev = loadMessageInputSendHistory(effectiveDraftStorageKey)
      const last = prev.length > 0 ? prev[prev.length - 1] : null
      if (last === trimmed) {
        sendHistoryRef.current = prev
        return
      }

      const next = [...prev, trimmed].slice(-MESSAGE_INPUT_SEND_HISTORY_LIMIT)
      saveMessageInputSendHistory(effectiveDraftStorageKey, next)
      sendHistoryRef.current = next
    },
    [effectiveDraftStorageKey]
  )

  const handlePickFiles = useCallback(async () => {
    if (disabled) return
    try {
      const selected = await open({
        multiple: true,
        directory: false,
        defaultPath: defaultPath || undefined,
      })
      if (!selected) return
      const picked = Array.isArray(selected) ? selected : [selected]
      appendResourceAttachments(picked.filter((item): item is string => !!item))
    } catch (error) {
      console.error("[MessageInput] pick files failed:", error)
    }
  }, [appendResourceAttachments, defaultPath, disabled])

  useEffect(() => {
    if (!attachmentTabId) return

    const handleAttachFile = (event: Event) => {
      const customEvent = event as CustomEvent<AttachFileToSessionDetail>
      if (!customEvent.detail) return
      if (customEvent.detail.tabId !== attachmentTabId) return
      appendResourceAttachments([customEvent.detail.path])
    }

    window.addEventListener(ATTACH_FILE_TO_SESSION_EVENT, handleAttachFile)
    return () => {
      window.removeEventListener(ATTACH_FILE_TO_SESSION_EVENT, handleAttachFile)
    }
  }, [appendResourceAttachments, attachmentTabId])

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
        void appendPathsFromDropRef.current(payload.paths).catch((error) => {
          console.error("[MessageInput] drag drop paths failed:", error)
        })
        return
      }
      setDragActiveIfChanged(inside && !disabledRef.current)
    }

    const setup = async () => {
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
    const trimmed = textRef.current.trim()
    if (!trimmed && attachments.length === 0) return null

    const blocks: PromptInputBlock[] = []
    if (trimmed) {
      blocks.push({ type: "text", text: trimmed })
    }
    for (const attachment of attachments) {
      if (attachment.type === "resource") {
        if (attachment.kind === "link") {
          blocks.push({
            type: "resource_link",
            uri: attachment.uri,
            name: attachment.name,
            mime_type: attachment.mimeType,
            description: null,
          })
        } else {
          blocks.push({
            type: "resource",
            uri: attachment.uri,
            mime_type: attachment.mimeType,
            text: attachment.text ?? null,
            blob: attachment.blob ?? null,
          })
        }
      } else {
        blocks.push({
          type: "image",
          data: attachment.data,
          mime_type: attachment.mimeType,
          uri: attachment.uri,
        })
      }
    }

    const displayText =
      trimmed ||
      `Attached ${attachments.length} attachment${attachments.length > 1 ? "s" : ""}`
    return { blocks, displayText }
  }, [attachments])

  const handleSend = useCallback(() => {
    const draft = buildDraft()
    if (!draft) return

    if (!isEditingQueueItem) {
      pushMessageToSendHistory(textRef.current)
      setSendHistoryIndex(null)
    }

    // Edit mode: save back to queue item
    if (isEditingQueueItem && onSaveQueueEdit) {
      onSaveQueueEdit(draft)
      setText("")
      setAttachments([])
      return
    }

    // Prompting mode: enqueue instead of sending
    if (isPrompting && onEnqueue) {
      onEnqueue(draft, showModeSelector ? effectiveModeId : null)
      setText("")
      setAttachments([])
      return
    }

    onSend(draft, showModeSelector ? effectiveModeId : null)
    if (effectiveDraftStorageKey) {
      clearMessageInputDraft(effectiveDraftStorageKey)
    }
    setText("")
    setAttachments([])
  }, [
    buildDraft,
    isEditingQueueItem,
    pushMessageToSendHistory,
    isPrompting,
    onSaveQueueEdit,
    onEnqueue,
    onSend,
    effectiveModeId,
    showModeSelector,
    effectiveDraftStorageKey,
  ])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (
        e.nativeEvent.isComposing ||
        composingRef.current ||
        e.key === "Process" ||
        e.keyCode === 229
      ) {
        return
      }

      if (slashMenuOpen && filteredSlashCommands.length > 0) {
        if (e.key === "ArrowDown") {
          e.preventDefault()
          setSlashSelectedIndex((i) =>
            i < filteredSlashCommands.length - 1 ? i + 1 : 0
          )
          return
        }
        if (e.key === "ArrowUp") {
          e.preventDefault()
          setSlashSelectedIndex((i) =>
            i > 0 ? i - 1 : filteredSlashCommands.length - 1
          )
          return
        }
        if (e.key === "Enter" || e.key === "Tab") {
          e.preventDefault()
          handleSlashSelect(filteredSlashCommands[slashSelectedIndex])
          return
        }
        if (e.key === "Escape") {
          e.preventDefault()
          setSlashMenuOpen(false)
          return
        }
      }

      if (
        !disabled &&
        !isEditingQueueItem &&
        !slashMenuOpen &&
        (e.key === "ArrowUp" || e.key === "ArrowDown") &&
        !e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        const textarea = e.currentTarget as HTMLTextAreaElement
        const value = textarea.value
        const history = sendHistoryRef.current

        if (history.length > 0) {
          const applyHistoryValue = (
            nextText: string,
            nextIndex: number | null
          ) => {
            e.preventDefault()
            setSendHistoryIndex(nextIndex)
            setText(nextText)
            requestAnimationFrame(() => {
              const el = textareaRef.current
              if (!el) return
              const pos = el.value.length
              el.selectionStart = el.selectionEnd = pos
            })
          }

          const atStart =
            textarea.selectionStart === 0 && textarea.selectionEnd === 0
          const atEnd =
            textarea.selectionStart === value.length &&
            textarea.selectionEnd === value.length

          if (e.key === "ArrowUp" && atStart) {
            if (sendHistoryIndex === null) {
              if (value.length === 0) {
                const nextIndex = history.length - 1
                applyHistoryValue(history[nextIndex] ?? "", nextIndex)
                return
              }
            } else {
              if (value.length === 0) {
                const nextIndex = history.length - 1
                applyHistoryValue(history[nextIndex] ?? "", nextIndex)
                return
              }

              const current = history[sendHistoryIndex]
              if (current && value === current) {
                const nextIndex = Math.max(0, sendHistoryIndex - 1)
                applyHistoryValue(history[nextIndex] ?? "", nextIndex)
                return
              }
            }
          }

          if (e.key === "ArrowDown" && atEnd && sendHistoryIndex !== null) {
            if (value.length === 0) {
              setSendHistoryIndex(null)
              return
            }

            const current = history[sendHistoryIndex]
            if (!current || value !== current) return

            const isLast = sendHistoryIndex >= history.length - 1
            if (isLast) {
              applyHistoryValue("", null)
              return
            }

            const nextIndex = sendHistoryIndex + 1
            applyHistoryValue(history[nextIndex] ?? "", nextIndex)
            return
          }
        }
      }

      if (isEditingQueueItem && e.key === "Escape") {
        e.preventDefault()
        onCancelQueueEdit?.()
        return
      }

      if (matchShortcutEvent(e, shortcuts.send_message)) {
        e.preventDefault()
        if (!disabled || isPrompting || isEditingQueueItem) handleSend()
      } else if (matchShortcutEvent(e, shortcuts.newline_in_message)) {
        e.preventDefault()
        const textarea = e.currentTarget as HTMLTextAreaElement
        const start = textarea.selectionStart
        const end = textarea.selectionEnd
        const value = textarea.value
        const newValue = value.substring(0, start) + "\n" + value.substring(end)
        setText(newValue)
        requestAnimationFrame(() => {
          textarea.selectionStart = textarea.selectionEnd = start + 1
        })
      }
    },
    [
      disabled,
      isPrompting,
      isEditingQueueItem,
      onCancelQueueEdit,
      handleSend,
      shortcuts,
      slashMenuOpen,
      filteredSlashCommands,
      slashSelectedIndex,
      handleSlashSelect,
      sendHistoryIndex,
    ]
  )

  const handleContainerDragOver = useCallback(
    (event: React.DragEvent<HTMLDivElement>) => {
      if (!hasDragFiles(event.dataTransfer)) return
      event.preventDefault()
      if (!disabled) {
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
      if (!hasDragFiles(event.dataTransfer)) return
      event.preventDefault()
      lastDomDropAtRef.current = Date.now()
      setDragActiveIfChanged(false)
      if (disabled) return
      const files = Array.from(event.dataTransfer.files ?? [])
      if (files.length > 0) {
        void appendFilesFromInput(files).catch((error) => {
          console.error("[MessageInput] drop files failed:", error)
        })
      }
    },
    [appendFilesFromInput, disabled, setDragActiveIfChanged]
  )

  const hasImageAttachments = imageAttachments.length > 0
  const hasResourceAttachments = resourceAttachments.length > 0
  const topPaddingClass =
    hasImageAttachments && hasResourceAttachments
      ? "pt-[6.25rem]"
      : hasImageAttachments
        ? "pt-[4.5rem]"
        : hasResourceAttachments
          ? "pt-10"
          : "pt-3"
  const bottomPaddingClass = "pb-10"
  const showDragActive = isDragActive && !disabled

  const selectorItems = (
    <>
      {showConfigLoading && (
        <SelectorLoadingChip label={t("loadingSettings")} />
      )}
      {hasConfigOptions &&
        availableConfigOptions.map((option) => (
          <SessionConfigSelector
            key={option.id}
            option={option}
            onSelect={(configId, valueId) =>
              onConfigOptionChange?.(configId, valueId)
            }
          />
        ))}
      {showModeLoading && <SelectorLoadingChip label={t("loadingMode")} />}
      {showModeSelector && (
        <ModeSelector
          modes={availableModes}
          selectedModeId={effectiveModeId!}
          onSelect={handleModeSelect}
        />
      )}
    </>
  )

  return (
    <div
      ref={containerRef}
      className="relative"
      onDragOver={handleContainerDragOver}
      onDragLeave={handleContainerDragLeave}
      onDrop={handleContainerDrop}
    >
      {slashMenuOpen && filteredSlashCommands.length > 0 && (
        <SlashCommandMenu
          commands={filteredSlashCommands}
          selectedIndex={slashSelectedIndex}
          onSelect={handleSlashSelect}
        />
      )}
      <Textarea
        ref={textareaRef}
        value={text}
        onChange={handleTextChange}
        onKeyDown={handleKeyDown}
        onCompositionStart={() => (composingRef.current = true)}
        onCompositionEnd={() => (composingRef.current = false)}
        onPaste={handlePaste}
        onFocus={onFocus}
        placeholder={resolvedPlaceholder}
        className={cn(
          "text-sm pr-12 resize-none bg-transparent",
          showDragActive && "ring-1 ring-primary/40",
          topPaddingClass,
          bottomPaddingClass,
          className
        )}
        autoFocus={autoFocus}
      />
      {(hasImageAttachments || hasResourceAttachments) && (
        <div className="absolute left-2 right-12 top-2 z-10 flex flex-col gap-1">
          {hasImageAttachments && (
            <div className="flex items-center gap-1 overflow-x-auto pb-0.5">
              {imageAttachments.map((attachment) => (
                <div
                  key={attachment.id}
                  className="relative shrink-0 overflow-hidden rounded-md border border-border/70 bg-muted/30"
                >
                  <Image
                    src={`data:${attachment.mimeType};base64,${attachment.data}`}
                    alt={attachment.name}
                    width={56}
                    height={56}
                    unoptimized
                    className="h-14 w-14 object-cover"
                  />
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
            </div>
          )}
          {hasResourceAttachments && (
            <div className="flex items-center gap-1 overflow-x-auto">
              {resourceAttachments.map((attachment) => (
                <div
                  key={attachment.id}
                  className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full border border-border/70 bg-muted/40 px-2 text-[11px] text-muted-foreground"
                >
                  <FileSearch className="h-3 w-3" />
                  <span className="max-w-40 truncate">{attachment.name}</span>
                  <button
                    type="button"
                    onClick={() => removeAttachment(attachment.id)}
                    className="rounded-sm p-0.5 hover:bg-muted-foreground/15"
                    aria-label={t("removeAttachmentAria", {
                      name: attachment.name,
                    })}
                  >
                    <X className="h-3 w-3" />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
      {showDragActive && (
        <div className="pointer-events-none absolute inset-1 z-20 flex items-center justify-center rounded-md border border-dashed border-primary/50 bg-background/80 text-xs text-muted-foreground">
          {t("dropFilesToAttach")}
        </div>
      )}
      <div className="@container absolute left-2 right-24 bottom-2">
        <div className="flex items-center gap-1">
          <Button
            onClick={handlePickFiles}
            disabled={disabled}
            variant="ghost"
            size="icon"
            className="h-6 w-6 shrink-0"
            title={t("attachFiles")}
          >
            <Plus className="size-4" />
          </Button>
          {/* 宽屏内联显示，窄屏（<300px）通过"更多"气泡显示 */}
          <div className="hidden @[300px]:contents">{selectorItems}</div>
          {hasAnySelector && (
            <Popover>
              <PopoverTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-6 w-6 shrink-0 @[300px]:hidden"
                >
                  <Ellipsis className="size-4" />
                </Button>
              </PopoverTrigger>
              <PopoverContent
                side="top"
                align="start"
                className="flex w-auto flex-col gap-1 rounded-xl p-1"
              >
                {selectorItems}
              </PopoverContent>
            </Popover>
          )}
        </div>
      </div>
      {isEditingQueueItem ? (
        <div className="absolute right-2 bottom-2 flex items-center gap-1">
          <Button
            onClick={onCancelQueueEdit}
            variant="ghost"
            size="icon"
            className="h-8 w-8"
            title={tQueue("cancelEdit")}
          >
            <X className="h-4 w-4" />
          </Button>
          <Button
            onClick={handleSend}
            disabled={!hasSendableContent}
            size="icon"
            title={tQueue("saveEdit")}
          >
            <Check className="h-4 w-4" />
          </Button>
        </div>
      ) : isPrompting && onCancel ? (
        <div className="absolute right-2 bottom-2 flex items-center gap-1">
          <Button
            onClick={handleSend}
            disabled={!hasSendableContent}
            variant="secondary"
            size="icon"
            className="h-8 w-8"
            title={tQueue("addToQueue")}
          >
            <ListPlus className="h-4 w-4" />
          </Button>
          <Button
            onClick={onCancel}
            variant="destructive"
            size="icon"
            title={t("cancel")}
          >
            <Square className="h-4 w-4" />
          </Button>
        </div>
      ) : (
        <Button
          onClick={handleSend}
          disabled={disabled || !hasSendableContent}
          size="icon"
          className="absolute right-2 bottom-2"
          title={t("send")}
        >
          <Send className="h-4 w-4" />
        </Button>
      )}
    </div>
  )
}
