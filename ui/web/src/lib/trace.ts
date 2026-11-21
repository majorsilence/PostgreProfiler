// Pure helpers for the live trace: filtering, grouping, CSV export.

export interface TraceRow {
  seq: number
  start_time: string | null
  xact_start: string | null
  /** OIDs arrive as JSON strings from the database. */
  userid: number | string
  dbid: number | string
  pid: number
  application_name: string | null
  client_addr: string | null
  xid: number | null
  /** 64-bit ids can exceed 2^53, so the server sends large ones as strings. */
  queryid: string | number
  plan_id: string | number
  cmd_type: string
  nesting: number
  exec_time_ms: number
  plan_time_ms: number
  cpu_user_ms: number
  cpu_sys_ms: number
  rows: number
  shared_blks_hit: number
  shared_blks_read: number
  shared_blks_dirtied: number
  shared_blks_written: number
  temp_blks_read: number
  temp_blks_written: number
  blk_read_time_ms: number
  blk_write_time_ms: number
  wal_records: number
  wal_fpi: number
  wal_bytes: number
  sqlstate: string | null
  query: string | null
  params: string | null
}

/** Role and database names by OID (as strings, which is how OIDs arrive). */
export interface Names {
  roles: Map<string, string>
  databases: Map<string, string>
}

export interface Filters {
  text: string
  minMs: number
  app: string
  user: string
  database: string
  cmd: string
  errorsOnly: boolean
  topLevelOnly: boolean
}

export const emptyFilters = (): Filters => ({
  text: '',
  minMs: 0,
  app: '',
  user: '',
  database: '',
  cmd: '',
  errorsOnly: false,
  topLevelOnly: false,
})

export const isError = (r: TraceRow): boolean => r.sqlstate !== null && r.sqlstate !== ''

const has = (hay: string | null | undefined, needle: string): boolean =>
  needle === '' || (hay ?? '').toLowerCase().includes(needle.toLowerCase())

export function userName(r: TraceRow, names: Names): string {
  return names.roles.get(String(r.userid)) ?? String(r.userid)
}

export function databaseName(r: TraceRow, names: Names): string {
  return names.databases.get(String(r.dbid)) ?? String(r.dbid)
}

/** Does a row pass all filters? Text search looks at the statement, parameters and application. */
export function matches(r: TraceRow, f: Filters, names: Names): boolean {
  if (f.errorsOnly && !isError(r)) return false
  if (f.topLevelOnly && r.nesting !== 0) return false
  if (f.minMs > 0 && r.exec_time_ms < f.minMs) return false
  if (f.cmd && r.cmd_type !== f.cmd) return false
  if (f.app && !has(r.application_name, f.app)) return false
  if (f.user && !has(userName(r, names), f.user)) return false
  if (f.database && !has(databaseName(r, names), f.database)) return false
  if (f.text) {
    const t = f.text
    if (!(has(r.query, t) || has(r.params, t) || has(r.application_name, t) || has(r.sqlstate, t))) return false
  }
  return true
}

export type GroupKey = 'none' | 'queryid' | 'application_name' | 'user' | 'pid'

export interface Group {
  key: string
  label: string
  count: number
  total_ms: number
  avg_ms: number
  max_ms: number
  errors: number
  rows: number
  last: string | null
}

/** Aggregate rows by statement, application, user or session, slowest total first. */
export function groupRows(rows: TraceRow[], by: GroupKey, names: Names): Group[] {
  const map = new Map<string, Group>()
  for (const r of rows) {
    let key: string
    let label: string
    switch (by) {
      case 'queryid':
        key = String(r.queryid)
        label = (r.query ?? '').replace(/\s+/g, ' ').slice(0, 120) || key
        break
      case 'application_name':
        key = r.application_name ?? ''
        label = key || '(none)'
        break
      case 'user':
        key = userName(r, names)
        label = key
        break
      case 'pid':
        key = String(r.pid)
        label = `backend ${r.pid}`
        break
      default:
        key = String(r.seq)
        label = key
    }
    let g = map.get(key)
    if (!g) {
      g = { key, label, count: 0, total_ms: 0, avg_ms: 0, max_ms: 0, errors: 0, rows: 0, last: null }
      map.set(key, g)
    }
    g.count++
    g.total_ms += r.exec_time_ms
    g.max_ms = Math.max(g.max_ms, r.exec_time_ms)
    g.rows += r.rows
    if (isError(r)) g.errors++
    if (!g.last || (r.start_time ?? '') > g.last) g.last = r.start_time
  }
  const out = [...map.values()]
  for (const g of out) g.avg_ms = g.count ? g.total_ms / g.count : 0
  return out.sort((a, b) => b.total_ms - a.total_ms)
}

/** A CSV cell. Cells that a spreadsheet would run as a formula get a leading quote. */
export function csvCell(v: unknown): string {
  let s = v === null || v === undefined ? '' : String(v)
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`
  return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s
}

export function toCsv(rows: TraceRow[], columns: { id: string; label: string }[], names: Names): string {
  const value = (r: TraceRow, id: string): unknown => {
    if (id === 'user') return userName(r, names)
    if (id === 'database') return databaseName(r, names)
    return (r as unknown as Record<string, unknown>)[id]
  }
  const lines = [columns.map((c) => csvCell(c.label)).join(',')]
  for (const r of rows) lines.push(columns.map((c) => csvCell(value(r, c.id))).join(','))
  return lines.join('\r\n') + '\r\n'
}

/** Keep at most `max` rows, newest first, and drop duplicates by sequence number. */
export function mergeNewest(existing: TraceRow[], incomingOldestFirst: TraceRow[], max: number): TraceRow[] {
  const top = existing.length ? existing[0].seq : -1
  const fresh = incomingOldestFirst.filter((r) => r.seq > top)
  if (fresh.length === 0) return existing
  const merged = fresh.reverse().concat(existing)
  return merged.length > max ? merged.slice(0, max) : merged
}
