// Time ranges for the Query Store screens.

export interface Range {
  from: Date
  to: Date
}

export interface Preset {
  id: string
  label: string
  ms: number
}

export const presets: Preset[] = [
  { id: '15m', label: 'Last 15 minutes', ms: 15 * 60_000 },
  { id: '1h', label: 'Last hour', ms: 3_600_000 },
  { id: '6h', label: 'Last 6 hours', ms: 6 * 3_600_000 },
  { id: '24h', label: 'Last 24 hours', ms: 24 * 3_600_000 },
  { id: '7d', label: 'Last 7 days', ms: 7 * 86_400_000 },
]

/** Either a preset (ending now) or a fixed range of ISO timestamps. */
export interface Selection {
  preset: string
  from?: string
  to?: string
}

export const defaultSelection = (): Selection => ({ preset: '1h' })

export function resolve(sel: Selection, now: Date = new Date()): Range {
  if (sel.preset === 'custom' && sel.from && sel.to) {
    const from = new Date(sel.from)
    const to = new Date(sel.to)
    if (!Number.isNaN(from.getTime()) && !Number.isNaN(to.getTime()) && from < to) return { from, to }
  }
  const p = presets.find((x) => x.id === sel.preset) ?? presets[1]
  return { from: new Date(now.getTime() - p.ms), to: now }
}

/** The period of the same length that ends where `r` starts. */
export function previous(r: Range): Range {
  const span = r.to.getTime() - r.from.getTime()
  return { from: new Date(r.from.getTime() - span), to: new Date(r.from.getTime()) }
}

const BUCKETS = [60, 300, 900, 1800, 3600, 7200, 21_600, 43_200, 86_400]

/** A bucket length in seconds that gives at most about 120 buckets over the range. */
export function bucketSeconds(r: Range): number {
  const span = (r.to.getTime() - r.from.getTime()) / 1000
  return BUCKETS.find((b) => span / b <= 120) ?? BUCKETS[BUCKETS.length - 1]
}

export const iso = (d: Date): string => d.toISOString()

/** `value` of a `<input type="datetime-local">` for a date (local time). */
export function toLocalInput(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`
}

export function describe(r: Range): string {
  const f = (d: Date) => d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
  return `${f(r.from)} – ${f(r.to)}`
}
