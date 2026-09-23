import type {
  Pipeline,
  PipelineAttempt,
  PipelineChange,
  PipelineDiff,
  PipelineGraph,
  PipelineRun,
  PipelineRunStatus,
  PipelineVerdict,
} from "@/lib/types"

/**
 * Standard 2-step Duet pipeline graph fixture:
 * 1. Coder (claude_code, read/write)
 * 2. Reviewer (claude_code, read-only, loops back up to 3 iterations on changes_requested)
 */
export const mockDuetGraph: PipelineGraph = {
  steps: [
    {
      id: "coder",
      role: "coder",
      label: "Coder",
      agent_type: "claude_code",
      config_values: {},
      prompt_template: "$task",
      timeout_secs: 1800,
      read_memory: false,
      read_only: false,
    },
    {
      id: "reviewer",
      role: "reviewer",
      label: "Reviewer",
      agent_type: "claude_code",
      config_values: {},
      prompt_template: "Review changes against the requirements",
      timeout_secs: 1800,
      read_memory: false,
      read_only: true,
    },
  ],
  loops: [
    {
      from_step: "reviewer",
      to_step: "coder",
      max_iterations: 3,
    },
  ],
}

/**
 * Duet preset entity fixture
 */
export const mockDuetPreset: Pipeline = {
  id: 1,
  name: "duet",
  preset_key: "duet",
  folder_id: null,
  graph: mockDuetGraph,
  isolation: "worktree_per_run",
  created_at: "2026-09-19T00:00:00Z",
  updated_at: "2026-09-19T00:00:00Z",
}

/**
 * Multi-file pipeline diff fixture with additions, deletions, and unified patch
 */
export const mockPipelineDiff: PipelineDiff = {
  files: [
    {
      path: "src/main.rs",
      status: "M",
      additions: 3,
      deletions: 1,
    },
    {
      path: "src/lib.rs",
      status: "A",
      additions: 2,
      deletions: 0,
    },
  ],
  patch: `diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,5 @@
 fn main() {
-    println!("hello");
+    let greeting = "hello world";
+    let timestamp = "2026-09-19";
+    println!("{}: {}", timestamp, greeting);
 }
diff --git a/src/lib.rs b/src/lib.rs
--- /dev/null
+++ b/src/lib.rs
@@ -0,0 +1,2 @@
+pub fn run() {}
+pub fn stop() {}
`,
  truncated: false,
}

/**
 * Creates a PipelineAttempt fixture with defaults
 */
export function createMockAttempt(
  overrides: Partial<PipelineAttempt> = {}
): PipelineAttempt {
  return {
    id: 1,
    run_id: 1,
    step_id: "coder",
    iteration: 1,
    status: "running",
    conversation_id: 101,
    model_requested: "claude-3-7-sonnet",
    model_actual: "claude-3-7-sonnet-20250219",
    verdict: null,
    verdict_source: null,
    notes: null,
    summary: null,
    started_at: "2026-09-19T10:00:00Z",
    ended_at: null,
    ...overrides,
  }
}

/**
 * Creates a PipelineRun fixture with configurable overrides
 */
export function createMockPipelineRun(
  overrides: Partial<PipelineRun> = {}
): PipelineRun {
  return {
    id: 1,
    pipeline_id: 1,
    folder_id: 10,
    worktree_folder_id: 11,
    parent_conversation_id: 100,
    graph: mockDuetGraph,
    status: "running",
    current_step_id: "coder",
    current_iteration: 1,
    error: null,
    attempts: [createMockAttempt()],
    started_at: "2026-09-19T10:00:00Z",
    ended_at: null,
    ...overrides,
  }
}

/**
 * Stage 1 fixture: Pipeline initialized, Coder step running in Round 1
 */
export const mockInitialDuetRun: PipelineRun = createMockPipelineRun({
  id: 1,
  status: "running",
  current_step_id: "coder",
  current_iteration: 1,
  attempts: [
    createMockAttempt({
      id: 1,
      step_id: "coder",
      iteration: 1,
      status: "running",
    }),
  ],
})

/**
 * Stage 2 fixture: Reviewer step settled with changes_requested and notes
 */
