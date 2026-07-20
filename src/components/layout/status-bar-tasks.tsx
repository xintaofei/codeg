"use client"

import { Clock } from "lucide-react"
import { useTranslations } from "next-intl"
import { useTaskContext } from "@/contexts/task-context"
import { Skeleton } from "@/components/ui/skeleton"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"

function Spinner({ className }: { className?: string }) {
  return (
    <div
      className={`animate-spin rounded-full border-[1.5px] border-current border-t-transparent ${className}`}
    />
  )
}

export function StatusBarTasks() {
  const t = useTranslations("Folder.statusBar.tasks")
  const { tasks } = useTaskContext()
  const isMobile = useIsMobile()

  if (tasks.length === 0) return null

  const runningTask = tasks.find(
    (t) => t.status === "running" || t.status === "pending"
  )

  const taskList = (
    <div className="space-y-2 max-h-48 overflow-y-auto">
      {tasks.map((task) => (
        <div key={task.id} className="space-y-1">
          <div className="flex items-center gap-2 text-xs">
            {task.status === "running" ? (
              <Spinner className="h-3 w-3 text-blue-500" />
            ) : (
              <Clock className="h-3 w-3 text-muted-foreground" />
            )}
            <span className="truncate flex-1">{task.label}</span>
          </div>
          {task.status === "running" && task.progress != null && (
            <div className="h-1 rounded-full bg-muted ml-5">
              <div
                className="h-full rounded-full bg-blue-500 transition-all"
                style={{ width: `${task.progress}%` }}
              />
            </div>
          )}
        </div>
      ))}
    </div>
  )

  if (isMobile) {
    return (
      <Popover>
        <PopoverTrigger asChild>
          <button className="flex min-w-0 flex-1 items-center justify-end gap-1.5 transition-colors hover:text-foreground">
            {runningTask ? (
              <Spinner className="h-3.5 w-3.5 shrink-0 text-primary" />
            ) : (
              <Clock className="h-3.5 w-3.5 shrink-0" />
            )}
            <span className="min-w-0 truncate">
              {runningTask
                ? runningTask.label || runningTask.description
                : t("title")}
            </span>
            {tasks.length > 1 && (
              <span className="shrink-0 tabular-nums">{tasks.length}</span>
            )}
          </button>
        </PopoverTrigger>
        <PopoverContent side="bottom" align="end" className="w-72 p-3">
          <div className="text-xs font-medium mb-2">{t("title")}</div>
          {taskList}
        </PopoverContent>
      </Popover>
    )
  }

  return (
    <div className="flex items-center gap-2">
      {runningTask && (
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span className="truncate max-w-40">
            {runningTask.label || runningTask.description}
          </span>
          <Skeleton className="h-1 w-28 rounded bg-primary/80" />
          <Spinner className="h-3 w-3 shrink-0" />
        </div>
      )}
      <Popover>
        <PopoverTrigger asChild>
          <button className="flex items-center gap-1 hover:text-foreground transition-colors">
            {!runningTask && <Clock className="h-3 w-3" />}
            {tasks.length > 1 && <span>{tasks.length}</span>}
          </button>
        </PopoverTrigger>
        <PopoverContent side="top" align="end" className="w-72 p-3">
          <div className="text-xs font-medium mb-2">{t("title")}</div>
          {taskList}
        </PopoverContent>
      </Popover>
    </div>
  )
}
