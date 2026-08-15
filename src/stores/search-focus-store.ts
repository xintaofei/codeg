import { create } from "zustand"
import type { SearchMatchLocation } from "@/lib/types"

export type SearchFocusMatch = SearchMatchLocation & {
  occurrenceIndex: number
}

/** Transient request produced by the Ctrl+K dialog and consumed by the
 *  conversation detail view. It is intentionally never persisted. */
export interface SearchFocus {
  conversationId: number
  query: string
  titleMatches: SearchMatchLocation[]
  contentMatches: SearchFocusMatch[]
  activeMatchIndex: number
}

interface SearchFocusState {
  focus: SearchFocus | null
  setFocus: (focus: SearchFocus) => void
  setActiveMatchIndex: (index: number) => void
  advance: () => void
  clear: () => void
}

export const useSearchFocusStore = create<SearchFocusState>((set) => ({
  focus: null,
  setFocus: (focus) => set({ focus }),
  setActiveMatchIndex: (activeMatchIndex) =>
    set((state) =>
      state.focus ? { focus: { ...state.focus, activeMatchIndex } } : {}
    ),
  advance: () =>
    set((state) => {
      if (!state.focus || state.focus.contentMatches.length === 0) return {}
      const nextIndex =
        (state.focus.activeMatchIndex + 1) % state.focus.contentMatches.length
      return {
        focus: {
          ...state.focus,
          activeMatchIndex: nextIndex,
        },
      }
    }),
  clear: () => set({ focus: null }),
}))
