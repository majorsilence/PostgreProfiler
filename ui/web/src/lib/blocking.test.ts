import { describe, expect, it } from 'vitest'
import { blockingTree, type RunningRow } from './blocking'

function r(pid: number, blocked_by: number[] = [], elapsed_ms = 1): RunningRow {
  return {
    pid, usename: 'u', datname: 'd', application_name: null, client_addr: null, state: 'active',
    wait_event_type: null, wait_event: null, queryid: '1', cmd_type: 'select', start_time: null,
    xact_start: null, elapsed_ms, blocked_by, query: 'SELECT 1',
  }
}

describe('blockingTree', () => {
  it('lists unrelated sessions as roots', () => {
    expect(blockingTree([r(1), r(2)]).map((t) => [t.pid, t.depth])).toEqual([[1, 0], [2, 0]])
  })

  it('puts a blocked session under its blocker', () => {
    const t = blockingTree([r(3, [1]), r(1), r(4, [3])])
    expect(t.map((x) => [x.pid, x.depth])).toEqual([[1, 0], [3, 1], [4, 2]])
  })

  it('adds a placeholder for a blocker that is not running a statement', () => {
    const t = blockingTree([r(7, [99]), r(8, [99])])
    expect(t.map((x) => [x.pid, x.depth, x.row === null])).toEqual([[99, 0, true], [7, 1, false], [8, 1, false]])
  })

  it('shows the other blockers of a session blocked by several', () => {
    const t = blockingTree([r(1), r(2), r(3, [1, 2])])
    expect(t.find((x) => x.pid === 3)?.alsoBlockedBy).toEqual([2])
  })

  it('survives a deadlock cycle', () => {
    const t = blockingTree([r(1, [2]), r(2, [1])])
    expect(t.map((x) => x.pid).sort()).toEqual([1, 2])
  })

  it('handles null blocked_by', () => {
    const row = r(1)
    row.blocked_by = null
    expect(blockingTree([row])).toHaveLength(1)
  })
})
