import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { DagGraph } from "./dag-graph"
import type { AttentionKey } from "@/lib/loop-attention"
import type {
  ArtifactIterationRef,
  LoopArtifactRow,
  LoopInboxItemRow,
  LoopIterationRow,
  LoopLinkKind,
  LoopLinkRow,
  LoopStage,
} from "@/lib/types"

// Echoing translator across every namespace the graph uses. Counts therefore
// can't be read from translated text (the mock drops ICU params) — connector
// counts are asserted via the structured data-total/active/hidden attributes.
vi.mock("next-intl", () => ({ useTranslations: () => (k: string) => k }))

function art(
  over: Partial<LoopArtifactRow> & { id: number; kind: LoopArtifactRow["kind"] }
): LoopArtifactRow {
  return {
    issue_id: 1,
    issue_seq: 1,
    title: "T",
    status: "done",
    origin: "agent",
    produced_by_iteration_id: null,
    verdict: null,
    contribution_kind: "delta",
    attempt: 0,
    sort: 0,
    updated_at: "2026-06-17T00:00:00Z",
    ...over,
  }
}

function link(
  id: number,
  from: number,
  to: number,
  kind: LoopLinkKind
): LoopLinkRow {
  return {
    id,
    from_artifact_id: from,
    to_artifact_id: to,
    kind,
    source_revision_id: null,
  }
}

function iter(
  id: number,
  stage: LoopStage,
  over: Partial<LoopIterationRow> = {}
): LoopIterationRow {
  return {
    id,
    issue_id: 1,
    issue_seq: id,
    stage,
    target_artifact_id: null,
    target_title: null,
    conversation_id: null,
    status: "running",
    launched_by: "engine",
    attempt: 0,
    tokens_used: 0,
    outcome: null,
    created_at: "2026-06-17T00:00:00Z",
    started_at: "2026-06-17T00:00:00Z",
    ended_at: null,
    ...over,
  }
}

const scrollIntoView = vi.fn()

beforeEach(() => {
  vi.clearAllMocks()
  // jsdom has no layout engine; the focus effect calls scrollIntoView.
  Element.prototype.scrollIntoView = scrollIntoView
})

const base = {
  links: [],
  liveIterations: [],
  executingIds: new Set<string>(),
  onSelect: () => {},
}

describe("DagGraph focus replay (Codex r1)", () => {
  it("scrolls to and consumes a focus whose node is present", () => {
    const onFocusConsumed = vi.fn()
    render(
      <DagGraph
        {...base}
        artifacts={[art({ id: 1, kind: "issue", title: "Root" })]}
        focus={1}
        onFocusConsumed={onFocusConsumed}
      />
    )
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
    expect(onFocusConsumed).toHaveBeenCalledTimes(1)
  })

  it("consumes (without scrolling) a focus whose node is absent once the graph has data", () => {
    const onFocusConsumed = vi.fn()
    render(
      <DagGraph
        {...base}
        artifacts={[art({ id: 1, kind: "issue", title: "Root" })]}
        focus={999}
        onFocusConsumed={onFocusConsumed}
      />
    )
    expect(scrollIntoView).not.toHaveBeenCalled()
    // Layout is ready but the target is gone → consume so it can't pulse later.
    expect(onFocusConsumed).toHaveBeenCalledTimes(1)
  })

  it("keeps an unresolved focus while the graph is still empty (replay on data)", () => {
    const onFocusConsumed = vi.fn()
    render(
      <DagGraph
        {...base}
        artifacts={[]}
        focus={999}
        onFocusConsumed={onFocusConsumed}
      />
    )
    expect(onFocusConsumed).not.toHaveBeenCalled()
  })
})

describe("DagGraph issue-level attention (Codex r2)", () => {
  const card = { id: 1 } as unknown as LoopInboxItemRow

  it("rings the issue root node for issue-level (issue-root) cards", () => {
    const attentionMap = new Map<AttentionKey, LoopInboxItemRow[]>([
      ["issue-root", [card]],
    ])
    render(
      <DagGraph
        {...base}
        artifacts={[art({ id: 1, kind: "issue", title: "Root" })]}
        attentionMap={attentionMap}
      />
    )
    // The echoing translator renders the attention label as "attention", so an
    // attentioned node's accessible name carries the " — attention" suffix.
    expect(screen.getByLabelText("issue: Root — attention")).toBeInTheDocument()
  })

  it("leaves the issue root unmarked when there are no issue-level cards", () => {
    render(
      <DagGraph
        {...base}
        artifacts={[art({ id: 1, kind: "issue", title: "Root" })]}
        attentionMap={new Map()}
      />
    )
    expect(screen.getByLabelText("issue: Root")).toBeInTheDocument()
    expect(
      screen.queryByLabelText("issue: Root — attention")
    ).not.toBeInTheDocument()
  })
})

