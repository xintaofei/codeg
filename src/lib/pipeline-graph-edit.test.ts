import { describe, expect, it } from "vitest"
import type { PipelineGraph, PipelineStep } from "@/lib/types"
import {
  addStep,
  createDefaultStep,
  formatValidationError,
  moveStep,
  removeLoop,
  removeStep,
  setLoop,
  updateStep,
  validatePipelineGraph,
} from "./pipeline-graph-edit"

function makeStep(
  id: string,
  role: PipelineStep["role"] = "coder",
  over: Partial<PipelineStep> = {}
): PipelineStep {
  return {
    id,
    role,
    label: id,
    agent_type: "claude_code",
    mode_id: null,
    config_values: {},
    prompt_template: "$task",
    timeout_secs: 1800,
    read_memory: false,
    read_only: role === "reviewer",
    ...over,
  }
}

describe("pipeline-graph-edit: graph transformations", () => {
  it("creates default step with standard values", () => {
    const s = createDefaultStep("coder_1", "coder")
    expect(s.id).toBe("coder_1")
    expect(s.role).toBe("coder")
    expect(s.agent_type).toBe("claude_code")
    expect(s.timeout_secs).toBe(1800)
    expect(s.read_only).toBe(false)

    const rev = createDefaultStep("rev_1", "reviewer")
    expect(rev.read_only).toBe(true)
  })

  it("adds step at the end or at a specific index", () => {
    const g: PipelineGraph = {
      steps: [makeStep("step1"), makeStep("step2")],
      loops: [],
    }

    const g2 = addStep(g, makeStep("step3"))
    expect(g2.steps.map((s) => s.id)).toEqual(["step1", "step2", "step3"])

    const g3 = addStep(g, makeStep("step_mid"), 1)
    expect(g3.steps.map((s) => s.id)).toEqual(["step1", "step_mid", "step2"])
  })

  it("removes step and cleans up any loops referencing it", () => {
    const g: PipelineGraph = {
      steps: [
        makeStep("coder"),
        makeStep("reviewer", "reviewer"),
        makeStep("tests", "tests"),
      ],
      loops: [
        { from_step: "reviewer", to_step: "coder", max_iterations: 3 },
        { from_step: "tests", to_step: "coder", max_iterations: 3 },
      ],
    }

    const g2 = removeStep(g, "reviewer")
    expect(g2.steps.map((s) => s.id)).toEqual(["coder", "tests"])
    expect(g2.loops).toEqual([
      { from_step: "tests", to_step: "coder", max_iterations: 3 },
    ])

    const g3 = removeStep(g, "coder")
    expect(g3.steps.map((s) => s.id)).toEqual(["reviewer", "tests"])
    expect(g3.loops).toEqual([])
  })

  it("updates step fields and updates loop references if id changes", () => {
    const g: PipelineGraph = {
      steps: [makeStep("coder"), makeStep("reviewer", "reviewer")],
      loops: [{ from_step: "reviewer", to_step: "coder", max_iterations: 3 }],
    }

    const g2 = updateStep(g, "coder", {
      id: "coder_v2",
      prompt_template: "new prompt",
    })
    expect(g2.steps[0].id).toBe("coder_v2")
    expect(g2.steps[0].prompt_template).toBe("new prompt")
    expect(g2.loops[0].to_step).toBe("coder_v2")

    const g3 = updateStep(g2, "reviewer", { id: "rev_v2" })
    expect(g3.loops[0].from_step).toBe("rev_v2")
  })

  it("sets or updates a loop and removes a loop", () => {
    const g: PipelineGraph = {
      steps: [makeStep("coder"), makeStep("reviewer", "reviewer")],
      loops: [{ from_step: "reviewer", to_step: "coder", max_iterations: 3 }],
    }

    const g2 = setLoop(g, {
      from_step: "reviewer",
      to_step: "coder",
      max_iterations: 5,
    })
    expect(g2.loops).toHaveLength(1)
    expect(g2.loops[0].max_iterations).toBe(5)

    const g3 = removeLoop(g2, "reviewer")
    expect(g3.loops).toHaveLength(0)
  })
})

