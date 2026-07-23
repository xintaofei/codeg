"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react"
import { BookOpenText } from "lucide-react"
import type { RichComposerHandle } from "@/components/chat/composer/rich-composer"
import {
  commandToReference,
  skillToReference,
} from "@/components/chat/composer/invocation-reference"
import type { ReferenceAttrs } from "@/components/chat/composer/types"
import { useAgentSkills } from "@/hooks/use-agent-skills"
import { rankByTextMatch } from "@/lib/fuzzy-text-match"
import { cn } from "@/lib/utils"
import type {
  AgentSkillItem,
  AgentType,
  AvailableCommandInfo,
} from "@/lib/types"

interface UseComposerInvocationsArgs {
  editorRef: RefObject<RichComposerHandle | null>
  agentType: AgentType
  /** Folder path for project-scoped Codex skills (global skills load regardless). */
  folderPath: string | null
  /** Slash commands from the agent-options probe (empty if none / not yet ready). */
  availableCommands: AvailableCommandInfo[]
}

export interface ComposerInvocations {
  /** True only when the menu is open AND has at least one item to show. */
  isOpen: boolean
  commands: AvailableCommandInfo[]
  skills: AgentSkillItem[]
  /** Index into the merged [commands, skills] list. */
  activeIndex: number
  /** Re-evaluate the trigger from the editor's current caret (call on change). */
  detect: () => void
  /** Routed from RichComposer while `isExternalMenuOpen`; returns true if handled. */
  onKeyDown: (event: KeyboardEvent) => boolean
  selectCommand: (cmd: AvailableCommandInfo) => void
  selectSkill: (skill: AgentSkillItem) => void
}

/**
 * A self-contained `/` (slash commands) + `$` (Codex skills) autocomplete for the
 * automation editor's `RichComposer`, mirroring the chat composer's parent-owned
 * menu (`message-input.tsx`) but isolated here so the central chat input stays
 * untouched. It reuses the shared, pure reference builders + the composer's
 * `isExternalMenuOpen`/`onExternalMenuKeyDown` escape hatch; a selected item
 * becomes a badge that serializes to its literal `/cmd` / `$skill` token on save.
 */
export function useComposerInvocations({
  editorRef,
  agentType,
  folderPath,
  availableCommands,
}: UseComposerInvocationsArgs): ComposerInvocations {
  const isCodex = agentType === "codex"
  // Codex-only `$` skills (filesystem scan — no live session needed).
  const skills = useAgentSkills(isCodex ? "codex" : null, folderPath)
  const [open, setOpen] = useState(false)
  const [triggerChar, setTriggerChar] = useState<"/" | "$" | null>(null)
  const [filter, setFilter] = useState("")
  const [rawActiveIndex, setRawActiveIndex] = useState(0)

  const close = useCallback(() => {
    setOpen(false)
    setTriggerChar(null)
  }, [])

  // Inspect the text before the collapsed caret: a `/` (any agent) or `$`
  // (Codex) at the start or right after whitespace, not inside code, opens the
  // menu — same rule as the chat composer's detectSlashTrigger.
  const detect = useCallback(() => {
    const editor = editorRef.current?.getEditor()
    const hasSource = availableCommands.length > 0 || skills.length > 0
    if (!editor || !hasSource) return close()
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
    const regex = isCodex ? /(^|\s)([/$])(\S*)$/ : /(^|\s)(\/)(\S*)$/
    const match = before.match(regex)
    if (!match) return close()
    setTriggerChar(match[2] as "/" | "$")
    setFilter(match[3])
    setRawActiveIndex(0)
    setOpen(true)
  }, [editorRef, availableCommands.length, skills.length, isCodex, close])

  const commands = useMemo(() => {
    if (!open || triggerChar !== "/" || availableCommands.length === 0)
      return []
    return rankByTextMatch(filter, availableCommands, (c) => c.name)
  }, [open, triggerChar, availableCommands, filter])

  const matchedSkills = useMemo(() => {
    // Skills autocomplete is Codex-only and triggered by `$`.
    if (!isCodex || !open || triggerChar !== "$" || skills.length === 0)
      return []
    return rankByTextMatch(
      filter,
      skills,
      (skill) => skill.name,
      (skill) => skill.id
    )
  }, [isCodex, open, triggerChar, skills, filter])

  const count = commands.length + matchedSkills.length
  // Clamp on read so a shrinking filtered list never points past the end (avoids
  // a clamping effect / set-state-in-effect).
  const activeIndex = count > 0 ? Math.min(rawActiveIndex, count - 1) : 0

  // Replace the live `/…` / `$…` token before the caret with an inline badge
  // (+ trailing space unless one follows), then close. The badge serializes back
  // to its literal token on save (referenceToMarkdown).
  const replaceTrigger = useCallback(
    (ref: ReferenceAttrs) => {
      const editor = editorRef.current?.getEditor()
      if (!editor) return close()
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
        const tokenLen = match[2].length + match[3].length
        chain = chain.deleteRange({ from: $from.pos - tokenLen, to: $from.pos })
      }
      chain = chain.insertReference(ref)
      if (suffix) chain = chain.insertContent(suffix)
      chain.run()
      close()
    },
    [editorRef, close]
  )

  const selectCommand = useCallback(
    (cmd: AvailableCommandInfo) => replaceTrigger(commandToReference(cmd)),
    [replaceTrigger]
  )
  const selectSkill = useCallback(
    (skill: AgentSkillItem) => replaceTrigger(skillToReference(skill, "$")),
    [replaceTrigger]
  )

  const onKeyDown = useCallback(
    (event: KeyboardEvent): boolean => {
      if (event.isComposing) return false
      if (!open || count === 0) return false
      if (event.key === "ArrowDown") {
        setRawActiveIndex((i) => (i < count - 1 ? i + 1 : 0))
        return true
      }
      if (event.key === "ArrowUp") {
        setRawActiveIndex((i) => (i > 0 ? i - 1 : count - 1))
        return true
      }
      if (event.key === "Enter" || event.key === "Tab") {
        if (activeIndex < commands.length) selectCommand(commands[activeIndex])
        else {
          const skill = matchedSkills[activeIndex - commands.length]
          if (skill) selectSkill(skill)
        }
        return true
      }
      if (event.key === "Escape") {
        close()
        return true
      }
      return false
    },
    [
      open,
      count,
      activeIndex,
      commands,
      matchedSkills,
      selectCommand,
      selectSkill,
      close,
    ]
  )

  return {
    isOpen: open && count > 0,
    commands,
    skills: matchedSkills,
    activeIndex,
    detect,
    onKeyDown,
    selectCommand,
    selectSkill,
  }
}

