import type { Names, TraceRow } from './trace'
import { databaseName, userName } from './trace'
import { formatBytes, formatMs, formatTime } from './format'

export interface Column {
  id: string
  label: string
  right?: boolean
  default: boolean
  /** Text shown in the grid cell. */
  cell: (r: TraceRow, names: Names) => string
}

const num = (f: (r: TraceRow) => number) => (r: TraceRow) => String(f(r))

export const columns: Column[] = [
  { id: 'start_time', label: 'Time', default: true, cell: (r) => formatTime(r.start_time) },
  { id: 'user', label: 'User', default: true, cell: (r, n) => userName(r, n) },
  { id: 'database', label: 'Database', default: true, cell: (r, n) => databaseName(r, n) },
  { id: 'application_name', label: 'Application', default: true, cell: (r) => r.application_name ?? '' },
  { id: 'pid', label: 'PID', default: true, right: true, cell: (r) => String(r.pid) },
  { id: 'cmd_type', label: 'Type', default: false, cell: (r) => r.cmd_type },
  { id: 'exec_time_ms', label: 'Duration (ms)', default: true, right: true, cell: (r) => formatMs(r.exec_time_ms) },
  { id: 'plan_time_ms', label: 'Planning (ms)', default: false, right: true, cell: (r) => formatMs(r.plan_time_ms) },
  { id: 'cpu_user_ms', label: 'CPU user (ms)', default: false, right: true, cell: (r) => formatMs(r.cpu_user_ms) },
  { id: 'rows', label: 'Rows', default: true, right: true, cell: num((r) => r.rows) },
  { id: 'shared_blks_hit', label: 'Buffer hits', default: false, right: true, cell: num((r) => r.shared_blks_hit) },
  { id: 'shared_blks_read', label: 'Buffer reads', default: false, right: true, cell: num((r) => r.shared_blks_read) },
  { id: 'temp_blks_written', label: 'Temp written', default: false, right: true, cell: num((r) => r.temp_blks_written) },
  { id: 'wal_bytes', label: 'WAL', default: false, right: true, cell: (r) => (r.wal_bytes ? formatBytes(r.wal_bytes) : '') },
  { id: 'nesting', label: 'Level', default: false, right: true, cell: num((r) => r.nesting) },
  { id: 'xid', label: 'XID', default: false, right: true, cell: (r) => (r.xid === null ? '' : String(r.xid)) },
  { id: 'queryid', label: 'Query ID', default: false, cell: (r) => String(r.queryid) },
  { id: 'plan_id', label: 'Plan ID', default: false, cell: (r) => String(r.plan_id) },
  { id: 'sqlstate', label: 'Error', default: true, cell: (r) => r.sqlstate ?? '' },
  { id: 'query', label: 'Statement', default: true, cell: (r) => (r.query ?? '').replace(/\s+/g, ' ') },
]

export const defaultColumnIds = (): string[] => columns.filter((c) => c.default).map((c) => c.id)
