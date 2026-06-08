import { type ReactElement } from "react"
import { render } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi, beforeEach } from "vitest"

import { SidebarConversationCard } from "./sidebar-conversation-card"
import { formatRelative } from "./sidebar-conversation-grouping"
import type { DbConversationSummary } from "@/lib/types"
import enMessages from "@/i18n/messages/en.json"

// AgentIcon renders exactly once per card body execution, so counting its
// renders counts how many cards actually re-rendered (a card that bails out via
// memo never re-runs its body, hence never re-renders AgentIcon). Cheap leaf →
// easy, unambiguous render probe.
const probe = vi.hoisted(() => ({ agentIconRenders: 0 }))
vi.mock("@/components/agent-icon", () => ({
  AgentIcon: () => {
    probe.agentIconRenders++
    return null
  },
}))

const MINUTE = 60_000
const NOW = 1_700_000_000_000

// Stable callback identities shared across renders — the production list hands
// memoized callbacks down, so the test must too.
const onSelect = vi.fn()
const onDoubleClick = vi.fn()
const onRename = vi.fn(async () => {})
const onDelete = vi.fn(async () => {})
const onStatusChange = vi.fn(async () => {})

function conv(id: number): DbConversationSummary {
  // 5 minutes ago → label "5m"; one extra minute later it ages to "6m".
  const createdAt = new Date(NOW - 5 * MINUTE).toISOString()
  return {
    id,
    folder_id: 1,
    title: `conv-${id}`,
    agent_type: "claude_code",
    status: "pending",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 0,
    created_at: createdAt,
    updated_at: createdAt,
  }
}

function CardList({
  conversations,
  now,
  select = onSelect,
}: {
  conversations: DbConversationSummary[]
  now: number
  select?: (id: number, agentType: string, folderId: number) => void
}) {
  return (
    <>
      {conversations.map((c) => (
        <SidebarConversationCard
          key={c.id}
          conversation={c}
          isSelected={false}
          isOpenInTab={false}
          timeLabel={formatRelative(c.created_at, now)}
          onSelect={select}
          onDoubleClick={onDoubleClick}
          onRename={onRename}
          onDelete={onDelete}
          onStatusChange={onStatusChange}
        />
      ))}
    </>
  )
}

function renderWithIntl(ui: ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

const BASE = [conv(1), conv(2), conv(3), conv(4), conv(5)]

describe("SidebarConversationCard memo (sidebar perf Phase 1 gate)", () => {
  beforeEach(() => {
    probe.agentIconRenders = 0
  })

  it("re-renders only the card whose summary object changed", () => {
    const { rerender } = renderWithIntl(
      <CardList conversations={BASE} now={NOW} />
    )

    // Control: an identical re-render must bail out for every card.
    probe.agentIconRenders = 0
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CardList conversations={BASE} now={NOW} />
      </NextIntlClientProvider>
    )
    expect(probe.agentIconRenders).toBe(0)

    // Replace exactly one summary (new object ref) — mirrors a single
    // `conversation_status_changed` patch in updateConversationLocal.
    const next = BASE.slice()
    next[2] = { ...BASE[2], status: "completed" }

    probe.agentIconRenders = 0
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CardList conversations={next} now={NOW} />
      </NextIntlClientProvider>
    )
    expect(probe.agentIconRenders).toBe(1)
  })

  it("re-renders all cards (only) once per minute as the shared now advances", () => {
    const { rerender } = renderWithIntl(
      <CardList conversations={BASE} now={NOW} />
    )

    // Advancing the shared `now` past a unit boundary ages every label
    // "5m" → "6m", so every card re-renders — but just this once. This is the
    // bounded cost that justifies threading a single `now` instead of letting
    // each row read Date.now() on every unrelated render.
    probe.agentIconRenders = 0
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CardList conversations={BASE} now={NOW + MINUTE} />
      </NextIntlClientProvider>
    )
    expect(probe.agentIconRenders).toBe(BASE.length)
  })

  it("re-renders every card when callback identity is unstable (defeats memo)", () => {
    const { rerender } = renderWithIntl(
      <CardList conversations={BASE} now={NOW} select={() => {}} />
    )

    // A fresh onSelect each render is exactly the R1b regression: stable
    // conversations + stable now, yet every card re-renders.
    probe.agentIconRenders = 0
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CardList conversations={BASE} now={NOW} select={() => {}} />
      </NextIntlClientProvider>
    )
    expect(probe.agentIconRenders).toBe(BASE.length)
  })
})
