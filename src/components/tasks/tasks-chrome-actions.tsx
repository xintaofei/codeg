"use client"

import { useTranslations } from "next-intl"
import { Kanban, List, Settings2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useTasksView } from "@/contexts/tasks-view-context"
import type { WorkbenchChromeActionsProps } from "@/components/workbench/workbench-content"

/** Fired by the chrome cluster's settings button; TasksPage owns the dialog
 *  (and the folder filter that scopes it), so the button just asks it to open.
 *  Lives here rather than in tasks-page.tsx because the sender is in the window
 *  chrome and the receiver is the page — neither should import the other. */
export const OPEN_TASK_SETTINGS_EVENT = "codeg:open-task-settings"

/**
 * The Tasks route's own entries in the window's top-right chrome cluster,
 * rendered immediately left of the (window-level) settings gear — see
 * `WorkbenchRouteChromeActions`.
 *
 * They cost nothing in width: a full-page route hides the terminal and aux
 * toggles (they act on the workspace this route covers), so these two take
 * their place in the top-right caption cluster.
 */
export function TasksChromeActions({
  buttonClassName,
  iconClassName,
}: WorkbenchChromeActionsProps) {
  const t = useTranslations("Tasks")
  const { tasks, viewMode, setViewMode } = useTasksView()
  // A single toggle rather than a segmented pair, and it shows the mode it
  // would switch TO: on one button "what happens if I press this" is the only
  // reading that doesn't need a legend.
  const toList = viewMode === "board"
  const switchLabel = t(toList ? "viewSwitchToList" : "viewSwitchToBoard")

  return (
    <>
      {/* Withheld on an empty board, like the page's own toolbar: with no tasks
          there is nothing to lay out either way. */}
      {tasks.length > 0 ? (
        <Button
          variant="ghost"
          size="icon"
          className={buttonClassName}
          onClick={() => setViewMode(toList ? "list" : "board")}
          title={switchLabel}
          aria-label={switchLabel}
        >
          {toList ? (
            <List className={iconClassName} />
          ) : (
            <Kanban className={iconClassName} />
          )}
        </Button>
      ) : null}
      <Button
        variant="ghost"
        size="icon"
        className={buttonClassName}
        onClick={() =>
          window.dispatchEvent(new Event(OPEN_TASK_SETTINGS_EVENT))
        }
        title={t("settingsTitle")}
        aria-label={t("settingsTitle")}
      >
        <Settings2 className={iconClassName} />
      </Button>
    </>
  )
}
