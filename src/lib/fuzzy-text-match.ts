/**
 * Ranked text matcher for slash-command / skill autocomplete.
 * No external fuzzy dependency — tiers are exact → prefix → substring →
 * subsequence so short inputs like `bmhp` can still hit `bmad-help`.
 *
 * Same scoring shape as `file-search-match.ts` (tier dominates; earlier
 * match position and shorter candidate win within a tier).
 */

const TIER_BASE = 100_000_000

const TIER_EXACT = 6
const TIER_PREFIX = 5
const TIER_SUBSTRING = 4
const TIER_SUBSEQUENCE = 3

function tierScore(tier: number, position: number, length: number): number {
  return (
    tier * TIER_BASE - Math.min(position, 9_999) * 1_000 - Math.min(length, 999)
  )
}

/**
 * Index of the first matched character if every char of `query` appears in `s`
 * in order (a subsequence), else -1. `query` must be non-empty.
 */
export function subsequenceFirstIndex(query: string, s: string): number {
  let qi = 0
  let firstIdx = -1
  for (let si = 0; si < s.length && qi < query.length; si++) {
    if (s[si] === query[qi]) {
      if (qi === 0) firstIdx = si
      qi++
    }
  }
  return qi === query.length ? firstIdx : -1
}

/**
 * Score one already-lowercased string against an already-lowercased,
 * non-empty `query`. Higher is better; `null` means no match.
 */
export function scoreTextMatch(query: string, text: string): number | null {
  if (!query) return null

  if (text === query) {
    return tierScore(TIER_EXACT, 0, text.length)
  }
  if (text.startsWith(query)) {
    return tierScore(TIER_PREFIX, 0, text.length)
  }
  const idx = text.indexOf(query)
  if (idx !== -1) {
    return tierScore(TIER_SUBSTRING, idx, text.length)
  }
  const sub = subsequenceFirstIndex(query, text)
  if (sub !== -1) {
    return tierScore(TIER_SUBSEQUENCE, sub, text.length)
  }
  return null
}

/**
 * Filter + rank `items` by primary text, with optional secondary field fallback
 * (e.g. description / skill id). Empty query returns items unchanged (original
 * order). Secondary hits always rank below any primary hit.
 */
export function rankByTextMatch<T>(
  query: string,
  items: readonly T[],
  getPrimary: (item: T) => string,
  getSecondary?: (item: T) => string | undefined | null
): T[] {
  const q = query.trim().toLowerCase()
  if (!q) return items.slice()

  // Drop secondary below every primary tier so a weak name subsequence still
  // beats a perfect description / id match (mirrors previous name-first lists).
  const secondaryOffset = TIER_EXACT * TIER_BASE

  const scored: { item: T; score: number }[] = []
  for (const item of items) {
    const primary = getPrimary(item).toLowerCase()
    let score = scoreTextMatch(q, primary)
    if (score === null && getSecondary) {
      const secondary = getSecondary(item)?.toLowerCase()
      if (secondary) {
        const sec = scoreTextMatch(q, secondary)
        if (sec !== null) score = sec - secondaryOffset
      }
    }
    if (score !== null) scored.push({ item, score })
  }

  // ES2019 stable sort keeps original order for equal scores.
  scored.sort((a, b) => b.score - a.score)
  return scored.map((s) => s.item)
}
