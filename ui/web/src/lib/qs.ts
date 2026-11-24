// Calls to the Query Store endpoints.
import { api } from '../api'
import { iso, type Range } from './range'

export interface Status {
  has_history: boolean
  history_from: string | null
  history_to: string | null
  memory_entries: number
  database: string
}

export interface Overall {
  bucket_start: string
  calls: number
  errors: number
  exec_total_ms: number
  avg_exec_ms: number | null
  rows: number
  shared_blks_read: number
  shared_blks_written: number
  temp_blks_written: number
  wal_bytes: number
  cpu_ms: number
  distinct_queries: number
}

export interface Consumer {
  dbid: string | number
  queryid: string | number
  query: string | null
  calls: number
  errors: number
  total_exec_ms: number
  avg_exec_ms: number | null
  stddev_exec_ms: number | null
  p95_exec_ms: number | null
  rows: number
  shared_blks_read: number
  temp_blks_written: number
  wal_bytes: number
  cpu_ms: number
  io_ms: number
  plans: number
  metric_value: number | null
}

export interface Variation {
  dbid: string | number
  queryid: string | number
  query: string | null
  calls: number
  avg_exec_ms: number
  stddev_exec_ms: number
  cv: number | null
  min_exec_ms: number | null
  max_exec_ms: number | null
  p50_exec_ms: number | null
  p95_exec_ms: number | null
  p99_exec_ms: number | null
}

export interface Regressed {
  dbid: string | number
  queryid: string | number
  query: string | null
  baseline_calls: number
  recent_calls: number
  baseline_avg_ms: number
  recent_avg_ms: number
  ratio: number
  z_score: number | null
  added_time_ms: number
  plan_changed: boolean
  baseline_plans: (string | number)[]
  recent_plans: (string | number)[]
}

export interface Wait {
  dbid: string | number
  queryid: string | number
  query: string | null
  category: string
  samples: number
  pct_of_query: number
}

export interface WaitPoint {
  bucket_start: string
  category: string
  samples: number
}

export interface QueryInterval {
  interval_start: string
  plan_id: string | number
  calls: number
  errors: number
  avg_exec_ms: number | null
  stddev_exec_ms: number | null
  min_exec_ms: number | null
  max_exec_ms: number | null
  p95_exec_ms: number | null
  rows: number
  shared_blks_read: number
  temp_blks_written: number
}

export interface PlanInfo {
  plan_id: string | number
  first_seen: string
  last_seen: string
}

export interface PlanSample {
  ts: string
  plan_id: string | number
  reason: string
  exec_time_ms: number
  max_misestimate: number
  temp_blks_written: number
  format: string
}

export interface PlanText {
  plan: string
  format: string
  reason: string
  ts: string
  has_actuals: boolean
  exec_time_ms?: number
  max_misestimate?: number
}

const q = (params: Record<string, string | number | undefined>): string =>
  Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== '')
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`)
    .join('&')

const span = (r: Range) => ({ from: iso(r.from), to: iso(r.to) })

export const store = {
  status: () => api.get<Status>('/api/qs/status'),
  overall: (r: Range, bucket: number) => api.get<Overall[]>(`/api/qs/overall?${q({ ...span(r), bucket })}`),
  top: (r: Range, metric: string, limit = 25) => api.get<Consumer[]>(`/api/qs/top?${q({ ...span(r), metric, limit })}`),
  variation: (r: Range, minCalls: number, limit = 25) =>
    api.get<Variation[]>(`/api/qs/variation?${q({ ...span(r), min_calls: minCalls, limit })}`),
  regressed: (recent: Range, baseline: Range, minCalls: number, minRatio: number, minZ: number) =>
    api.get<Regressed[]>(
      `/api/qs/regressed?${q({
        recent_from: iso(recent.from), recent_to: iso(recent.to),
        baseline_from: iso(baseline.from), baseline_to: iso(baseline.to),
        min_calls: minCalls, min_ratio: minRatio, min_z: minZ,
      })}`,
    ),
  waits: (r: Range, limit = 50) => api.get<Wait[]>(`/api/qs/waits?${q({ ...span(r), limit })}`),
  waitSeries: (r: Range, bucket: number) => api.get<WaitPoint[]>(`/api/qs/wait-series?${q({ ...span(r), bucket })}`),
  query: (dbid: string, queryid: string, r: Range) =>
    api.get<{ query: string | null; history: QueryInterval[] }>(`/api/qs/query?${q({ dbid, queryid, ...span(r) })}`),
  plans: (dbid: string, queryid: string) =>
    api.get<{ plans: PlanInfo[]; samples: PlanSample[] }>(`/api/qs/query/plans?${q({ dbid, queryid })}`),
  plan: (dbid: string, queryid: string, planId: string | number, sampleAt?: string) =>
    api.get<PlanText | null>(`/api/qs/plan?${q({ dbid, queryid, plan_id: planId, from: sampleAt })}`),
}