// ---------------------------------------------------------------------------
// P2: six-phase containers + folded connectors (the de-spaghetti render).
// ---------------------------------------------------------------------------

describe("DagGraph phase containers", () => {
  it("renders all six phase containers, with unreached phases as placeholders", () => {
    const { container } = render(
      <DagGraph
        {...base}
        artifacts={[art({ id: 1, kind: "issue", title: "Root" })]}
      />
    )
    const phases = [...container.querySelectorAll("[data-phase]")].map((el) =>
      el.getAttribute("data-phase")
    )
    expect(new Set(phases)).toEqual(
      new Set([
        "issue",
        "requirement",
        "design",
        "implement",
        "result",
        "reflect",
      ])
    )
    // Issue has a member → solid; the rest are empty → slim placeholders.
    expect(container.querySelector('[data-phase="issue"]')).not.toHaveAttribute(
      "data-placeholder"
    )
    expect(container.querySelector('[data-phase="design"]')).toHaveAttribute(
      "data-placeholder",
      "true"
    )
    expect(container.querySelector('[data-phase="result"]')).toHaveAttribute(
      "data-placeholder",
      "true"
    )
  })
})

// Golden fixture = issue #1: 1 issue + 12 requirements + 1 design + 10 tasks +
// 1 review, with the 34 lineage edges that must fold into 3 connectors.
function issueOneFixture(): {
  artifacts: LoopArtifactRow[]
  links: LoopLinkRow[]
} {
  const ISSUE = 1
  const REQ_IDS = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]
  const DESIGN = 14
  const TASK_IDS = [15, 16, 17, 18, 19, 20, 21, 22, 23, 24]
  const REVIEW = 25
  const artifacts: LoopArtifactRow[] = [
    art({ id: ISSUE, kind: "issue", status: "done" }),
    ...REQ_IDS.map((id) => art({ id, kind: "requirement", status: "done" })),
    art({ id: DESIGN, kind: "design", status: "done" }),
    ...TASK_IDS.map((id, i) =>
      art({
        id,
        kind: "task",
        status: i === 0 ? "in_progress" : "done",
        sort: id,
      })
    ),
    art({ id: REVIEW, kind: "review", status: "done", verdict: "pass" }),
  ]
  let linkId = 0
  const links: LoopLinkRow[] = []
  for (const r of REQ_IDS) links.push(link(++linkId, r, ISSUE, "derives_from"))
  for (const r of REQ_IDS) links.push(link(++linkId, DESIGN, r, "derives_from"))
  for (const t of TASK_IDS)
    links.push(link(++linkId, t, DESIGN, "derives_from"))
  for (let i = 2; i < TASK_IDS.length; i++)
    links.push(link(++linkId, TASK_IDS[i], TASK_IDS[i - 1], "depends_on"))
  links.push(link(++linkId, REVIEW, 16, "reviews"))
  return { artifacts, links }
}

describe("DagGraph folded connectors", () => {
  it("folds issue #1's 34 lineage edges into 3 connectors (12/12/10)", () => {
    const { artifacts, links } = issueOneFixture()
    const { container } = render(
      <DagGraph {...base} artifacts={artifacts} links={links} />
    )
    const connectors = [...container.querySelectorAll("[data-connector]")]
    expect(connectors).toHaveLength(3)
    const total = (sel: string) =>
      container.querySelector(sel)?.getAttribute("data-total")
    expect(total('[data-connector="issue->requirement:lineage"]')).toBe("12")
    expect(total('[data-connector="requirement->design:lineage"]')).toBe("12")
    expect(total('[data-connector="design->implement:lineage"]')).toBe("10")
  })

  it("routes a skip connector around the empty phase it crosses (not hung on it)", () => {
    // skip_design route: a task both derives_from and skips_to the requirement,
    // leaving design empty. The skip must appear as its own dashed connector.
    const { container } = render(
      <DagGraph
        {...base}
        artifacts={[
          art({ id: 1, kind: "issue" }),
          art({ id: 2, kind: "requirement" }),
          art({ id: 10, kind: "task", status: "in_progress" }),
        ]}
        links={[link(1, 10, 2, "derives_from"), link(2, 10, 2, "skips_to")]}
      />
    )
    const skip = container.querySelector(
      '[data-connector="requirement->implement:skip"]'
    )
    expect(skip).not.toBeNull()
    expect(skip).toHaveAttribute("data-skip", "true")
    // Design is crossed but holds nothing → placeholder, and no connector touches it.
    expect(container.querySelector('[data-phase="design"]')).toHaveAttribute(
      "data-placeholder",
      "true"
    )
    expect(container.querySelector('[data-connector*="design"]')).toBeNull()
  })

  it("hides a zero-active connector by default and reveals it (with counts) on toggle", () => {
    // design derives_from a superseded requirement → total 1, active 0.
    const artifacts = [
      art({ id: 14, kind: "design", status: "done" }),
      art({ id: 3, kind: "requirement", status: "superseded" }),
    ]
    const links = [link(1, 14, 3, "derives_from")]
    const { container } = render(
      <DagGraph {...base} artifacts={artifacts} links={links} />
    )
    const sel = '[data-connector="requirement->design:lineage"]'
    // Off: activeCount 0 → not drawn at all.
    expect(container.querySelector(sel)).toBeNull()
    // Reveal superseded.
    fireEvent.click(container.querySelector("button[aria-pressed]")!)
    const conn = container.querySelector(sel)
    expect(conn).not.toBeNull()
    expect(conn).toHaveAttribute("data-total", "1")
    expect(conn).toHaveAttribute("data-active", "0")
    expect(conn).toHaveAttribute("data-hidden", "1")
  })
})

