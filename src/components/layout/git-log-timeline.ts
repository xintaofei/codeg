// Pure helpers behind the git-log timeline UI. Kept out of the component so they
// can be unit-tested without pulling the whole "use client" module graph.

export function parseDate(dateStr: string): Date | null {
  const date = new Date(dateStr)
  return Number.isNaN(date.getTime()) ? null : date
}
