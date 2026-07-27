"use client"

import { createContext, useContext } from "react"

export type GoalControlAction = "pause" | "clear"

export interface GoalControlValue {
  /**
   * Fire a pause/clear on the session's currently active Codex goal, or `null`
   * when goal control isn't available for this surface — a history reload with
   * no live connection, a read-only viewer, or the read-only sub-agent dialog
   * (which renders its own `MessageListView` with no provider). The goal card
   * hides its control buttons whenever this is `null`, so liveness/ownership are
   * decided once at the panel where the connection is owned, not in the card.
   */
  onGoalControl: ((action: GoalControlAction) => void) | null
}

const GoalControlContext = createContext<GoalControlValue>({
  onGoalControl: null,
})

export const GoalControlProvider = GoalControlContext.Provider

export function useGoalControl(): GoalControlValue {
  return useContext(GoalControlContext)
}