export const mockReviewChangesRequestedRun: PipelineRun = createMockPipelineRun(
  {
    id: 1,
    status: "running",
    current_step_id: "reviewer",
    current_iteration: 1,
    attempts: [
      createMockAttempt({
        id: 1,
        step_id: "coder",
        iteration: 1,
        status: "done",
        ended_at: "2026-09-19T10:02:00Z",
      }),
      createMockAttempt({
        id: 2,
        step_id: "reviewer",
        iteration: 1,
        status: "done",
        conversation_id: 102,
        verdict: "changes_requested",
        verdict_source: "marker",
        notes: "src/main.rs:2: please optimize string formatting",
        summary: "Changes requested on formatting",
        started_at: "2026-09-19T10:02:00Z",
        ended_at: "2026-09-19T10:03:00Z",
      }),
    ],
  }
)

/**
 * Stage 3 fixture: Round 2 of 3 started, Coder running
 */
export const mockRound2CoderRun: PipelineRun = createMockPipelineRun({
  id: 1,
  status: "running",
  current_step_id: "coder",
  current_iteration: 2,
  attempts: [
    createMockAttempt({
      id: 1,
      step_id: "coder",
      iteration: 1,
      status: "done",
      ended_at: "2026-09-19T10:02:00Z",
    }),
    createMockAttempt({
      id: 2,
      step_id: "reviewer",
      iteration: 1,
      status: "done",
      conversation_id: 102,
      verdict: "changes_requested",
      verdict_source: "marker",
      notes: "src/main.rs:2: please optimize string formatting",
      started_at: "2026-09-19T10:02:00Z",
      ended_at: "2026-09-19T10:03:00Z",
    }),
    createMockAttempt({
      id: 3,
      step_id: "coder",
      iteration: 2,
      status: "running",
      conversation_id: 103,
      started_at: "2026-09-19T10:03:30Z",
      ended_at: null,
    }),
  ],
})

/**
 * Stage 4 fixture: Round 2 Reviewer settled with pass, status succeeded
 */
export const mockSucceededRun: PipelineRun = createMockPipelineRun({
  id: 1,
  status: "succeeded",
  current_step_id: "reviewer",
  current_iteration: 2,
  ended_at: "2026-09-19T10:06:00Z",
  attempts: [
    createMockAttempt({
      id: 1,
      step_id: "coder",
      iteration: 1,
      status: "done",
      ended_at: "2026-09-19T10:02:00Z",
    }),
    createMockAttempt({
      id: 2,
      step_id: "reviewer",
      iteration: 1,
      status: "done",
      verdict: "changes_requested",
      verdict_source: "marker",
      notes: "src/main.rs:2: please optimize string formatting",
      started_at: "2026-09-19T10:02:00Z",
      ended_at: "2026-09-19T10:03:00Z",
    }),
    createMockAttempt({
      id: 3,
      step_id: "coder",
      iteration: 2,
      status: "done",
      started_at: "2026-09-19T10:03:30Z",
      ended_at: "2026-09-19T10:04:30Z",
    }),
    createMockAttempt({
      id: 4,
      step_id: "reviewer",
      iteration: 2,
      status: "done",
      verdict: "pass",
      verdict_source: "marker",
      notes: "All checks passed!",
      started_at: "2026-09-19T10:04:30Z",
      ended_at: "2026-09-19T10:05:30Z",
    }),
  ],
})

/**
 * Factory for pipeline://changed event payloads
 */
export const mockPipelineEvents = {
  runStarted: (runId = 1, folderId = 10): PipelineChange => ({
    kind: "run_started",
    run_id: runId,
    folder_id: folderId,
  }),
  stepStarted: (
    runId = 1,
    attemptId = 1,
    stepId = "coder",
    iteration = 1
  ): PipelineChange => ({
    kind: "step_started",
    run_id: runId,
    attempt_id: attemptId,
    step_id: stepId,
    iteration,
  }),
  stepSettled: (
    runId = 1,
    attemptId = 2,
    verdict: PipelineVerdict = "changes_requested"
  ): PipelineChange => ({
    kind: "step_settled",
    run_id: runId,
    attempt_id: attemptId,
    verdict,
  }),
  runSettled: (
    runId = 1,
    status: PipelineRunStatus = "succeeded"
  ): PipelineChange => ({
    kind: "run_settled",
    run_id: runId,
    status,
  }),
}
