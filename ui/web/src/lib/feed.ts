// One shared EventSource for the live trace and the events; every screen that
// wants rows subscribes, and the connection closes when the last one leaves.
import { ref } from 'vue'
import type { TraceRow } from './trace'

export interface EventRow {
  seq: number
  ts: string | null
  kind: string
  pid: number
  userid: number | string
  dbid: number | string
  application_name: string | null
  client_addr: string | null
  sqlstate: string | null
  duration_ms: number
  detail: string | null
}

export interface FeedBatch {
  trace: TraceRow[]
  events: EventRow[]
  /** Rows were overwritten before they could be read. */
  lapped: boolean
}

export const feedStatus = ref<'idle' | 'connecting' | 'live' | 'down'>('idle')

const listeners = new Set<(b: FeedBatch) => void>()
let source: EventSource | null = null

function open() {
  feedStatus.value = 'connecting'
  source = new EventSource('/api/trace/stream')
  source.onopen = () => (feedStatus.value = 'live')
  // The browser reconnects by itself; just show that the feed is interrupted.
  source.onerror = () => (feedStatus.value = 'down')
  source.addEventListener('batch', (e) => {
    try {
      const batch = JSON.parse((e as MessageEvent).data) as FeedBatch
      for (const fn of listeners) fn(batch)
    } catch {
      // ignore a malformed batch
    }
  })
  source.addEventListener('lagged', () => {
    for (const fn of listeners) fn({ trace: [], events: [], lapped: true })
  })
}

export function subscribe(fn: (b: FeedBatch) => void): () => void {
  listeners.add(fn)
  if (!source) open()
  return () => {
    listeners.delete(fn)
    if (listeners.size === 0 && source) {
      source.close()
      source = null
      feedStatus.value = 'idle'
    }
  }
}
