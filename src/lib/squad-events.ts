import type {
  SquadArtifactInfo,
  SquadEvent,
  SquadRoleRunInfo,
  SquadRunInfo,
  SquadRunSnapshot,
  SquadTaskInfo,
} from "@/lib/types"

/**
 * Result of folding a single squad event into an existing snapshot.
 *
 * - `snapshot`: the patched snapshot, or `null` if the event was for a
 *   different run and was therefore ignored.
 * - `changed`: whether the snapshot actually changed. UI consumers can use
 *   this to skip re-renders for no-op events.
 * - `needsReload`: the event references an entity we don't have locally
 *   (e.g. a brand-new role, or stale snapshot). Caller should fall back
 *   to a full `squadGetRun` fetch.
 */
export interface ApplySquadEventResult {
  snapshot: SquadRunSnapshot | null
  changed: boolean
  needsReload: boolean
}

function asObject(payload: unknown): Record<string, unknown> | null {
  if (payload && typeof payload === "object" && !Array.isArray(payload)) {
    return payload as Record<string, unknown>
  }
  return null
}

function castRunInfo(payload: unknown): SquadRunInfo | null {
  const obj = asObject(payload)
  if (!obj) return null
  if (typeof obj.id !== "number" || typeof obj.status !== "string") return null
  return obj as unknown as SquadRunInfo
}

function castRoleRunInfo(payload: unknown): SquadRoleRunInfo | null {
  const obj = asObject(payload)
  if (!obj) return null
  if (typeof obj.id !== "number" || typeof obj.roleKind !== "string") {
    return null
  }
  return obj as unknown as SquadRoleRunInfo
}

function castTaskInfo(payload: unknown): SquadTaskInfo | null {
  const obj = asObject(payload)
  if (!obj) return null
  if (typeof obj.id !== "number" || typeof obj.status !== "string") return null
  return obj as unknown as SquadTaskInfo
}

function castArtifactInfo(payload: unknown): SquadArtifactInfo | null {
  const obj = asObject(payload)
  if (!obj) return null
  if (typeof obj.id !== "number" || typeof obj.artifactType !== "string") {
    return null
  }
  return obj as unknown as SquadArtifactInfo
}

function upsertById<T extends { id: number }>(list: T[], item: T): T[] {
  const idx = list.findIndex((x) => x.id === item.id)
  if (idx === -1) {
    return [...list, item]
  }
  const next = list.slice()
  next[idx] = item
  return next
}

function prependUniqueById<T extends { id: number }>(list: T[], item: T): T[] {
  if (list.some((x) => x.id === item.id)) {
    return list.map((x) => (x.id === item.id ? item : x))
  }
  return [item, ...list]
}

/**
 * Apply a single squad event to a snapshot, returning a patched copy.
 * Pure: never mutates inputs. Returns `needsReload: true` for events whose
 * payload references entities the snapshot doesn't yet know about, so the
 * caller can decide whether to fetch the authoritative snapshot.
 */
export function applySquadEvent(
  snapshot: SquadRunSnapshot | null,
  event: SquadEvent
): ApplySquadEventResult {
  // Snapshot is for a different run — ignore.
  if (snapshot && snapshot.run.id !== event.squadRunId) {
    return { snapshot, changed: false, needsReload: false }
  }
  // No snapshot yet — caller should fetch.
  if (!snapshot) {
    return { snapshot: null, changed: false, needsReload: true }
  }

  switch (event.type) {
    case "squad_run_status_changed": {
      const run = castRunInfo(event.payload)
      if (!run || run.id !== snapshot.run.id) {
        return { snapshot, changed: false, needsReload: !run }
      }
      if (snapshot.run === run) {
        return { snapshot, changed: false, needsReload: false }
      }
      return {
        snapshot: { ...snapshot, run },
        changed: true,
        needsReload: false,
      }
    }

    case "squad_role_status_changed": {
      const role = castRoleRunInfo(event.payload)
      if (!role) return { snapshot, changed: false, needsReload: true }
      if (role.squadRunId !== snapshot.run.id) {
        return { snapshot, changed: false, needsReload: false }
      }
      const next = upsertById(snapshot.roles, role)
      if (next === snapshot.roles) {
        return { snapshot, changed: false, needsReload: false }
      }
      return {
        snapshot: { ...snapshot, roles: next },
        changed: true,
        needsReload: false,
      }
    }

    case "squad_role_connection_attached": {
      // Pure side-channel notice; nothing to patch in the snapshot itself.
      return { snapshot, changed: false, needsReload: false }
    }

    case "squad_task_created": {
      const task = castTaskInfo(event.payload)
      if (!task) return { snapshot, changed: false, needsReload: true }
      if (task.squadRunId !== snapshot.run.id) {
        return { snapshot, changed: false, needsReload: false }
      }
      return {
        snapshot: {
          ...snapshot,
          tasks: prependUniqueById(snapshot.tasks, task),
        },
        changed: true,
        needsReload: false,
      }
    }

    case "squad_task_status_changed": {
      const task = castTaskInfo(event.payload)
      if (!task) return { snapshot, changed: false, needsReload: true }
      if (task.squadRunId !== snapshot.run.id) {
        return { snapshot, changed: false, needsReload: false }
      }
      // If we don't yet know about this task, surface as needsReload so the
      // caller can refetch instead of silently dropping it.
      const known = snapshot.tasks.some((t) => t.id === task.id)
      if (!known) {
        return { snapshot, changed: false, needsReload: true }
      }
      return {
        snapshot: {
          ...snapshot,
          tasks: upsertById(snapshot.tasks, task),
        },
        changed: true,
        needsReload: false,
      }
    }

    case "squad_artifact_created": {
      const artifact = castArtifactInfo(event.payload)
      if (!artifact) return { snapshot, changed: false, needsReload: true }
      if (artifact.squadRunId !== snapshot.run.id) {
        return { snapshot, changed: false, needsReload: false }
      }
      return {
        snapshot: {
          ...snapshot,
          artifacts: prependUniqueById(snapshot.artifacts, artifact),
        },
        changed: true,
        needsReload: false,
      }
    }

    // Summary-only events: no snapshot change required. Components that care
    // about the summary can subscribe to the raw event channel separately.
    case "squad_conductor_plan_applied":
    case "squad_dispatch_round_completed":
      return { snapshot, changed: false, needsReload: false }

    default:
      // Unknown event type: be conservative and ask for a refetch.
      return { snapshot, changed: false, needsReload: true }
  }
}
