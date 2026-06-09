"use client"

/**
 * Read-only inline view of the codeg-mcp `ask_user_question` tool in the message
 * stream (historical transcripts + the in-flight tool marker).
 *
 * The answered / declined record reuses the live `AskQuestionCard` in its
 * `readOnly` mode, so the layout (tabs, headers, option cards) stays identical
 * to the interactive card the user actually answered — it just renders the
 * selection as disabled and drops the footer. The Q&A is reconstructed from the
 * tool's raw input JSON + the companion's rendered result text (see
 * `@/lib/ask-question`). Error and in-flight states fall back to a compact
 * header card, since there is no answered selection to show yet.
 */

import { useMemo, type ReactNode } from "react"
import { useTranslations } from "next-intl"
import { Loader2, MessageCircleQuestionMark } from "lucide-react"

import { AskQuestionCard } from "@/components/chat/ask-question-card"
import { Badge } from "@/components/ui/badge"
import { cn } from "@/lib/utils"
import {
  matchSelections,
  parseAskQuestionInput,
  parseAskQuestionOutcome,
} from "@/lib/ask-question"
import type { PendingQuestionState } from "@/lib/types"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

// Separator for the composite "header + question" lookup key. A control char
// (unit separator) that never appears in agent-authored text.
const KEY_SEP = String.fromCharCode(31)
const NOOP = () => {}

interface Props {
  input?: string | null
  output?: string | null
  errorText?: string | null
  state?: ToolCallState
}

export function AskQuestionResultCard({
  input,
  output,
  errorText,
  state,
}: Props) {
  const t = useTranslations("Folder.chat.askQuestionResult")

  const questions = useMemo(() => parseAskQuestionInput(input), [input])
  const outcome = useMemo(() => parseAskQuestionOutcome(output), [output])

  const isError = !!errorText?.trim()
  const isRunning = state === "input-available" || state === "input-streaming"
  const isInFlight = !isError && !outcome && isRunning

  // The question set, shaped for AskQuestionCard (synthetic ids: the persisted
  // input carries none — the backend mints them server-side).
  const pending = useMemo<PendingQuestionState | null>(() => {
    if (questions.length === 0) return null
    return {
      question_id: "result",
      created_at: "",
      questions: questions.map((q, i) => ({
        id: `q${i}`,
        question: q.question,
        header: q.header,
        multi_select: q.multiSelect,
        options: q.options,
      })),
    }
  }, [questions])

  // Seed each question's selection by matching its answer text against the
  // offered option labels (option-aware, so a label containing ", " survives).
  const initialSelections = useMemo(() => {
    const sel: Record<string, { chosen: string[]; otherText: string }> = {}
    if (!pending || outcome?.declined) return sel
    const bySig = new Map(
      (outcome?.answers ?? []).map((a) => [
        `${a.header}${KEY_SEP}${a.question}`,
        a.selected,
      ])
    )
    pending.questions.forEach((q) => {
      const joined = bySig.get(`${q.header}${KEY_SEP}${q.question}`) ?? ""
      const { selected, other } = matchSelections(
        joined,
        q.options.map((o) => o.label)
      )
      sel[q.id] = { chosen: selected, otherText: other.join(", ") }
    })
    return sel
  }, [pending, outcome])

  // Compact header card for the states with no answered selection to render.
  const shell = (subtitle: string | null, body?: ReactNode) => (
    <div
      data-testid="ask-question-result-card"
      className={cn(
        "mb-2 overflow-hidden rounded-xl border bg-card",
        isError ? "border-destructive/30" : "border-primary/30"
      )}
    >
      <div className="flex flex-col gap-2 p-3">
        <div className="flex items-start gap-2.5">
          <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-primary">
            <MessageCircleQuestionMark className="size-4" />
          </span>
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium">{t("title")}</p>
            {subtitle && (
              <p className="text-xs text-muted-foreground">{subtitle}</p>
            )}
          </div>
          {isInFlight && (
            <Loader2 className="size-4 shrink-0 animate-spin text-muted-foreground" />
          )}
        </div>
        {body}
      </div>
    </div>
  )

  if (isError) {
    return shell(
      null,
      <p className="whitespace-pre-wrap text-xs text-destructive">
        {errorText?.trim()}
      </p>
    )
  }

  if (isInFlight) {
    return shell(
      t("awaiting"),
      questions.length > 0 ? (
        <div className="flex flex-wrap gap-1.5">
          {questions.map((q, i) => (
            <Badge key={i} variant="outline" className="text-[10px]">
              {q.header || q.question}
            </Badge>
          ))}
        </div>
      ) : undefined
    )
  }

  // Input didn't parse (e.g. a truncated transcript) but the result text did:
  // show the answers as chips so the record isn't lost.
  if (!pending) {
    const answers = outcome?.answers ?? []
    if (answers.length === 0) return null
    return shell(
      outcome?.declined ? t("declined") : null,
      <div className="space-y-2">
        {answers.map((a, i) => {
          const labels = matchSelections(a.selected, []).other
          return (
            <div key={i} className="space-y-1">
              {a.question && (
                <p className="text-xs text-foreground/90">{a.question}</p>
              )}
              {labels.length > 0 ? (
                <div className="flex flex-wrap gap-1.5">
                  {labels.map((label) => (
                    <Badge key={label} className="text-xs">
                      {label}
                    </Badge>
                  ))}
                </div>
              ) : (
                <span className="text-xs text-muted-foreground">
                  {t("noSelection")}
                </span>
              )}
            </div>
          )
        })}
      </div>
    )
  }

  // Answered / declined: reuse the live card so the layout matches exactly.
  return (
    <AskQuestionCard
      readOnly
      question={pending}
      onAnswer={NOOP}
      initialSelections={initialSelections}
      title={t("title")}
      subtitle={outcome?.declined ? t("declined") : ""}
    />
  )
}
