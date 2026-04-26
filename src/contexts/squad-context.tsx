"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import {
  squadCreateRun,
  squadCreateTask,
  squadGetRun,
  squadListRuns,
  squadPromptRole,
  squadSeedRoleProfiles,
  squadStartRun,
  squadStopRun,
  squadUpdateRoleProfile,
  squadUpdateTaskStatus,
} from "@/lib/api"
import { subscribe } from "@/lib/platform"
import { applySquadEvent } from "@/lib/squad-events"
import type {
  SquadEvent,
  SquadRoleKind,
  SquadRoleProfileInfo,
  SquadRoleProfilePatch,
  SquadRunInfo,
  SquadRunMode,
  SquadRunSnapshot,
  SquadTaskInfo,
  SquadTaskStatus,
} from "@/lib/types"

interface SquadContextValue {
  profiles: SquadRoleProfileInfo[]
  runs: SquadRunInfo[]
  activeRun: SquadRunSnapshot | null
  loading: boolean
  error: string | null
  loadForFolder: (folderId: number) => Promise<void>
  createRun: (params: {
    folderId: number
    mode: SquadRunMode
    goalSummary: string
  }) => Promise<SquadRunSnapshot>
  startRun: (squadRunId: number, workingDir?: string | null) => Promise<void>
  stopRun: (squadRunId: number) => Promise<void>
  updateRoleProfile: (params: {
    folderId: number
    roleKind: SquadRoleKind
    patch: SquadRoleProfilePatch
  }) => Promise<SquadRoleProfileInfo>
  createTask: (params: {
    squadRunId: number
    assignedRoleKind: SquadRoleKind
    title: string
    description: string
  }) => Promise<SquadTaskInfo>
  updateTaskStatus: (params: {
    taskId: number
    status: SquadTaskStatus
  }) => Promise<SquadTaskInfo>
  promptRole: (params: {
    squadRunId: number
    roleKind: SquadRoleKind
    taskId?: number | null
  }) => Promise<void>
}

const SquadContext = createContext<SquadContextValue | null>(null)

export function useSquadContext() {
  const ctx = useContext(SquadContext)
  if (!ctx) {
    throw new Error("useSquadContext must be used within SquadProvider")
  }
  return ctx
}

function errorMessage(err: unknown) {
  return err instanceof Error ? err.message : String(err)
}