describe("pipeline-graph-edit: validation", () => {
  it("validates a standard valid graph (duet)", () => {
    const graph: PipelineGraph = {
      steps: [makeStep("coder", "coder"), makeStep("review", "reviewer")],
      loops: [{ from_step: "review", to_step: "coder", max_iterations: 3 }],
    }
    expect(validatePipelineGraph(graph)).toEqual({ valid: true })
  })

  it("rejects empty graph", () => {
    const res = validatePipelineGraph({ steps: [], loops: [] })
    expect(res.valid).toBe(false)
    if (!res.valid) {
      expect(res.error.code).toBe("empty")
    }
  })

  it("rejects more than 8 steps", () => {
    const graph: PipelineGraph = {
      steps: Array.from({ length: 9 }, (_, i) => makeStep(`s${i}`)),
      loops: [],
    }
    const res = validatePipelineGraph(graph)
    expect(res.valid).toBe(false)
    if (!res.valid) {
      expect(res.error.code).toBe("tooManySteps")
    }
  })

  it("rejects invalid step id (uppercase, special chars, empty, >32 chars)", () => {
    const g1: PipelineGraph = {
      steps: [makeStep("Coder")],
      loops: [],
    }
    expect(validatePipelineGraph(g1)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "badStepId", id: "Coder" }),
    })

    const g2: PipelineGraph = {
      steps: [makeStep("a".repeat(33))],
      loops: [],
    }
    expect(validatePipelineGraph(g2)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "badStepId" }),
    })

    const g3: PipelineGraph = {
      steps: [makeStep("step with spaces")],
      loops: [],
    }
    expect(validatePipelineGraph(g3)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "badStepId" }),
    })
  })

  it("rejects duplicate step ids", () => {
    const graph: PipelineGraph = {
      steps: [makeStep("coder"), makeStep("coder", "reviewer")],
      loops: [],
    }
    expect(validatePipelineGraph(graph)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "duplicateStepId", id: "coder" }),
    })
  })

  it("rejects unknown agent types", () => {
    const g1: PipelineGraph = {
      steps: [makeStep("coder", "coder", { agent_type: "unknown_bot" })],
      loops: [],
    }
    expect(validatePipelineGraph(g1)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "unknownAgent", id: "coder" }),
    })

    const g2: PipelineGraph = {
      steps: [makeStep("coder", "coder", { agent_type: "custom:" })],
      loops: [],
    }
    expect(validatePipelineGraph(g2)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "unknownAgent", id: "coder" }),
    })

    const g3: PipelineGraph = {
      steps: [makeStep("coder", "coder", { agent_type: "custom:my-agent" })],
      loops: [],
    }
    expect(validatePipelineGraph(g3)).toEqual({ valid: true })
  })

  it("rejects empty prompt", () => {
    const graph: PipelineGraph = {
      steps: [makeStep("coder", "coder", { prompt_template: "   " })],
      loops: [],
    }
    expect(validatePipelineGraph(graph)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "emptyPrompt", id: "coder" }),
    })
  })

  it("rejects bad timeout", () => {
    const g1: PipelineGraph = {
      steps: [makeStep("coder", "coder", { timeout_secs: 0 })],
      loops: [],
    }
    expect(validatePipelineGraph(g1)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "badTimeout", id: "coder" }),
    })

    const g2: PipelineGraph = {
      steps: [makeStep("coder", "coder", { timeout_secs: 90000 })],
      loops: [],
    }
    expect(validatePipelineGraph(g2)).toEqual({
      valid: false,
      error: expect.objectContaining({ code: "badTimeout", id: "coder" }),
    })
  })

  it("rejects loop from wrong role", () => {
    const graph: PipelineGraph = {
      steps: [makeStep("coder1", "coder"), makeStep("coder2", "coder")],
      loops: [{ from_step: "coder2", to_step: "coder1", max_iterations: 3 }],
    }
    expect(validatePipelineGraph(graph)).toEqual({
      valid: false,
      error: expect.objectContaining({
        code: "loopFromWrongRole",
        from: "coder2",
      }),
    })
  })

  it("rejects loop target that is not earlier", () => {
    const graph: PipelineGraph = {
      steps: [makeStep("reviewer", "reviewer"), makeStep("coder", "coder")],
      loops: [{ from_step: "reviewer", to_step: "coder", max_iterations: 3 }],
    }
    expect(validatePipelineGraph(graph)).toEqual({
      valid: false,
      error: expect.objectContaining({
        code: "loopTargetNotEarlier",
        from: "reviewer",
        to: "coder",
      }),
    })
  })

  it("rejects duplicate loop from the same step", () => {
    const graph: PipelineGraph = {
      steps: [
        makeStep("coder", "coder"),
        makeStep("reviewer", "reviewer"),
        makeStep("tests", "tests"),
      ],
      loops: [
        { from_step: "reviewer", to_step: "coder", max_iterations: 3 },
        { from_step: "reviewer", to_step: "coder", max_iterations: 2 },
      ],
    }
    expect(validatePipelineGraph(graph)).toEqual({
      valid: false,
      error: expect.objectContaining({
        code: "duplicateLoop",
        from: "reviewer",
      }),
    })
  })

  it("rejects bad max iterations for loops", () => {
    const g1: PipelineGraph = {
      steps: [makeStep("coder", "coder"), makeStep("reviewer", "reviewer")],
      loops: [{ from_step: "reviewer", to_step: "coder", max_iterations: 0 }],
    }
    expect(validatePipelineGraph(g1)).toEqual({
      valid: false,
      error: expect.objectContaining({
        code: "badMaxIterations",
        from: "reviewer",
      }),
    })

    const g2: PipelineGraph = {
      steps: [makeStep("coder", "coder"), makeStep("reviewer", "reviewer")],
      loops: [{ from_step: "reviewer", to_step: "coder", max_iterations: 11 }],
    }
    expect(validatePipelineGraph(g2)).toEqual({
      valid: false,
      error: expect.objectContaining({
        code: "badMaxIterations",
        from: "reviewer",
      }),
    })
  })

  it("formats validation errors with next-intl translation function", () => {
    const t = (key: string, params?: Record<string, string | number>) => {
      if (key === "validation.empty") return "Add at least one step"
      if (key === "validation.duplicateStepId")
        return `Duplicate step id ${params?.id}`
      return key
    }

    expect(formatValidationError({ code: "empty" }, t)).toBe(
      "Add at least one step"
    )
    expect(
      formatValidationError({ code: "duplicateStepId", id: "coder" }, t)
    ).toBe("Duplicate step id coder")
    expect(
      formatValidationError({ code: "duplicate_step_id", id: "coder" }, t)
    ).toBe("Duplicate step id coder")
  })
})

