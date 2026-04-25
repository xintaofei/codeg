"use client"

import { useEffect, useMemo, useState } from "react"
import { Play, Send, Square, Users } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useSquadContext } from "@/contexts/squad-context"
import type {
  AgentType,
  SquadRoleKind,
  SquadRoleProfileInfo,
  SquadTaskInfo,
  SquadWorkspacePolicy,
} from "@/lib/types"

const ROLE_LABELS: Record<SquadRoleKind, string> = {
  conductor: "Conductor",
  frontend: "Frontend",
  backend: "Backend",
  worker: "Worker",
}

const AGENT_LABELS: Record<AgentType, string> = {
  claude_code: "Claude Code",
  codex: "Codex",
  open_code: "OpenCode",
  gemini: "Gemini CLI",
  generic: "Generic",
  cline: "Cline",
}

const WORKSPACE_POLICY_LABELS: Record<SquadWorkspacePolicy, string> = {
  read_only: "Read only",
  write_isolated: "Isolated writes",
  write_shared: "Shared workspace",
}

const ROLE_ORDER: SquadRoleKind[] = [
  "conductor",
  "frontend",
  "backend",
  "worker",
]
const AGENT_TYPES = Object.keys(AGENT_LABELS) as AgentType[]
const WORKSPACE_POLICIES = Object.keys(
  WORKSPACE_POLICY_LABELS
) as SquadWorkspacePolicy[]

function sortProfiles(profiles: SquadRoleProfileInfo[]) {
  return [...profiles].sort(
    (a, b) => ROLE_ORDER.indexOf(a.roleKind) - ROLE_ORDER.indexOf(b.roleKind)
  )
}

function taskLabel(task: SquadTaskInfo) {
  return `#${task.id} ${ROLE_LABELS[task.assignedRoleKind]} · ${task.title}`
}

