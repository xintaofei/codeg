/**
 * Humanize a 5-field cron expression into a small structured descriptor that the
 * UI renders via i18n. Covers the patterns the editor's presets and the
 * automation templates emit — every-N-minutes, hourly, daily, weekdays,
 * weekly-on-a-weekday, monthly-on-a-day. Anything else falls back to
 * `{ kind: "raw" }` so the expression is shown verbatim rather than
 * mis-described.
 *
 * Pure: no i18n lookups and no clock reads, so it is trivially testable and the
 * caller owns localization (day names, time format).
 *
 * Field order is the POSIX standard: `minute hour day-of-month month day-of-week`.
 */
export type CronDescriptor =
  | { kind: "everyMinutes"; n: number }
  | { kind: "hourly" }
  | { kind: "daily"; hour: number; minute: number }
  | { kind: "weekdays"; hour: number; minute: number }
  | { kind: "weekly"; dow: number; hour: number; minute: number }
  | { kind: "monthly"; dom: number; hour: number; minute: number }
  | { kind: "raw"; cron: string }

/** A single non-negative integer field, or null for anything else (ranges,
 *  lists, steps, `*`). */
function intField(field: string): number | null {
  return /^\d+$/.test(field) ? Number(field) : null
}

export function describeCron(cron: string): CronDescriptor {
  const trimmed = cron.trim()
  const raw: CronDescriptor = { kind: "raw", cron: trimmed }
  const fields = trimmed.split(/\s+/)
  if (fields.length !== 5) return raw
  const [min, hr, dom, mon, dow] = fields
  const restIsStar = dom === "*" && mon === "*" && dow === "*"

  // Every N minutes: `*/N * * * *`.
  const step = /^\*\/(\d+)$/.exec(min)
  if (step && hr === "*" && restIsStar) {
    const n = Number(step[1])
    return n >= 1 && n <= 59 ? { kind: "everyMinutes", n } : raw
  }

  // Hourly: a fixed in-range minute on every hour (`M * * * *`). The exact
  // minute is immaterial to the label, but an out-of-range value must still
  // fall through to raw rather than be mis-described.
  const hourlyMinute = intField(min)
  if (hourlyMinute != null && hourlyMinute <= 59 && hr === "*" && restIsStar) {
    return { kind: "hourly" }
  }

  // Everything below needs a fixed HH:MM anchor and a wildcard month.
  const minute = intField(min)
  const hour = intField(hr)
  if (minute == null || hour == null || minute > 59 || hour > 23) return raw
  if (mon !== "*") return raw

  if (dom === "*" && dow === "1-5") return { kind: "weekdays", hour, minute }
  if (dom === "*" && dow === "*") return { kind: "daily", hour, minute }

  // Weekly on a single weekday (cron allows 0 or 7 for Sunday — normalize to 0).
  if (dom === "*") {
    const d = intField(dow)
    if (d != null && d >= 0 && d <= 7) {
      return { kind: "weekly", dow: d === 7 ? 0 : d, hour, minute }
    }
    return raw
  }

  // Monthly on a single day-of-month.
  if (dow === "*") {
    const d = intField(dom)
    if (d != null && d >= 1 && d <= 31) {
      return { kind: "monthly", dom: d, hour, minute }
    }
    return raw
  }

  return raw
}
