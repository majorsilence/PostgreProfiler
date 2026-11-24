// Axis maths for the SVG charts.

export interface Point {
  t: number // milliseconds since the epoch
  v: number
}

export interface Series {
  name: string
  color: string
  points: Point[]
}

/** Round `x` to a "nice" number (1, 2, 5 times a power of ten). */
function nice(x: number, round: boolean): number {
  const exp = Math.floor(Math.log10(x))
  const f = x / Math.pow(10, exp)
  let n: number
  if (round) n = f < 1.5 ? 1 : f < 3 ? 2 : f < 7 ? 5 : 10
  else n = f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10
  return n * Math.pow(10, exp)
}

/** About `count` evenly spaced round values covering [min, max]. */
export function niceTicks(min: number, max: number, count = 5): number[] {
  if (!(max > min)) {
    const v = Number.isFinite(min) ? min : 0
    return [v, v + 1]
  }
  const step = nice((max - min) / Math.max(1, count - 1), true)
  const start = Math.floor(min / step) * step
  const end = Math.ceil(max / step) * step
  const out: number[] = []
  for (let v = start; v <= end + step / 2; v += step) out.push(Number(v.toPrecision(12)))
  return out
}

const STEPS = [
  60_000, 5 * 60_000, 10 * 60_000, 15 * 60_000, 30 * 60_000, 3_600_000, 2 * 3_600_000, 3 * 3_600_000,
  6 * 3_600_000, 12 * 3_600_000, 86_400_000, 2 * 86_400_000, 7 * 86_400_000,
]

/** Round timestamps (multiples of a minute, hour or day) between `from` and `to`. */
export function timeTicks(from: number, to: number, count = 6): number[] {
  const span = Math.max(1, to - from)
  const step = STEPS.find((s) => span / s <= count) ?? STEPS[STEPS.length - 1]
  const out: number[] = []
  for (let t = Math.ceil(from / step) * step; t <= to; t += step) out.push(t)
  return out
}

export function formatTick(t: number, spanMs: number): string {
  const d = new Date(t)
  const p = (n: number) => String(n).padStart(2, '0')
  const hm = `${p(d.getHours())}:${p(d.getMinutes())}`
  if (spanMs > 2 * 86_400_000) return `${d.getMonth() + 1}/${d.getDate()}`
  if (spanMs > 86_400_000) return `${d.getMonth() + 1}/${d.getDate()} ${hm}`
  return hm
}

/** Compact number for an axis: 1.2k, 3.4M. */
export function compact(v: number): string {
  const a = Math.abs(v)
  if (a >= 1e9) return `${(v / 1e9).toFixed(1)}G`
  if (a >= 1e6) return `${(v / 1e6).toFixed(1)}M`
  if (a >= 1e4) return `${(v / 1e3).toFixed(0)}k`
  if (a >= 1e3) return `${(v / 1e3).toFixed(1)}k`
  if (a >= 100 || Number.isInteger(v)) return String(Math.round(v))
  return v.toFixed(a >= 10 ? 1 : 2)
}

/** Largest value over all series, or of the stacked totals for bars. */
export function maxValue(series: Series[], stacked: boolean): number {
  if (stacked) {
    const sums = new Map<number, number>()
    for (const s of series) for (const p of s.points) sums.set(p.t, (sums.get(p.t) ?? 0) + p.v)
    return Math.max(0, ...sums.values())
  }
  return Math.max(0, ...series.flatMap((s) => s.points.map((p) => p.v)))
}

/** The point of each series nearest to time `t` (within `tolerance` ms). */
export function nearest(series: Series[], t: number, tolerance: number): { name: string; color: string; v: number; t: number }[] {
  const out: { name: string; color: string; v: number; t: number }[] = []
  for (const s of series) {
    let best: Point | null = null
    for (const p of s.points) if (!best || Math.abs(p.t - t) < Math.abs(best.t - t)) best = p
    if (best && Math.abs(best.t - t) <= tolerance) out.push({ name: s.name, color: s.color, v: best.v, t: best.t })
  }
  return out
}

export const palette = ['#2563eb', '#e07b00', '#1b8a4b', '#c2410c', '#7c3aed', '#0891b2', '#be185d', '#65a30d', '#64748b', '#ca8a04', '#0f766e', '#9333ea']