/**
 * The floating list for {@link useComposerInvocations}. Render inside a
 * `relative` wrapper around the composer; it anchors below the box (the editor
 * sits near the top of the scrollable form, so opening upward gets clipped by the
 * ScrollArea). Navigation is routed from the editor's keydown, so this only
 * handles pointer selection.
 */
export function ComposerInvocationsPopup({
  inv,
}: {
  inv: ComposerInvocations
}) {
  const listRef = useRef<HTMLDivElement>(null)

  // Keep the active row in view as the user arrows through (manual scrollTop,
  // mirroring the chat composer — no scrollIntoView, which jsdom lacks).
  useEffect(() => {
    if (!inv.isOpen) return
    const container = listRef.current
    const el = container?.children[inv.activeIndex] as HTMLElement | undefined
    if (!container || !el) return
    const elTop = el.offsetTop
    const elBottom = elTop + el.offsetHeight
    const viewTop = container.scrollTop
    const viewBottom = viewTop + container.clientHeight
    if (elTop < viewTop) container.scrollTop = elTop
    else if (elBottom > viewBottom)
      container.scrollTop = elBottom - container.clientHeight
  }, [inv.isOpen, inv.activeIndex])

  if (!inv.isOpen) return null

  return (
    <div className="absolute left-0 right-0 top-full z-50 mt-1 flex max-h-[min(16rem,40dvh)] flex-col overflow-hidden rounded-xl border border-border bg-popover shadow-lg">
      <div ref={listRef} className="flex-1 overflow-y-auto p-1">
        {inv.commands.map((cmd, i) => (
          <button
            key={`cmd-${cmd.name}`}
            type="button"
            className={cn(
              "flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-sm",
              i === inv.activeIndex
                ? "bg-accent text-accent-foreground"
                : "hover:bg-muted"
            )}
            onMouseDown={(e) => {
              e.preventDefault()
              inv.selectCommand(cmd)
            }}
          >
            <span className="shrink-0 font-mono text-primary">/{cmd.name}</span>
            <span className="truncate text-xs text-muted-foreground">
              {cmd.description}
            </span>
          </button>
        ))}
        {inv.skills.map((skill, i) => {
          const absoluteIndex = inv.commands.length + i
          return (
            <button
              key={`skill-${skill.scope}-${skill.id}`}
              type="button"
              className={cn(
                "flex w-full items-start gap-2 rounded-lg px-3 py-2 text-left text-sm",
                absoluteIndex === inv.activeIndex
                  ? "bg-accent text-accent-foreground"
                  : "hover:bg-muted"
              )}
              onMouseDown={(e) => {
                e.preventDefault()
                inv.selectSkill(skill)
              }}
            >
              <BookOpenText className="mt-0.5 size-4 shrink-0 text-primary/80" />
              <div className="flex min-w-0 flex-1 items-center gap-2">
                <span className="shrink-0 font-medium">{skill.name}</span>
                <span
                  className="min-w-0 flex-1 truncate text-xs text-muted-foreground"
                  title={skill.description ?? undefined}
                >
                  {skill.description ?? `$${skill.id}`}
                </span>
              </div>
            </button>
          )
        })}
      </div>
    </div>
  )
}
