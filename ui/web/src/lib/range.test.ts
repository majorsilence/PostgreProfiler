import { describe, expect, it } from 'vitest'
import { compact, formatTick, maxValue, nearest, niceTicks, timeTicks } from './chart'
import { bucketSeconds, previous, resolve, toLocalInput } from './range'

const now = new Date('2026-03-01T12:00:00Z')

describe('range', () => {
  it('resolves a preset to a window ending now', () => {
    const r = resolve({ preset: '1h' }, now)
    expect(r.to).toEqual(now)
    expect(r.to.getTime() - r.from.getTime()).toBe(3_600_000)
  })
  it('uses a custom range when it is valid and falls back otherwise', () => {
    const r = resolve({ preset: 'custom', from: '2026-01-01T00:00:00Z', to: '2026-01-02T00:00:00Z' }, now)
    expect(r.from.toISOString()).toBe('2026-01-01T00:00:00.000Z')
    expect(resolve({ preset: 'custom', from: 'nonsense', to: 'x' }, now).to).toEqual(now)
    expect(resolve({ preset: 'custom', from: '2026-01-02T00:00:00Z', to: '2026-01-01T00:00:00Z' }, now).to).toEqual(now)
    expect(resolve({ preset: 'unknown' }, now).to.getTime() - resolve({ preset: 'unknown' }, now).from.getTime()).toBe(3_600_000)
  })
  it('gives the period just before', () => {
    const r = resolve({ preset: '6h' }, now)
    const p = previous(r)
    expect(p.to).toEqual(r.from)
    expect(p.to.getTime() - p.from.getTime()).toBe(6 * 3_600_000)
  })
  it('picks a bucket that gives at most 120 buckets', () => {
    expect(bucketSeconds(resolve({ preset: '15m' }, now))).toBe(60)
    expect(bucketSeconds(resolve({ preset: '24h' }, now))).toBe(900)
    expect(bucketSeconds(resolve({ preset: '7d' }, now))).toBe(7200)
  })
  it('formats a local input value', () => {
    expect(toLocalInput(new Date(2026, 2, 1, 9, 5))).toBe('2026-03-01T09:05')
  })
})

describe('chart maths', () => {
  it('makes round ticks that cover the data', () => {
    expect(niceTicks(0, 97)).toEqual([0, 20, 40, 60, 80, 100])
    expect(niceTicks(0, 0)).toEqual([0, 1])
    const t = niceTicks(3, 7.2, 5)
    expect(t[0]).toBeLessThanOrEqual(3)
    expect(t[t.length - 1]).toBeGreaterThanOrEqual(7.2)
  })
  it('makes time ticks on round multiples', () => {
    const from = Date.UTC(2026, 2, 1, 10, 7)
    const ticks = timeTicks(from, from + 3_600_000, 6)
    expect(ticks.length).toBeGreaterThan(2)
    expect(ticks.every((t) => t % 300_000 === 0)).toBe(true)
  })
  it('formats ticks by span', () => {
    expect(formatTick(new Date(2026, 2, 1, 9, 5).getTime(), 3_600_000)).toBe('09:05')
    expect(formatTick(new Date(2026, 2, 3, 9, 5).getTime(), 7 * 86_400_000)).toBe('3/3')
  })
  it('abbreviates numbers', () => {
    expect(compact(1234)).toBe('1.2k')
    expect(compact(2_500_000)).toBe('2.5M')
    expect(compact(0.456)).toBe('0.46')
    expect(compact(12)).toBe('12')
  })
  it('finds the maximum, stacked or not, and the nearest points', () => {
    const s = [
      { name: 'a', color: 'x', points: [{ t: 1, v: 2 }, { t: 2, v: 5 }] },
      { name: 'b', color: 'y', points: [{ t: 1, v: 4 }, { t: 2, v: 1 }] },
    ]
    expect(maxValue(s, false)).toBe(5)
    expect(maxValue(s, true)).toBe(6)
    expect(nearest(s, 1.9, 1).map((p) => [p.name, p.v])).toEqual([['a', 5], ['b', 1]])
    expect(nearest(s, 100, 1)).toEqual([])
  })
})
