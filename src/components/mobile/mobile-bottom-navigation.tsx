"use client"

import { useState } from "react"
import { BellRing, ListChecks, MessageCircle, Settings } from "lucide-react"
import { useRouter } from "next/navigation"

import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet"
import { useAlertContext } from "@/contexts/alert-context"
import { useSidebarContext } from "@/contexts/sidebar-context"
import { useTaskContext } from "@/contexts/task-context"
import { isMobileEnvironment } from "@/lib/transport/detect"
import { cn } from "@/lib/utils"

function NavButton({
  label,
  active,
  badge,
  onClick,
  children,
}: {
  label: string
  active?: boolean
  badge?: number
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "relative flex min-h-12 min-w-20 flex-1 flex-col items-center justify-center gap-0.5 rounded-xl text-[11px] transition-colors",
        active
          ? "bg-primary/10 text-primary"
          : "text-muted-foreground active:bg-muted"
      )}
    >
      {children}
      <span>{label}</span>
      {!!badge && (
        <>
          <span className="sr-only">{badge} 项需要关注</span>
          <span
            aria-hidden="true"
            data-slot="nav-attention-indicator"
            className="absolute right-[calc(50%-13px)] top-1.5 h-2 w-2 rounded-full bg-primary/70 ring-2 ring-background"
          />
        </>
      )}
    </button>
  )
}

export function MobileBottomNavigation() {
  const router = useRouter()
  const { toggle: toggleSidebar } = useSidebarContext()
  const { tasks } = useTaskContext()
  const { alerts } = useAlertContext()
  const [taskCenterOpen, setTaskCenterOpen] = useState(false)

  if (!isMobileEnvironment()) return null

  const attentionCount =
    tasks.filter(
      (task) => task.status === "pending" || task.status === "running"
    ).length + alerts.length
  const runningCount = tasks.filter((task) => task.status === "running").length
  const pendingCount = tasks.filter((task) => task.status === "pending").length

  return (
    <>
      <nav
        className="grid h-14 shrink-0 grid-cols-3 gap-1 border-t bg-background/95 px-2 py-1 backdrop-blur"
        aria-label="移动端主导航"
      >
        <NavButton label="会话" active onClick={toggleSidebar}>
          <MessageCircle className="h-5 w-5" />
        </NavButton>
        <NavButton
          label="任务"
          badge={attentionCount}
          onClick={() => setTaskCenterOpen(true)}
        >
          <ListChecks className="h-5 w-5" />
        </NavButton>
        <NavButton label="设置" onClick={() => router.push("/mobile-settings")}>
          <Settings className="h-5 w-5" />
        </NavButton>
      </nav>

      <Sheet open={taskCenterOpen} onOpenChange={setTaskCenterOpen}>
        <SheetContent
          side="bottom"
          className="flex h-[min(72dvh,720px)] max-h-[calc(100dvh-12px)] flex-col overflow-hidden rounded-t-3xl px-4 pt-5 pb-[calc(16px+env(safe-area-inset-bottom))]"
        >
          <SheetTitle className="flex shrink-0 items-center gap-2 pr-10 text-lg">
            <ListChecks className="h-5 w-5" />
            运行中与等待处理
          </SheetTitle>
          <div className="mt-4 grid shrink-0 grid-cols-2 gap-2">
            <div className="rounded-2xl border bg-card px-3 py-2.5">
              <div className="text-xl font-semibold tabular-nums text-emerald-500">
                {runningCount}
              </div>
              <div className="mt-0.5 text-xs text-muted-foreground">运行中</div>
            </div>
            <div className="rounded-2xl border bg-card px-3 py-2.5">
              <div className="text-xl font-semibold tabular-nums text-amber-500">
                {pendingCount + alerts.length}
              </div>
              <div className="mt-0.5 text-xs text-muted-foreground">
                等待处理
              </div>
            </div>
          </div>
          <div className="mt-4 min-h-0 flex-1 space-y-3 overflow-y-auto overscroll-contain pb-1">
            {tasks.map((task) => (
              <div
                key={task.id}
                className="flex min-h-[68px] items-center rounded-2xl border bg-card px-3 py-2.5"
              >
                <div className="flex w-full items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-medium">
                      {task.description || task.label}
                    </div>
                    <div className="mt-1 truncate text-xs text-muted-foreground">
                      {task.status === "running" ? "运行中" : "等待执行"}
                      {task.description ? ` · ${task.label}` : ""}
                    </div>
                  </div>
                  {task.status === "running" && (
                    <span className="h-2.5 w-2.5 shrink-0 animate-pulse rounded-full bg-emerald-500" />
                  )}
                </div>
              </div>
            ))}
            {alerts.map((alert) => (
              <div
                key={alert.id}
                className="flex min-h-[68px] items-center rounded-2xl border border-amber-500/30 bg-amber-500/5 px-3 py-2.5"
              >
                <div className="flex items-start gap-2">
                  <BellRing className="mt-0.5 h-4 w-4 shrink-0 text-amber-500" />
                  <div>
                    <div className="text-sm font-medium">{alert.message}</div>
                    {alert.detail && (
                      <div className="mt-1 text-xs text-muted-foreground">
                        {alert.detail}
                      </div>
                    )}
                  </div>
                </div>
              </div>
            ))}
            {attentionCount === 0 && (
              <div className="flex min-h-32 items-center justify-center text-center text-sm text-muted-foreground">
                当前没有运行中或等待处理的任务
              </div>
            )}
            <p className="text-center text-xs text-muted-foreground">
              权限批准和 Agent 提问会显示在对应会话中。
            </p>
          </div>
        </SheetContent>
      </Sheet>
    </>
  )
}
