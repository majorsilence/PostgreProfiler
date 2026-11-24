import { describe, expect, it } from 'vitest'
import { flatten, misestimate, nodeLabel, parsePlan } from './plan'

const TEXT = `Aggregate  (cost=1167.57..1167.58 rows=1 width=8) (actual time=3.1..3.1 rows=1 loops=1)
  ->  Nested Loop  (cost=0.42..1166.88 rows=277 width=0) (actual time=0.05..3.0 rows=277 loops=1)
        ->  Seq Scan on pgbench_history h  (cost=0.00..17.00 rows=277 width=4) (actual time=0.01..0.1 rows=277 loops=1)
              Filter: (delta > 0)
        ->  Index Only Scan using pgbench_accounts_pkey on pgbench_accounts a  (cost=0.42..4.14 rows=1 width=4) (actual time=0.01..0.01 rows=1 loops=277)
              Index Cond: (aid = h.aid)
              Heap Fetches: 0
Planning Time: 0.2 ms
Execution Time: 3.2 ms`

describe('parsePlan (text)', () => {
  const plan = parsePlan(TEXT)!

  it('builds the tree', () => {
    expect(plan.root.type).toBe('Aggregate')
    expect(plan.root.children).toHaveLength(1)
    const loop = plan.root.children[0]
    expect(loop.type).toBe('Nested Loop')
    expect(loop.children.map((c) => c.type)).toEqual(['Seq Scan', 'Index Only Scan'])
  })

  it('reads relation, alias, index, costs and actuals', () => {
    const [seq, idx] = plan.root.children[0].children
    expect(seq).toMatchObject({ relation: 'pgbench_history', alias: 'h', estRows: 277, totalCost: 17, actualRows: 277, loops: 1 })
    expect(idx).toMatchObject({ index: 'pgbench_accounts_pkey', relation: 'pgbench_accounts', alias: 'a', loops: 277 })
    expect(nodeLabel(idx)).toBe('Index Only Scan using pgbench_accounts_pkey on pgbench_accounts a')
    expect(plan.hasActuals).toBe(true)
    expect(plan.maxCost).toBeCloseTo(1167.58)
  })

  it('attaches property lines to their node and keeps the summary apart', () => {
    const [seq, idx] = plan.root.children[0].children
    expect(seq.details).toEqual(['Filter: (delta > 0)'])
    expect(idx.details).toEqual(['Index Cond: (aid = h.aid)', 'Heap Fetches: 0'])
    expect(plan.summary).toEqual(['Planning Time: 0.2 ms', 'Execution Time: 3.2 ms'])
  })

  it('numbers every node', () => {
    const ids = flatten(plan.root).map((n) => n.id)
    expect(new Set(ids).size).toBe(ids.length)
  })
})

describe('parsePlan (edge cases)', () => {
  it('handles a single node without actuals', () => {
    const p = parsePlan('Seq Scan on t  (cost=0.00..1.05 rows=5 width=4)')!
    expect(p.hasActuals).toBe(false)
    expect(p.root).toMatchObject({ type: 'Seq Scan', relation: 't', estRows: 5 })
  })

  it('handles never executed nodes and fractional rows', () => {
    const p = parsePlan(`Limit  (cost=0.00..1.00 rows=1 width=4) (actual time=0.0..0.0 rows=1.00 loops=1)
  ->  Seq Scan on t  (cost=0.00..1.00 rows=1 width=4) (never executed)`)!
    expect(p.root.actualRows).toBe(1)
    expect(p.root.children[0].neverExecuted).toBe(true)
    expect(p.root.children[0].flags.map((f) => f.kind)).toContain('never')
  })

  it('groups InitPlan children', () => {
    const p = parsePlan(`Result  (cost=0.01..0.02 rows=1 width=4)
  InitPlan 1
    ->  Seq Scan on t  (cost=0.00..1.00 rows=1 width=4)`)!
    expect(p.root.children[0].type).toBe('InitPlan 1')
    expect(p.root.children[0].children[0].relation).toBe('t')
  })

  it('returns null for text that is not a plan', () => {
    expect(parsePlan('')).toBeNull()
    expect(parsePlan('[ not json')).toBeNull()
    expect(parsePlan('{"a": 1}')).toBeNull()
  })
})

describe('parsePlan (JSON)', () => {
  const json = JSON.stringify([
    {
      Plan: {
        'Node Type': 'Sort', 'Startup Cost': 10, 'Total Cost': 20, 'Plan Rows': 1000, 'Plan Width': 8,
        'Actual Rows': 1000, 'Actual Loops': 1, 'Actual Total Time': 5, 'Sort Method': 'external merge', 'Sort Space Used': 4000, 'Sort Space Type': 'Disk',
        Plans: [{ 'Node Type': 'Seq Scan', 'Relation Name': 'big', Alias: 'big', 'Plan Rows': 10, 'Actual Rows': 5000, 'Actual Loops': 1, 'Total Cost': 5, 'Startup Cost': 0, 'Plan Width': 4 }],
      },
      'Planning Time': 0.1,
    },
  ])

  it('maps the tree and the summary', () => {
    const p = parsePlan(json)!
    expect(p.root.type).toBe('Sort')
    expect(p.root.children[0]).toMatchObject({ type: 'Seq Scan', relation: 'big' })
    expect(p.root.children[0].alias).toBeUndefined()
    expect(p.summary).toEqual(['Planning Time: 0.1 ms'])
    expect(p.hasActuals).toBe(true)
  })

  it('flags a misestimate (but not tiny row counts) and a spill', () => {
    const p = parsePlan(json)!
    const scan = p.root.children[0]
    expect(misestimate(scan)).toBe(500)
    expect(scan.flags.find((f) => f.kind === 'misestimate')?.text).toBe('underestimated 500×')
    expect(misestimate({ ...scan, estRows: 2, actualRows: 50 })).toBe(0)
    expect(p.root.flags.some((f) => f.kind === 'spill')).toBe(false) // "Disk" is in a value, not a "Disk:" line
  })
})

describe('flags', () => {
  it('flags a spilling sort and a large sequential scan in text', () => {
    const p = parsePlan(`Sort  (cost=1.00..2.00 rows=10 width=4) (actual time=1..2 rows=10 loops=1)
  Sort Method: external merge  Disk: 1234kB
  ->  Seq Scan on big  (cost=0.00..9999.00 rows=500000 width=4)`)!
    expect(p.root.flags.find((f) => f.kind === 'spill')?.text).toBe('spilled to disk (1234kB)')
    expect(p.root.children[0].flags.find((f) => f.kind === 'seqscan')).toBeTruthy()
  })
})
