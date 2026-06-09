"use client"

/**
 * Read-only inline card for the codeg-mcp `ask_user_question` tool as it appears
 * in the message stream (historical transcripts + the in-flight tool marker).
 *
 * The live, interactive answering happens in `AskQuestionCard`, pinned below the
 * stream and driven by the `question_request` event — this card never collects
 * an answer. It mirrors that card's visual language but only *displays* what was
 * asked and which option(s) the user picked, reconstructing the Q&A from the
 * tool's raw input JSON + the companion's rendered result text (see
 * `@/lib/ask-question`).
 */

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import { Check, Loader2, MessageCircleQuestionMark } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { cn } from "@/lib/utils"
import {
  parseAskQuestionInput,
  parseAskQuestionOutcome,
  splitRecommended,
  type AskQuestion,
} from "@/lib/ask-question"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

// Separator for the composite "header + question" Map key. A control char (unit
// separator) that never appears in agent-authored text, so the join can't be
// ambiguous between two different (header, question) pairs.
const KEY_SEP = String.fromCharCode(31)

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

  // Map each answered block back to its question by (header, question) text.
  // The backend emits answers in asked order but drops unanswered questions, so
  // a positional zip would misalign — keying on the text is robust to drops.
  const selectedByKey = useMemo(() => {
    const m = new Map<string, string[]>()
    for (const a of outcome?.answers ?? []) {
      m.set(`${a.header}${KEY_SEP}${a.question}`, a.selected)
    }
    return m
  }, [outcome])

  // Fall back to the answered blocks when the input JSON didn't parse (e.g. a
  // truncated historical transcript) so the card still shows what was asked.
  const displayQuestions = useMemo<AskQuestion[]>(() => {
    if (questions.length > 0) return questions
    return (outcome?.answers ?? []).map((a) => ({
      question: a.question,
      header: a.header,
      multiSelect: false,
      options: [],
    }))
  }, [questions, outcome])

  const isError = !!errorText?.trim()
  // Still blocking on the pinned interactive card: the tool is running and no
  // result text has arrived yet (`outcome` is null only for empty output).
  const isRunning = state === "input-available" || state === "input-streaming"
  const isInFlight = !isError && !outcome && isRunning
  const isDeclined = !!outcome?.declined

  const subtitle = isInFlight
    ? t("awaiting")
    : isDeclined
      ? t("declined")
      : null

  const renderQuestion = (q: AskQuestion, idx: number) => {
    const selected =
      selectedByKey.get(`${q.header}${KEY_SEP}${q.question}`) ?? []
    const selectedSet = new Set(selected)
    // Selected labels that aren't one of the offered options = free-text "Other".
    const otherCustoms = selected.filter(
      (s) => !q.options.some((o) => o.label === s)
    )
    // Declined: show what was asked, but highlight nothing.
    const highlight = !isDeclined

    return (
      <div key={idx} className="space-y-1.5">
        <div className="flex items-center gap-2">
          <Badge variant="outline" className="shrink-0 text-[10px]">
            {q.multiSelect ? t("multiple") : t("single")}
          </Badge>
          {q.question && (
            <p className="text-sm text-foreground/90">{q.question}</p>
          )}
        </div>

        {q.options.length > 0 ? (
          <div className="space-y-1">
            {q.options.map((opt) => {
              const isSel = highlight && selectedSet.has(opt.label)
              const { text, recommended } = splitRecommended(opt.label)
              return (
                <div
                  key={opt.label}
                  data-selected={isSel ? "true" : "false"}
                  className={optionRowClass(isSel)}
                >
                  <span className="mt-0.5 flex size-4 shrink-0 items-center justify-center">
                    {isSel ? (
                      <Check className="size-3.5 text-primary" />
                    ) : (
                      <span className="size-1.5 rounded-full bg-muted-foreground/30" />
                    )}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-1.5 text-sm">
                      <span
                        className={
                          isSel ? "font-medium" : "text-muted-foreground"
                        }
                      >
                        {text}
                      </span>
                      {recommended && (
                        <Badge variant="secondary" className="text-[10px]">
                          {t("recommended")}
                        </Badge>
                      )}
                    </span>
                    {opt.description && (
                      <span className="mt-0.5 block text-xs text-muted-foreground">
                        {opt.description}
                      </span>
                    )}
                  </span>
                </div>
              )
            })}
            {highlight &&
              otherCustoms.map((label) => (
                <div
                  key={`other-${label}`}
                  data-selected="true"
                  className={optionRowClass(true)}
                >
                  <Check className="mt-0.5 size-3.5 shrink-0 text-primary" />
                  <span className="min-w-0 flex-1 text-sm font-medium">
                    {label}
                    <Badge variant="outline" className="ml-1.5 text-[10px]">
                      {t("other")}
                    </Badge>
                  </span>
                </div>
              ))}
          </div>
        ) : (
          // No option metadata (a pseudo-question rebuilt from the result text):
          // show the selected labels as chips.
          <div className="flex flex-wrap gap-1.5">
            {!highlight || selected.length === 0 ? (
              <span className="text-xs text-muted-foreground">
                {t("noSelection")}
              </span>
            ) : (
              selected.map((label) => (
                <Badge key={label} className="text-xs">
                  {splitRecommended(label).text}
                </Badge>
              ))
            )}
          </div>
        )}
      </div>
    )
  }

  return (
    <div
      data-testid="ask-question-result-card"
      className={cn(
        "mb-2 overflow-hidden rounded-xl border bg-card",
        isError ? "border-destructive/30" : "border-primary/30"
      )}
    >
      <div className="flex items-start gap-2.5 p-3 pb-0">
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

      <div className="space-y-3 p-3">
        {isError && (
          <p className="whitespace-pre-wrap text-xs text-destructive">
            {errorText?.trim()}
          </p>
        )}
        {isInFlight ? (
          displayQuestions.length > 0 && (
            <div className="flex flex-wrap gap-1.5">
              {displayQuestions.map((q, i) => (
                <Badge key={i} variant="outline" className="text-[10px]">
                  {q.header || q.question}
                </Badge>
              ))}
            </div>
          )
        ) : (
          <>{displayQuestions.map((q, i) => renderQuestion(q, i))}</>
        )}
      </div>
    </div>
  )
}

const optionRowClass = (selected: boolean) =>
  cn(
    "flex items-start gap-2 rounded-lg border p-2 transition-colors",
    selected ? "border-primary bg-primary/10" : "border-border/40"
  )
