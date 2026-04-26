import { describe, it, expect } from "vitest"
import { applySquadEvent } from "./squad-events"
import type {
  SquadArtifactInfo,
  SquadEvent,
  SquadRoleRunInfo,
  SquadRunInfo,
  SquadRunSnapshot,
  SquadTaskInfo,
} from "./types"

// ---- fixture builders ------------------------------------------------------

function makeRun(overrides: Partial<SquadRunInfo> = {}): SquadRunInfo {
  return {
    id: 1,
    folderId: 10,
    originConversationId: null,
    mode: "feature",
    status: "running",
    goalSummary: "ship it",
    baseBranch: "main",
    isolationMode: "shared",
    startedWithDirtyBase: false,
    createdAt: "2026-04-26T00:00:00Z",
    updatedAt: "2026-04-26T00:00:00Z",
    startedAt: null,
    completedAt: null,
    cancelledAt: null,
    errorMessage: null,
    ...overrides,
  }
}

function makeRole(overrides: Partial<SquadRoleRunInfo> = {}): SquadRoleRunInfo {
  return {
    id: 100,
    squadRunId: 1,
    roleKind: "frontend",
    roleProfileSnapshotJson: "{}",
    connectionId: null,
    sessionId: null,
    conversationId: null,
    workspacePath: null,
    branchName: null,
    status: "idle",
    lastEventAt: null,
    budgetStateJson: null,
    errorMessage: null,
    createdAt: "2026-04-26T00:00:00Z",
    updatedAt: "2026-04-26T00:00:00Z",
    ...overrides,
  }
}

function makeTask(overrides: Partial<SquadTaskInfo> = {}): SquadTaskInfo {
  return {
    id: 200,
    squadRunId: 1,
    assignedRoleKind: "frontend",
    title: "do the thing",
    description: "",
    inputSummary: null,
    status: "pending",
    dependsOnJson: null,
    priority: 0,
    createdAt: "2026-04-26T00:00:00Z",
    updatedAt: "2026-04-26T00:00:00Z",
    completedAt: null,
    errorMessage: null,
    ...overrides,
  }
}

function makeArtifact(
  overrides: Partial<SquadArtifactInfo> = {}
): SquadArtifactInfo {
  return {
    id: 300,
    squadRunId: 1,
    squadRoleRunId: null,
    taskId: null,
    roleKind: null,
    artifactType: "summary",
    title: "summary",
    contentJson: "{}",
    createdAt: "2026-04-26T00:00:00Z",
    ...overrides,
  }
}

function makeSnapshot(
  overrides: Partial<SquadRunSnapshot> = {}
): SquadRunSnapshot {
  return {
    run: makeRun(),
    roles: [],
    tasks: [],
    artifacts: [],
    ...overrides,
  }
}

function makeEvent(overrides: Partial<SquadEvent>): SquadEvent {
  return {
    type: "squad_run_status_changed",
    squadRunId: 1,
    seq: 1,
    at: "2026-04-26T00:00:00Z",
    roleKind: null,
    payload: null,
    ...overrides,
  }
}

// ---- tests -----------------------------------------------------------------