describe("DagGraph superseded toggle", () => {
  it("renders an all-dead phase as a placeholder, then solid + dimmed on reveal", () => {
    const artifacts = [
      art({ id: 1, kind: "issue", status: "done" }),
      art({ id: 2, kind: "requirement", status: "superseded" }),
    ]
    const { container } = render(<DagGraph {...base} artifacts={artifacts} />)
    // Off: the requirement phase has only a dead member → placeholder, no card.
    expect(
      container.querySelector('[data-phase="requirement"]')
    ).toHaveAttribute("data-placeholder", "true")
    expect(container.querySelector('[data-artifact-id="2"]')).toBeNull()
    // Reveal: the phase goes solid and the dead member renders dimmed.
    fireEvent.click(container.querySelector("button[aria-pressed]")!)
    expect(
      container.querySelector('[data-phase="requirement"]')
    ).not.toHaveAttribute("data-placeholder")
    const card = container.querySelector('[data-artifact-id="2"]')
    expect(card).not.toBeNull()
    expect(card?.className).toContain("opacity-50")
  })
})

describe("DagGraph non-Implement pending ghosts", () => {
  it("renders refine/design/reflect ghosts in their own phases", () => {
    render(
      <DagGraph
        {...base}
        artifacts={[art({ id: 1, kind: "issue" })]}
        liveIterations={[
          iter(101, "refine", { status: "running" }),
          iter(102, "design", { status: "running" }),
          iter(103, "reflect", { status: "queued" }),
        ]}
      />
    )
    // Ghost label = `${kind}: ${status}` under the echoing translator.
    expect(screen.getByLabelText("requirement: running")).toBeInTheDocument()
    expect(screen.getByLabelText("design: running")).toBeInTheDocument()
    expect(screen.getByLabelText("reflection: queued")).toBeInTheDocument()
  })
})

describe("DagGraph reviews are first-class folded nodes", () => {
  const reviewFixture = {
    artifacts: [
      art({ id: 10, kind: "task", status: "in_progress" }),
      art({
        id: 20,
        kind: "review",
        status: "done",
        verdict: "pass",
        title: "R",
      }),
    ],
    links: [link(1, 20, 10, "reviews")],
  }

  it("gives a folded review its own data-artifact-id and locate target", () => {
    const onFocusConsumed = vi.fn()
    const { container } = render(
      <DagGraph
        {...base}
        artifacts={reviewFixture.artifacts}
        links={reviewFixture.links}
        focus={20}
        onFocusConsumed={onFocusConsumed}
      />
    )
    expect(container.querySelector('[data-artifact-id="20"]')).not.toBeNull()
    // Locate resolves to the review node → scroll + consume.
    expect(scrollIntoView).toHaveBeenCalledTimes(1)
    expect(onFocusConsumed).toHaveBeenCalledTimes(1)
  })

  it("selects the review (not its task) when its row is clicked", () => {
    const onSelect = vi.fn()
    const { container } = render(
      <DagGraph
        {...base}
        artifacts={reviewFixture.artifacts}
        links={reviewFixture.links}
        onSelect={onSelect}
      />
    )
    fireEvent.click(container.querySelector('[data-artifact-id="20"]')!)
    expect(onSelect).toHaveBeenCalledWith(20)
  })
})

