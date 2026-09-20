import type {
  BuiltinAgentType,
  LoopBack,
  PipelineGraph,
  PipelineRole,
  PipelineStep,
} from "@/lib/types"

export const BUILTIN_AGENT_TYPES: readonly BuiltinAgentType[] = [
  "claude_code",
  "codex",
  "open_code",
  "gemini",
  "open_claw",
  "cline",
  "hermes",
  "code_buddy",
  "kimi_code",
  "pi",
  "grok",
  "cursor",
  "deepseek",
  "qoder",
  "antigravity",
] as const

const BUILTIN_AGENT_SET = new Set<string>(BUILTIN_AGENT_TYPES)

export function isValidAgentType(agentType: string): boolean {
  if (BUILTIN_AGENT_SET.has(agentType)) return true
  if (agentType.startsWith("custom:") && agentType.length > "custom:".length) {
    return true
  }
  return false
}

export const STEP_ID_REGEX = /^[a-z0-9_-]{1,32}$/

export type PipelineValidationErrorCode =
  | "empty"
  | "tooManySteps"
  | "too_many_steps"
  | "badStepId"
  | "bad_step_id"
  | "duplicateStepId"
  | "duplicate_step_id"
  | "unknownAgent"
  | "unknown_agent"
  | "emptyPrompt"
  | "empty_prompt"
  | "badTimeout"
  | "bad_timeout"
  | "duplicateLoop"
  | "duplicate_loop"
  | "loopFromWrongRole"
  | "loop_from_wrong_role"
  | "loopTargetNotEarlier"
  | "loop_target_not_earlier"
  | "badMaxIterations"
  | "bad_max_iterations"

export interface PipelineValidationError {
  code: PipelineValidationErrorCode
  id?: string
  from?: string
  to?: string
  message?: string
}

export type PipelineValidationResult =
  | { valid: true; error?: undefined }
  | { valid: false; error: PipelineValidationError }

/**
 * Validates a pipeline graph against the server rules.
 * Mirrors `src-tauri/src/pipeline/validate.rs`.
 */
export function validatePipelineGraph(
  graph: PipelineGraph
): PipelineValidationResult {
  if (!graph.steps || graph.steps.length === 0) {
    return {
      valid: false,
      error: {
        code: "empty",
        message: "Pipeline must contain at least one step",
      },
    }
  }

  if (graph.steps.length > 8) {
    return {
      valid: false,
      error: {
        code: "tooManySteps",
        message: "Pipeline cannot contain more than 8 steps",
      },
    }
  }

  const seenIds = new Set<string>()

  for (const step of graph.steps) {
    if (!step.id || !STEP_ID_REGEX.test(step.id)) {
      return {
        valid: false,
        error: {
          code: "badStepId",
          id: step.id,
          message: `Invalid step id: ${step.id}`,
        },
      }
    }

    if (seenIds.has(step.id)) {
      return {
        valid: false,
        error: {
          code: "duplicateStepId",
          id: step.id,
          message: `Duplicate step id: ${step.id}`,
        },
      }
    }
    seenIds.add(step.id)

    if (!isValidAgentType(step.agent_type)) {
      return {
        valid: false,
        error: {
          code: "unknownAgent",
          id: step.id,
          message: `Unknown agent for step ${step.id}`,
        },
      }
    }

    if (!step.prompt_template || step.prompt_template.trim().length === 0) {
      return {
        valid: false,
        error: {
          code: "emptyPrompt",
          id: step.id,
          message: `Step prompt is empty: ${step.id}`,
        },
      }
    }

    if (
      typeof step.timeout_secs !== "number" ||
      !Number.isInteger(step.timeout_secs) ||
      step.timeout_secs < 1 ||
      step.timeout_secs > 86400
    ) {
      return {
        valid: false,
        error: {
          code: "badTimeout",
          id: step.id,
          message: `Invalid timeout for step: ${step.id}`,
        },
      }
    }
  }

  const positions = new Map<string, number>()
  graph.steps.forEach((step, index) => {
    positions.set(step.id, index)
  })

  const sources = new Set<string>()
  for (const loop of graph.loops ?? []) {
    if (sources.has(loop.from_step)) {
      return {
        valid: false,
        error: {
          code: "duplicateLoop",
          from: loop.from_step,
          message: `Duplicate loop from step: ${loop.from_step}`,
        },
      }
    }
    sources.add(loop.from_step)

    const sourceIndex = positions.get(loop.from_step)
    if (sourceIndex === undefined) {
      return {
        valid: false,
        error: {
          code: "loopFromWrongRole",
          from: loop.from_step,
          message: `Loop source has an invalid role: ${loop.from_step}`,
        },
      }
    }

    const sourceStep = graph.steps[sourceIndex]
    if (sourceStep.role !== "reviewer" && sourceStep.role !== "tests") {
      return {
        valid: false,
        error: {
          code: "loopFromWrongRole",
          from: loop.from_step,
          message: `Loop source has an invalid role: ${loop.from_step}`,
        },
      }
    }

    const targetIndex = positions.get(loop.to_step)
    if (targetIndex === undefined || targetIndex >= sourceIndex) {
      return {
        valid: false,
        error: {
          code: "loopTargetNotEarlier",
          from: loop.from_step,
          to: loop.to_step,
          message: `Loop target is not earlier: ${loop.from_step} -> ${loop.to_step}`,
        },
      }
    }

    if (
      typeof loop.max_iterations !== "number" ||
      !Number.isInteger(loop.max_iterations) ||
      loop.max_iterations < 1 ||
      loop.max_iterations > 10
    ) {
      return {
        valid: false,
        error: {
          code: "badMaxIterations",
          from: loop.from_step,
          message: `Invalid max iterations for loop from step: ${loop.from_step}`,
        },
      }
    }
  }

  return { valid: true }
}