describe("applySquadEvent", () => {
  it("returns needsReload when no snapshot exists", () => {
    const result = applySquadEvent(
      null,
      makeEvent({ type: "squad_task_created", payload: makeTask() })
    )
    expect(result.snapshot).toBeNull()
    expect(result.changed).toBe(false)
    expect(result.needsReload).toBe(true)
  })

  it("ignores events for a different run", () => {
    const snapshot = makeSnapshot()
    const result = applySquadEvent(
      snapshot,
      makeEvent({
        type: "squad_task_created",
        squadRunId: 999,
        payload: makeTask({ squadRunId: 999 }),
      })
    )
    expect(result.snapshot).toBe(snapshot)
    expect(result.changed).toBe(false)
    expect(result.needsReload).toBe(false)
  })

  it("patches snapshot.run on squad_run_status_changed", () => {
    const snapshot = makeSnapshot()
    const newRun = makeRun({ status: "completed" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_run_status_changed", payload: newRun })
    )
    expect(result.changed).toBe(true)
    expect(result.snapshot?.run.status).toBe("completed")
    // immutability: original untouched
    expect(snapshot.run.status).toBe("running")
  })

  it("upserts role on squad_role_status_changed", () => {
    const existing = makeRole({ id: 100, status: "idle" })
    const snapshot = makeSnapshot({ roles: [existing] })
    const updated = makeRole({ id: 100, status: "working" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_role_status_changed", payload: updated })
    )
    expect(result.changed).toBe(true)
    expect(result.snapshot?.roles).toHaveLength(1)
    expect(result.snapshot?.roles[0].status).toBe("working")
  })

  it("appends a brand-new role on squad_role_status_changed", () => {
    const snapshot = makeSnapshot({ roles: [makeRole({ id: 100 })] })
    const newRole = makeRole({ id: 101, roleKind: "backend" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_role_status_changed", payload: newRole })
    )
    expect(result.changed).toBe(true)
    expect(result.snapshot?.roles).toHaveLength(2)
  })

  it("returns no-op for squad_role_connection_attached", () => {
    const snapshot = makeSnapshot()
    const result = applySquadEvent(
      snapshot,
      makeEvent({
        type: "squad_role_connection_attached",
        payload: { roleId: 100, connectionId: "abc" },
      })
    )
    expect(result.changed).toBe(false)
    expect(result.needsReload).toBe(false)
    expect(result.snapshot).toBe(snapshot)
  })

  it("prepends new task on squad_task_created", () => {
    const existing = makeTask({ id: 200, title: "old" })
    const snapshot = makeSnapshot({ tasks: [existing] })
    const created = makeTask({ id: 201, title: "new" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_task_created", payload: created })
    )
    expect(result.changed).toBe(true)
    expect(result.snapshot?.tasks).toHaveLength(2)
    expect(result.snapshot?.tasks[0].id).toBe(201) // prepended
  })

  it("upserts known task on squad_task_status_changed", () => {
    const existing = makeTask({ id: 200, status: "pending" })
    const snapshot = makeSnapshot({ tasks: [existing] })
    const updated = makeTask({ id: 200, status: "completed" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_task_status_changed", payload: updated })
    )
    expect(result.changed).toBe(true)
    expect(result.snapshot?.tasks[0].status).toBe("completed")
  })

  it("requests reload when task_status_changed references unknown task", () => {
    const snapshot = makeSnapshot() // no tasks
    const ghost = makeTask({ id: 999, status: "completed" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_task_status_changed", payload: ghost })
    )
    expect(result.changed).toBe(false)
    expect(result.needsReload).toBe(true)
  })

  it("prepends new artifact on squad_artifact_created", () => {
    const snapshot = makeSnapshot({ artifacts: [makeArtifact({ id: 300 })] })
    const created = makeArtifact({ id: 301, artifactType: "plan" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_artifact_created", payload: created })
    )
    expect(result.changed).toBe(true)
    expect(result.snapshot?.artifacts[0].id).toBe(301)
    expect(result.snapshot?.artifacts).toHaveLength(2)
  })

  it("returns no-op for summary-only events", () => {
    const snapshot = makeSnapshot()
    const planned = applySquadEvent(
      snapshot,
      makeEvent({
        type: "squad_conductor_plan_applied",
        payload: { created: 3, skipped: [] },
      })
    )
    expect(planned.changed).toBe(false)
    expect(planned.needsReload).toBe(false)

    const round = applySquadEvent(
      snapshot,
      makeEvent({
        type: "squad_dispatch_round_completed",
        payload: { dispatched: 0 },
      })
    )
    expect(round.changed).toBe(false)
    expect(round.needsReload).toBe(false)
  })

  it("returns needsReload for unknown event type", () => {
    const snapshot = makeSnapshot()
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_something_new", payload: {} })
    )
    expect(result.needsReload).toBe(true)
    expect(result.changed).toBe(false)
  })

  it("rejects malformed payloads (not an object) with needsReload", () => {
    const snapshot = makeSnapshot()
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_task_created", payload: "not-an-object" })
    )
    expect(result.changed).toBe(false)
    expect(result.needsReload).toBe(true)
  })

  it("ignores task_created whose squadRunId does not match snapshot", () => {
    // Same outer event runId matches snapshot, but inner payload references
    // a different run — caller should NOT mutate or refetch.
    const snapshot = makeSnapshot()
    const wrongRunTask = makeTask({ squadRunId: 42 })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_task_created", payload: wrongRunTask })
    )
    expect(result.changed).toBe(false)
    expect(result.needsReload).toBe(false)
  })

  it("does not mutate the input snapshot when applying changes", () => {
    const role = makeRole({ id: 100, status: "idle" })
    const snapshot = makeSnapshot({ roles: [role] })
    const updated = makeRole({ id: 100, status: "working" })
    const result = applySquadEvent(
      snapshot,
      makeEvent({ type: "squad_role_status_changed", payload: updated })
    )
    expect(result.snapshot).not.toBe(snapshot)
    expect(snapshot.roles[0].status).toBe("idle")
    expect(result.snapshot?.roles[0].status).toBe("working")
  })
})