describe("DagGraph agent facet (P3)", () => {
  // The echoing mock returns the key for every translation, so the agent icon's
  // presence is asserted via the accessible name carrying "agentRunBy" (pushed in
  // only when an agent resolves) and the attempt badge via its literal "×N".
  it("shows the agent icon (aria) + attempt badge on a node when the facet is on", () => {
    const refs: ArtifactIterationRef[] = [
      {
        artifact_id: 2,
        iteration_id: 5,
        stage: "refine",
        status: "succeeded",
        outcome: "succeeded",
        agent_type: "codex",
        conversation_id: 9,
        attempt_count: 3,
      },
    ]
    render(
      <DagGraph
        {...base}
        artifacts={[
          art({
            id: 2,
            kind: "requirement",
            title: "Req",
            produced_by_iteration_id: 5,
          }),
        ]}
        artifactIterationRefs={refs}
      />
    )
    expect(screen.getByRole("button", { name: /agentRunBy/ })).toBeTruthy()
    expect(screen.getByText("×3")).toBeTruthy()
  })

  it("renders the agent badge (with attempts) on a folded review row", () => {
    const refs: ArtifactIterationRef[] = [
      {
        artifact_id: 20,
        iteration_id: 8,
        stage: "review",
        status: "succeeded",
        outcome: "succeeded",
        agent_type: "codex",
        conversation_id: 4,
        attempt_count: 2,
      },
    ]
    render(
      <DagGraph
        {...base}
        artifacts={[
          art({ id: 10, kind: "task", status: "in_progress" }),
          art({
            id: 20,
            kind: "review",
            status: "done",
            verdict: "pass",
            title: "R",
            produced_by_iteration_id: 8,
          }),
        ]}
        links={[link(1, 20, 10, "reviews")]}
        artifactIterationRefs={refs}
      />
    )
    // The folded review row uses the shared AgentBadge, so it surfaces ×N from
    // the model-resolved attemptCount — a bespoke icon-only row dropped it.
    expect(screen.getByText("×2")).toBeTruthy()
  })

  it("shows an agent icon on a pending ghost when the facet is on", () => {
    render(
      <DagGraph
        {...base}
        artifacts={[]}
        liveIterations={[
          iter(1, "design", { status: "running", agent_type: "gemini" }),
        ]}
        artifactIterationRefs={[]}
      />
    )
    expect(screen.getByRole("button", { name: /agentRunBy/ })).toBeTruthy()
  })

  it("shows no facet — and no ghost icon — when refs are absent (old server)", () => {
    render(
      <DagGraph
        {...base}
        artifacts={[
          art({
            id: 2,
            kind: "requirement",
            title: "Req",
            produced_by_iteration_id: 5,
          }),
        ]}
        liveIterations={[
          iter(1, "design", { status: "running", agent_type: "gemini" }),
        ]}
      />
    )
    // No artifactIterationRefs ⇒ facet off ⇒ no node/ghost icon, no crash.
    expect(screen.queryByRole("button", { name: /agentRunBy/ })).toBeNull()
    expect(screen.queryByText("×3")).toBeNull()
  })

  it("renders no icon when the resolved agent_type is null (no crash)", () => {
    const refs: ArtifactIterationRef[] = [
      {
        artifact_id: 2,
        iteration_id: 5,
        stage: "refine",
        status: "succeeded",
        outcome: null,
        agent_type: null,
        conversation_id: null,
        attempt_count: 1,
      },
    ]
    render(
      <DagGraph
        {...base}
        artifacts={[
          art({
            id: 2,
            kind: "requirement",
            title: "Req",
            produced_by_iteration_id: 5,
          }),
        ]}
        artifactIterationRefs={refs}
      />
    )
    expect(screen.queryByRole("button", { name: /agentRunBy/ })).toBeNull()
  })

  it("renders a clickable session chip for a live artifact-less triage (session_only)", () => {
    const onOpenSession = vi.fn()
    const { container } = render(
      <DagGraph
        {...base}
        artifacts={[]}
        liveIterations={[
          iter(1, "triage", {
            status: "running",
            agent_type: "codex",
            conversation_id: 7,
          }),
        ]}
        artifactIterationRefs={[]}
        onOpenSession={onOpenSession}
      />
    )
    // The Issue phase stays solid (not a slim placeholder) so the chip has a home.
    const issueBox = container.querySelector('[data-phase="issue"]')
    expect(issueBox).not.toBeNull()
    expect(issueBox!.getAttribute("data-placeholder")).toBeNull()
    // The chip is the session entry point (no drawer for a live, artifact-less run).
    fireEvent.click(screen.getByRole("button", { name: /triage/ }))
    expect(onOpenSession).toHaveBeenCalledWith(
      expect.objectContaining({
        conversationId: 7,
        agentType: "codex",
        stage: "triage",
      })
    )
  })
})
