import { describe, expect, it } from 'vitest'
import { csvCell, emptyFilters, groupRows, matches, mergeNewest, toCsv, type Names, type TraceRow } from './trace'

const names: Names = { roles: new Map([['10', 'alice']]), databases: new Map([['5', 'shop']]) }

function row(over: Partial<TraceRow> = {}): TraceRow {
  return {
    seq: 1, start_time: '2026-01-01T00:00:00Z', xact_start: null, userid: '10', dbid: '5', pid: 100,
    application_name: 'web', client_addr: '10.0.0.1', xid: null, queryid: '1', plan_id: '2', cmd_type: 'select',
    nesting: 0, exec_time_ms: 5, plan_time_ms: 0, cpu_user_ms: 0, cpu_sys_ms: 0, rows: 1,
    shared_blks_hit: 0, shared_blks_read: 0, shared_blks_dirtied: 0, shared_blks_written: 0,
    temp_blks_read: 0, temp_blks_written: 0, blk_read_time_ms: 0, blk_write_time_ms: 0,
    wal_records: 0, wal_fpi: 0, wal_bytes: 0, sqlstate: null, query: 'SELECT 1', params: null,
    ...over,
  }
}

describe('matches', () => {
  it('passes everything with empty filters', () => {
    expect(matches(row(), emptyFilters(), names)).toBe(true)
  })
  it('filters by duration, errors and nesting', () => {
    expect(matches(row({ exec_time_ms: 5 }), { ...emptyFilters(), minMs: 10 }, names)).toBe(false)
    expect(matches(row({ exec_time_ms: 50 }), { ...emptyFilters(), minMs: 10 }, names)).toBe(true)
    expect(matches(row(), { ...emptyFilters(), errorsOnly: true }, names)).toBe(false)
    expect(matches(row({ sqlstate: '22012' }), { ...emptyFilters(), errorsOnly: true }, names)).toBe(true)
    expect(matches(row({ nesting: 1 }), { ...emptyFilters(), topLevelOnly: true }, names)).toBe(false)
  })
  it('searches statement text case-insensitively, and names of users and databases', () => {
    expect(matches(row({ query: 'SELECT * FROM Orders' }), { ...emptyFilters(), text: 'orders' }, names)).toBe(true)
    expect(matches(row(), { ...emptyFilters(), text: 'nothing' }, names)).toBe(false)
    expect(matches(row(), { ...emptyFilters(), user: 'ali' }, names)).toBe(true)
    expect(matches(row(), { ...emptyFilters(), database: 'zzz' }, names)).toBe(false)
    expect(matches(row(), { ...emptyFilters(), app: 'WEB' }, names)).toBe(true)
  })
  it('handles missing text', () => {
    expect(matches(row({ query: null }), { ...emptyFilters(), text: 'x' }, names)).toBe(false)
  })
})

describe('groupRows', () => {
  const rows = [
    row({ seq: 1, queryid: 'a', exec_time_ms: 10, query: 'SELECT  a' }),
    row({ seq: 2, queryid: 'a', exec_time_ms: 30, sqlstate: '40001' }),
    row({ seq: 3, queryid: 'b', exec_time_ms: 5, pid: 200 }),
  ]
  it('groups by statement, slowest total first', () => {
    const g = groupRows(rows, 'queryid', names)
    expect(g.map((x) => x.key)).toEqual(['a', 'b'])
    expect(g[0]).toMatchObject({ count: 2, total_ms: 40, avg_ms: 20, max_ms: 30, errors: 1 })
    expect(g[0].label).toBe('SELECT a')
  })
  it('groups by session and by user', () => {
    expect(groupRows(rows, 'pid', names).map((x) => x.label)).toEqual(['backend 100', 'backend 200'])
    expect(groupRows(rows, 'user', names)[0].label).toBe('alice')
  })
})

describe('csv', () => {
  it('quotes cells and defuses formulas', () => {
    expect(csvCell('a,b')).toBe('"a,b"')
    expect(csvCell('say "hi"')).toBe('"say ""hi"""')
    expect(csvCell('=1+1')).toBe("'=1+1")
    expect(csvCell(null)).toBe('')
    expect(csvCell(12.5)).toBe('12.5')
  })
  it('writes a header and one line per row, with names for user and database', () => {
    const csv = toCsv([row()], [{ id: 'user', label: 'User' }, { id: 'database', label: 'Database' }, { id: 'query', label: 'Query' }], names)
    expect(csv).toBe('User,Database,Query\r\nalice,shop,SELECT 1\r\n')
  })
})

describe('mergeNewest', () => {
  it('puts new rows first, newest on top, and ignores rows already seen', () => {
    const existing = [row({ seq: 5 }), row({ seq: 4 })]
    const merged = mergeNewest(existing, [row({ seq: 5 }), row({ seq: 6 }), row({ seq: 7 })], 100)
    expect(merged.map((r) => r.seq)).toEqual([7, 6, 5, 4])
  })
  it('returns the same array when nothing is new, and caps the length', () => {
    const existing = [row({ seq: 5 })]
    expect(mergeNewest(existing, [row({ seq: 5 })], 100)).toBe(existing)
    expect(mergeNewest(existing, [row({ seq: 6 }), row({ seq: 7 })], 2).map((r) => r.seq)).toEqual([7, 6])
  })
})