describe("moveStep", () => {
  const graph = {
    steps: [
      createDefaultStep("a", "coder", "A"),
      createDefaultStep("b", "reviewer", "B"),
      createDefaultStep("c", "tests", "C"),
    ],
    loops: [],
  }

  it("moves a step earlier and later", () => {
    expect(moveStep(graph, "c", "up").steps.map((s) => s.id)).toEqual([
      "a",
      "c",
      "b",
    ])
    expect(moveStep(graph, "a", "down").steps.map((s) => s.id)).toEqual([
      "b",
      "a",
      "c",
    ])
  })

  it("leaves the ends alone", () => {
    expect(moveStep(graph, "a", "up")).toBe(graph)
    expect(moveStep(graph, "c", "down")).toBe(graph)
    expect(moveStep(graph, "nope", "up")).toBe(graph)
  })

  it("refuses a move that would make a loop point forward", () => {
    // b loops back to a; putting b in front of a would invert it.
    const looped = {
      ...graph,
      loops: [{ from_step: "b", to_step: "a", max_iterations: 3 }],
    }
    expect(moveStep(looped, "b", "up")).toBe(looped)
    // Moving the unrelated tail is still fine.
    expect(moveStep(looped, "c", "up").steps.map((s) => s.id)).toEqual([
      "a",
      "c",
      "b",
    ])
  })
})