export function SquadPanelTab() {
  const { activeFolder } = useActiveFolder()
  const {
    profiles,
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
  } = useSquadContext()
  const [goal, setGoal] = useState("")
  const [taskTitle, setTaskTitle] = useState("")
  const [taskDescription, setTaskDescription] = useState("")
  const [taskRole, setTaskRole] = useState<SquadRoleKind>("worker")
  const [busy, setBusy] = useState(false)
  const sortedProfiles = useMemo(() => sortProfiles(profiles), [profiles])

  useEffect(() => {
    if (activeFolder?.id) {
      void loadForFolder(activeFolder.id)
    }
  }, [activeFolder?.id, loadForFolder])

  const runAction = async (action: () => Promise<void>) => {
    setBusy(true)
    try {
      await action()
    } finally {
      setBusy(false)
    }
  }

  const handleCreateRun = async () => {
    if (!activeFolder || !goal.trim()) return
    await runAction(async () => {
      await createRun({
        folderId: activeFolder.id,
        mode: "conductor_dispatch",
        goalSummary: goal.trim(),
      })
    })
  }

  const handleStartRun = async () => {
    if (!activeRun || !activeFolder) return
    await runAction(async () => {
      await startRun(activeRun.run.id, activeFolder.path)
    })
  }

  const handleStopRun = async () => {
    if (!activeRun) return
    await runAction(async () => {
      await stopRun(activeRun.run.id)
    })
  }

  const handleCreateTask = async () => {
    if (!activeRun || !taskTitle.trim()) return
    await runAction(async () => {
      await createTask({
        squadRunId: activeRun.run.id,
        assignedRoleKind: taskRole,
        title: taskTitle.trim(),
        description: taskDescription.trim(),
      })
      setTaskTitle("")
      setTaskDescription("")
    })
  }

  const handlePromptTask = async (task: SquadTaskInfo) => {
    if (!activeRun) return
    await runAction(async () => {
      await promptRole({
        squadRunId: activeRun.run.id,
        roleKind: task.assignedRoleKind,
        taskId: task.id,
      })
      await updateTaskStatus({ taskId: task.id, status: "running" })
    })
  }

  const handleProfileEnabled = async (
    profile: SquadRoleProfileInfo,
    enabled: boolean
  ) => {
    if (!activeFolder) return
    await runAction(async () => {
      await updateRoleProfile({
        folderId: activeFolder.id,
        roleKind: profile.roleKind,
        patch: { enabled },
      })
    })
  }

  const handleProfileAgent = async (
    profile: SquadRoleProfileInfo,
    agentType: AgentType
  ) => {
    if (!activeFolder) return
    await runAction(async () => {
      await updateRoleProfile({
        folderId: activeFolder.id,
        roleKind: profile.roleKind,
        patch: { agentType },
      })
    })
  }

  const handleProfileWorkspace = async (
    profile: SquadRoleProfileInfo,
    workspacePolicy: SquadWorkspacePolicy
  ) => {
    if (!activeFolder) return
    await runAction(async () => {
      await updateRoleProfile({
        folderId: activeFolder.id,
        roleKind: profile.roleKind,
        patch: { workspacePolicy },
      })
    })
  }

  if (!activeFolder) {
    return (
      <div className="flex h-full items-center justify-center p-4 text-sm text-muted-foreground">
        Open a folder to use role squads.
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3 overflow-y-auto p-3 text-sm">
      <div className="flex items-center gap-2 font-medium">
        <Users className="h-4 w-4" />
        Role Squad
      </div>

      {error ? <div className="text-xs text-destructive">{error}</div> : null}

      <section className="space-y-2 rounded-md border border-border p-2">
        <div className="text-xs font-medium text-muted-foreground">Goal</div>
        <Textarea
          id="squad-goal"
          name="squad-goal"
          value={goal}
          onChange={(event) => setGoal(event.target.value)}
          placeholder="Describe the goal for the squad..."
          className="min-h-24 resize-none"
        />
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            onClick={handleCreateRun}
            disabled={busy || loading || !goal.trim()}
          >
            Create run
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={handleStartRun}
            disabled={busy || !activeRun || activeRun.run.status === "running"}
          >
            <Play className="mr-1 h-3.5 w-3.5" />
            Start
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={handleStopRun}
            disabled={busy || !activeRun}
          >
            <Square className="mr-1 h-3.5 w-3.5" />
            Stop
          </Button>
        </div>
      </section>

      <section className="rounded-md border border-border p-2">
        <div className="mb-2 text-xs font-medium text-muted-foreground">
          Active run
        </div>
        {activeRun ? (
          <div className="space-y-1">
            <div className="font-medium">#{activeRun.run.id}</div>
            <div className="text-xs text-muted-foreground">
              {activeRun.run.status} · {activeRun.run.goalSummary}
            </div>
          </div>
        ) : (
          <div className="text-xs text-muted-foreground">No squad run yet.</div>
        )}
      </section>

      <section className="space-y-2 rounded-md border border-border p-2">
        <div className="text-xs font-medium text-muted-foreground">Roles</div>
        {sortedProfiles.map((profile) => {
          const roleRun = activeRun?.roles.find(
            (role) => role.roleKind === profile.roleKind
          )
          return (
            <div
              key={profile.id}
              className="space-y-2 rounded-md bg-muted/30 p-2"
            >
              <div className="flex items-center justify-between gap-2">
                <div>
                  <div className="font-medium">
                    {ROLE_LABELS[profile.roleKind]}
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {roleRun?.status ?? "profile"}
                  </div>
                </div>
                <Switch
                  checked={profile.enabled}
                  disabled={busy}
                  onCheckedChange={(checked) =>
                    void handleProfileEnabled(profile, checked)
                  }
                  aria-label={`Enable ${ROLE_LABELS[profile.roleKind]}`}
                />
              </div>
              <div className="grid gap-2">
                <Select
                  value={profile.agentType}
                  onValueChange={(value) =>
                    void handleProfileAgent(profile, value as AgentType)
                  }
                  disabled={busy || !profile.enabled}
                >
                  <SelectTrigger className="w-full" size="sm">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent align="start">
                    {AGENT_TYPES.map((agentType) => (
                      <SelectItem key={agentType} value={agentType}>
                        {AGENT_LABELS[agentType]}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Select
                  value={profile.workspacePolicy}
                  onValueChange={(value) =>
                    void handleProfileWorkspace(
                      profile,
                      value as SquadWorkspacePolicy
                    )
                  }
                  disabled={busy || !profile.enabled}
                >
                  <SelectTrigger className="w-full" size="sm">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent align="start">
                    {WORKSPACE_POLICIES.map((policy) => (
                      <SelectItem key={policy} value={policy}>
                        {WORKSPACE_POLICY_LABELS[policy]}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              {roleRun?.connectionId ? (
                <div className="truncate text-xs text-muted-foreground">
                  {roleRun.connectionId}
                </div>
              ) : null}
            </div>
          )
        })}
      </section>

      <section className="space-y-2 rounded-md border border-border p-2">
        <div className="text-xs font-medium text-muted-foreground">Tasks</div>
        <div className="space-y-2">
          <div className="flex gap-2">
            <Select
              value={taskRole}
              onValueChange={(value) => setTaskRole(value as SquadRoleKind)}
              disabled={busy || !activeRun}
            >
              <SelectTrigger className="w-32" size="sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                {ROLE_ORDER.map((roleKind) => (
                  <SelectItem key={roleKind} value={roleKind}>
                    {ROLE_LABELS[roleKind]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Input
              id="squad-task-title"
              name="squad-task-title"
              value={taskTitle}
              onChange={(event) => setTaskTitle(event.target.value)}
              placeholder="Task title"
              disabled={busy || !activeRun}
            />
          </div>
          <Textarea
            id="squad-task-description"
            name="squad-task-description"
            value={taskDescription}
            onChange={(event) => setTaskDescription(event.target.value)}
            placeholder="Task details for the selected role..."
            className="min-h-20 resize-none"
            disabled={busy || !activeRun}
          />
          <Button
            size="sm"
            variant="secondary"
            onClick={handleCreateTask}
            disabled={busy || !activeRun || !taskTitle.trim()}
          >
            Add task
          </Button>
        </div>

        <div className="space-y-2">
          {(activeRun?.tasks ?? []).map((task) => (
            <div key={task.id} className="rounded-md bg-muted/30 p-2">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="truncate font-medium">{taskLabel(task)}</div>
                  <div className="text-xs text-muted-foreground">
                    {task.status}
                  </div>
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => void handlePromptTask(task)}
                  disabled={busy || task.status === "completed"}
                >
                  <Send className="mr-1 h-3.5 w-3.5" />
                  Prompt
                </Button>
              </div>
              {task.description ? (
                <div className="mt-1 line-clamp-3 text-xs text-muted-foreground">
                  {task.description}
                </div>
              ) : null}
            </div>
          ))}
          {activeRun && activeRun.tasks.length === 0 ? (
            <div className="text-xs text-muted-foreground">No tasks yet.</div>
          ) : null}
        </div>
      </section>
    </div>
  )
}
