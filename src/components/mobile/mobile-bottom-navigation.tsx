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
        <span className="absolute right-[calc(50%-18px)] top-1 min-w-4 rounded-full bg-destructive px-1 text-center text-[9px] leading-4 text-destructive-foreground">
          {badge > 99 ? "99+" : badge}
        </span>
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

  return (
    <>
      <nav
        className="grid h-16 shrink-0 grid-cols-3 gap-1 border-t bg-background/95 px-2 py-1 backdrop-blur"
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
          className="max-h-[70vh] rounded-t-3xl px-4 pb-[calc(20px+env(safe-area-inset-bottom))]"
        >
          <SheetTitle className="flex items-center gap-2">
            <ListChecks className="h-5 w-5" />
            运行中与等待处理
          </SheetTitle>
          <div className="mt-5 space-y-3 overflow-y-auto">
            {tasks.map((task) => (
              <div key={task.id} className="rounded-xl border bg-card p-3">
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-medium">
                      {task.label || task.description}
                    </div>
                    <div className="mt-1 text-xs text-muted-foreground">
                      {task.status === "running" ? "运行中" : "等待执行"}
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
                className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-3"
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
              <div className="py-10 text-center text-sm text-muted-foreground">
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