export function SquadProvider({ children }: { children: ReactNode }) {
  const [profiles, setProfiles] = useState<SquadRoleProfileInfo[]>([])
  const [runs, setRuns] = useState<SquadRunInfo[]>([])
  const [activeRun, setActiveRun] = useState<SquadRunSnapshot | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const loadForFolder = useCallback(async (folderId: number) => {
    setLoading(true)
    try {
      const [nextProfiles, nextRuns] = await Promise.all([
        squadSeedRoleProfiles(folderId),
        squadListRuns(folderId),
      ])
      setProfiles(nextProfiles)
      setRuns(nextRuns)
      if (nextRuns[0]) {
        setActiveRun(await squadGetRun(nextRuns[0].id))
      } else {
        setActiveRun(null)
      }
      setError(null)
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

  const createRun = useCallback(
    async (params: {
      folderId: number
      mode: SquadRunMode
      goalSummary: string
    }) => {
      const snapshot = await squadCreateRun(params)
      setActiveRun(snapshot)
      setRuns((prev) => [
        snapshot.run,
        ...prev.filter((r) => r.id !== snapshot.run.id),
      ])
      return snapshot
    },
    []
  )

  const startRun = useCallback(
    async (squadRunId: number, workingDir?: string | null) => {
      await squadStartRun({ squadRunId, workingDir })
      setActiveRun(await squadGetRun(squadRunId))
    },
    []
  )

  const stopRun = useCallback(async (squadRunId: number) => {
    await squadStopRun(squadRunId)
    setActiveRun(await squadGetRun(squadRunId))
  }, [])

  const updateRoleProfile = useCallback(
    async (params: {
      folderId: number
      roleKind: SquadRoleKind
      patch: SquadRoleProfilePatch
    }) => {
      const profile = await squadUpdateRoleProfile(params)
      setProfiles((prev) =>
        prev.map((item) => (item.id === profile.id ? profile : item))
      )
      setError(null)
      return profile
    },
    []
  )

  const createTask = useCallback(
    async (params: {
      squadRunId: number
      assignedRoleKind: SquadRoleKind
      title: string
      description: string
    }) => {
      const task = await squadCreateTask(params)
      setActiveRun(await squadGetRun(params.squadRunId))
      setError(null)
      return task
    },
    []
  )

  const updateTaskStatus = useCallback(
    async (params: { taskId: number; status: SquadTaskStatus }) => {
      const task = await squadUpdateTaskStatus(params)
      setActiveRun(await squadGetRun(task.squadRunId))
      setError(null)
      return task
    },
    []
  )

  const promptRole = useCallback(
    async (params: {
      squadRunId: number
      roleKind: SquadRoleKind
      taskId?: number | null
    }) => {
      await squadPromptRole(params)
      setActiveRun(await squadGetRun(params.squadRunId))
      setError(null)
    },
    []
  )

  // Track the active run id in a ref so the event subscription doesn't have
  // to re-bind when the snapshot reference changes.
  const activeRunRef = useRef<SquadRunSnapshot | null>(null)
  useEffect(() => {
    activeRunRef.current = activeRun
  }, [activeRun])

  useEffect(() => {
    // Coalesce reload requests so a burst of unknown-entity events triggers
    // at most one refetch per run id.
    const pendingReload = new Map<number, ReturnType<typeof setTimeout>>()
    const scheduleReload = (squadRunId: number) => {
      if (pendingReload.has(squadRunId)) return
      const handle = setTimeout(() => {
        pendingReload.delete(squadRunId)
        void squadGetRun(squadRunId)
          .then((snap) => {
            // Only commit if we're still looking at this run.
            if (activeRunRef.current?.run.id === snap.run.id) {
              setActiveRun(snap)
            }
            setError(null)
          })
          .catch((err) => setError(errorMessage(err)))
      }, 50)
      pendingReload.set(squadRunId, handle)
    }

    const unlisten = subscribe<SquadEvent>("squad://event", (event) => {
      // Keep the runs list summary in sync for run-level transitions even
      // when the user isn't focused on that specific run.
      if (event.type === "squad_run_status_changed") {
        const payload = event.payload as Partial<SquadRunInfo> | undefined
        if (
          payload &&
          typeof payload.id === "number" &&
          typeof payload.status === "string"
        ) {
          const next = payload as SquadRunInfo
          setRuns((prev) => prev.map((r) => (r.id === next.id ? next : r)))
        }
      }
      const current = activeRunRef.current
      if (!current || current.run.id !== event.squadRunId) {
        // Event is for a different run: nothing to patch locally.
        return
      }
      const result = applySquadEvent(current, event)
      if (result.changed && result.snapshot) {
        setActiveRun(result.snapshot)
      }
      if (result.needsReload) {
        scheduleReload(event.squadRunId)
      }
    })
    return () => {
      for (const handle of pendingReload.values()) {
        clearTimeout(handle)
      }
      pendingReload.clear()
      void unlisten.then((fn) => fn())
    }
  }, [])

  const value = useMemo(
    () => ({
      profiles,
      runs,
      activeRun,
      loading,
      error,
      loadForFolder,
      createRun,
      startRun,
      stopRun,
      updateRoleProfile,
      createTask,
      updateTaskStatus,
      promptRole,
    }),
    [
      profiles,
      runs,
      activeRun,
      loading,
      error,
      loadForFolder,
      createRun,
      startRun,
      stopRun,
      updateRoleProfile,
      createTask,
      updateTaskStatus,
      promptRole,
    ]
  )

  return <SquadContext.Provider value={value}>{children}</SquadContext.Provider>
}