/** Alias for `validatePipelineGraph`. */
export const validateGraph = validatePipelineGraph

/**
 * Creates a default step with standard defaults.
 */
export function createDefaultStep(
  id: string,
  role: PipelineRole = "coder",
  label?: string
): PipelineStep {
  return {
    id,
    role,
    label:
      label ??
      (role === "coder"
        ? "Coder"
        : role === "reviewer"
          ? "Reviewer"
          : role === "planner"
            ? "Planner"
            : role === "tests"
              ? "Tests"
              : "Custom"),
    agent_type: "claude_code",
    mode_id: null,
    config_values: {},
    prompt_template: "$task",
    timeout_secs: 1800,
    read_memory: false,
    read_only: role === "reviewer",
  }
}

/**
 * Adds a step to the pipeline graph immutably.
 */
export function addStep(
  graph: PipelineGraph,
  step: PipelineStep,
  index?: number
): PipelineGraph {
  const steps = [...graph.steps]
  if (index !== undefined && index >= 0 && index <= steps.length) {
    steps.splice(index, 0, step)
  } else {
    steps.push(step)
  }
  return {
    ...graph,
    steps,
  }
}

/**
 * Removes a step from the graph and cleans up any loops referencing it.
 */
export function removeStep(
  graph: PipelineGraph,
  stepId: string
): PipelineGraph {
  const steps = graph.steps.filter((s) => s.id !== stepId)
  const loops = (graph.loops ?? []).filter(
    (l) => l.from_step !== stepId && l.to_step !== stepId
  )
  return {
    ...graph,
    steps,
    loops,
  }
}

/**
 * Updates a step in the graph immutably.
 */
export function updateStep(
  graph: PipelineGraph,
  stepId: string,
  patch: Partial<PipelineStep>
): PipelineGraph {
  const newId = patch.id ?? stepId
  const steps = graph.steps.map((s) => {
    if (s.id !== stepId) return s
    return { ...s, ...patch }
  })

  // If step id changed, update loops pointing to / from it
  const loops = (graph.loops ?? []).map((l) => {
    let from_step = l.from_step
    let to_step = l.to_step
    if (from_step === stepId) from_step = newId
    if (to_step === stepId) to_step = newId
    return { ...l, from_step, to_step }
  })

  return {
    ...graph,
    steps,
    loops,
  }
}

/**
 * Sets or updates a loop in the graph. Replaces any existing loop from `from_step`.
 */
export function setLoop(graph: PipelineGraph, loop: LoopBack): PipelineGraph {
  const loops = (graph.loops ?? []).filter(
    (l) => l.from_step !== loop.from_step
  )
  loops.push(loop)
  return {
    ...graph,
    loops,
  }
}

/**
 * Removes a loop from a specific step.
 */
export function removeLoop(
  graph: PipelineGraph,
  fromStep: string
): PipelineGraph {
  const loops = (graph.loops ?? []).filter((l) => l.from_step !== fromStep)
  return {
    ...graph,
    loops,
  }
}

/**
 * Formats a validation error using the translation function.
 */
export function formatValidationError(
  error: PipelineValidationError,
  t: (key: string, params?: Record<string, string | number>) => string
): string {
  // Normalize snake_case codes to camelCase keys for next-intl
  let codeKey: string = error.code
  if (codeKey.includes("_")) {
    codeKey = codeKey.replace(/_([a-z])/g, (_, ch) => ch.toUpperCase())
  }
  const params: Record<string, string | number> = {}
  if (error.id) params.id = error.id
  if (error.from) params.from = error.from
  if (error.to) params.to = error.to
  try {
    return t(`validation.${codeKey}`, params)
  } catch {
    return error.message ?? error.code
  }
}
