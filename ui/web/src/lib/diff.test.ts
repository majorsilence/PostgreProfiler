import { describe, expect, it } from 'vitest'
import { changed, diffLines, diffPlans, normalizePlan } from './diff'

describe('diffLines', () => {
  it('marks added and removed lines and keeps the rest', () => {
    const d = diffLines(['a', 'b', 'c'], ['a', 'x', 'c', 'd'])
    expect(d.map((l) => `${l.kind}:${l.text}`)).toEqual(['same:a', 'del:b', 'add:x', 'same:c', 'add:d'])
  })
  it('finds nothing to report for equal input, and handles empty sides', () => {
    expect(changed(diffLines(['a'], ['a']))).toBe(false)
    expect(diffLines([], ['a']).map((l) => l.kind)).toEqual(['add'])
    expect(diffLines(['a'], []).map((l) => l.kind)).toEqual(['del'])
  })
})

describe('normalizePlan', () => {
  it('drops costs, actuals and timings but keeps the structure', () => {
    const t = `Seq Scan on t  (cost=0.00..1.05 rows=5 width=4) (actual time=0.01..0.02 rows=5 loops=1)
  Filter: (a > 1)
  Rows Removed by Filter: 12
Planning Time: 0.1 ms
Execution Time: 0.3 ms`
    expect(normalizePlan(t)).toBe('Seq Scan on t\n  Filter: (a > ?)')
  })
})

describe('constants', () => {
  it('are masked in condition lines only', () => {
    expect(normalizePlan("Index Scan using i on t\n  Index Cond: (id = 42)\n  Filter: (name = 'bob'::text)")).toBe(
      'Index Scan using i on t\n  Index Cond: (id = ?)\n  Filter: (name = ?::text)',
    )
    expect(normalizePlan('Seq Scan on t1')).toBe('Seq Scan on t1')
    expect(normalizePlan('  Index Cond: (t1.id = t2.id)')).toBe('  Index Cond: (t1.id = t2.id)')
  })
})

describe('diffPlans', () => {
  const a = 'Index Scan using i on t  (cost=0.1..8.1 rows=1 width=4)\n  Index Cond: (id = 1)'
  const b = 'Seq Scan on t  (cost=0.0..99.0 rows=1 width=4)\n  Filter: (id = 1)'
  it('shows a plan change', () => {
    expect(changed(diffPlans(a, b, true))).toBe(true)
  })
  it('ignores a different constant when asked to', () => {
    const c = 'Index Scan using i on t  (cost=0.1..8.1 rows=1 width=4)\n  Index Cond: (id = 99)'
    expect(changed(diffPlans(a, c, true))).toBe(false)
    expect(changed(diffPlans(a, c, false))).toBe(true)
  })
  it('ignores estimate-only differences when asked to', () => {
    const c = 'Index Scan using i on t  (cost=0.2..9.9 rows=3 width=4)\n  Index Cond: (id = 1)'
    expect(changed(diffPlans(a, c, true))).toBe(false)
    expect(changed(diffPlans(a, c, false))).toBe(true)
  })
})
