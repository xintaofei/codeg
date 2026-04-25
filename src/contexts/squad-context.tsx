"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
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

  useEffect(() => {
    const unlisten = subscribe<SquadEvent>("squad://event", (event) => {
      void squadGetRun(event.squadRunId)
        .then(setActiveRun)
        .catch((err) => setError(errorMessage(err)))
    })
    return () => {
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
