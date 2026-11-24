// Small helpers shared by the Query Store screens.
import { ref } from 'vue'
import { store, type Status } from './qs'

export const metricLabels: Record<string, string> = {
  exec_time: 'Total execution time',
  avg_time: 'Average execution time',
  calls: 'Executions',
  rows: 'Rows',
  reads: 'Blocks read',
  temp: 'Temp blocks written',
  wal: 'WAL bytes',
  cpu: 'CPU time',
  io_time: 'I/O time',
  plan_time: 'Planning time',
}

export const metricUnit: Record<string, string> = {
  exec_time: 'ms', avg_time: 'ms', cpu: 'ms', io_time: 'ms', plan_time: 'ms', wal: 'B',
}

export const status = ref<Status | null>(null)

/** Is there anything to show? Loaded once; the screens show a hint if not. */
export async function loadStatus(): Promise<void> {
  try {
    status.value = await store.status()
  } catch {
    status.value = null
  }
}

export const queryPath = (dbid: string | number, queryid: string | number): string => `/store/query/${dbid}/${queryid}`
