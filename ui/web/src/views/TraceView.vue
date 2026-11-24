<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { api } from '../api'
import { columns, defaultColumnIds } from '../lib/columns'
import { subscribe } from '../lib/feed'
import { formatBytes, formatMs, formatDateTime } from '../lib/format'
import { loadNames, names } from '../lib/names'
import {
  databaseName,
  emptyFilters,
  groupRows,
  isError,
  matches,
  mergeNewest,
  toCsv,
  userName,
  type GroupKey,
  type TraceRow,
} from '../lib/trace'

const MAX_ROWS = 5000
const PAGE = 300

const rows = ref<TraceRow[]>([])
const pending = ref<TraceRow[]>([]) // rows that arrived while paused, oldest first
const paused = ref(false)
const lapped = ref(false)
const loading = ref(true)
const error = ref('')
const filters = reactive(emptyFilters())
const groupBy = ref<GroupKey>('none')
const shown = ref(PAGE)
const selected = ref<TraceRow | null>(null)
const plan = ref<{ plan: string; format: string; reason: string; ts: string; exec_time_ms: number; has_actuals: boolean } | null>(null)
const planState = ref<'idle' | 'loading' | 'missing'>('idle')
const showColumns = ref(false)

// Column choice survives reloads.
function loadColumnIds(): string[] {
  try {
    const saved = JSON.parse(localStorage.getItem('trace.columns') ?? 'null')
    if (Array.isArray(saved) && saved.every((s) => columns.some((c) => c.id === s))) return saved
  } catch {
    // fall through to the defaults
  }
  return defaultColumnIds()
}
const columnIds = ref<string[]>(loadColumnIds())
watch(columnIds, (v) => {
  try {
    localStorage.setItem('trace.columns', JSON.stringify(v))
  } catch {
    // storage may be unavailable
  }
})
const activeColumns = computed(() => columns.filter((c) => columnIds.value.includes(c.id)))

const visible = computed(() => rows.value.filter((r) => matches(r, filters, names.value)))
const groups = computed(() => (groupBy.value === 'none' ? [] : groupRows(visible.value, groupBy.value, names.value)))
const page = computed(() => visible.value.slice(0, shown.value))

watch([filters, groupBy], () => (shown.value = PAGE), { deep: true })

let stop: (() => void) | null = null

onMounted(async () => {
  loadNames()
  try {
    const initial = await api.get<TraceRow[]>('/api/trace?limit=500')
    rows.value = initial
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'could not load the trace'
  } finally {
    loading.value = false
  }
  stop = subscribe((batch) => {
    if (batch.lapped) lapped.value = true
    if (batch.trace.length === 0) return
    if (paused.value) {
      pending.value = pending.value.concat(batch.trace).slice(-MAX_ROWS)
    } else {
      rows.value = mergeNewest(rows.value, batch.trace.slice(), MAX_ROWS)
    }
  })
})

onBeforeUnmount(() => stop?.())

function resume() {
  paused.value = false
  if (pending.value.length) rows.value = mergeNewest(rows.value, pending.value.slice(), MAX_ROWS)
  pending.value = []
}

function clearView() {
  rows.value = []
  pending.value = []
  selected.value = null
  lapped.value = false
}

function resetFilters() {
  Object.assign(filters, emptyFilters())
}

async function select(r: TraceRow) {
  selected.value = r
  plan.value = null
  planState.value = 'idle'
}

async function loadPlan(r: TraceRow) {
  planState.value = 'loading'
  try {
    const p = await api.get<typeof plan.value>(`/api/plan?queryid=${encodeURIComponent(String(r.queryid))}&plan_id=${encodeURIComponent(String(r.plan_id))}`)
    plan.value = p
    planState.value = p ? 'idle' : 'missing'
  } catch {
    planState.value = 'missing'
  }
}

function prettyPlan(p: { plan: string; format: string }): string {
  if (p.format === 'json') {
    try {
      return JSON.stringify(JSON.parse(p.plan), null, 2)
    } catch {
      return p.plan
    }
  }
  return p.plan
}

function exportCsv() {
  const csv = toCsv(visible.value, activeColumns.value, names.value)
  const url = URL.createObjectURL(new Blob([csv], { type: 'text/csv;charset=utf-8' }))
  const a = document.createElement('a')
  a.href = url
  a.download = `pg_profiler_trace_${new Date().toISOString().replace(/[:.]/g, '-')}.csv`
  a.click()
  URL.revokeObjectURL(url)
}

const cmdTypes = ['select', 'insert', 'update', 'delete', 'merge', 'utility']
const hasFilters = computed(() => JSON.stringify(filters) !== JSON.stringify(emptyFilters()))
</script>

<template>
  <section class="trace">
    <div class="toolbar">
      <button v-if="!paused" @click="paused = true" title="Stop adding new rows (they are kept)">⏸ Pause</button>
      <button v-else class="primary" @click="resume">▶ Resume<span v-if="pending.length"> ({{ pending.length }} new)</span></button>
      <button @click="clearView" title="Empty this view (the server's buffer is not touched)">Clear view</button>
      <button @click="exportCsv" :disabled="!visible.length">Export CSV</button>
      <div class="dropdown">
        <button @click="showColumns = !showColumns" :aria-expanded="showColumns">Columns ▾</button>
        <div v-if="showColumns" class="menu card" @mouseleave="showColumns = false">
          <label v-for="c in columns" :key="c.id" class="check">
            <input type="checkbox" :value="c.id" v-model="columnIds" /> {{ c.label }}
          </label>
          <button class="link" @click="columnIds = defaultColumnIds()">Defaults</button>
        </div>
      </div>
      <label class="inline">
        Group by
        <select v-model="groupBy">
          <option value="none">Nothing (every execution)</option>
          <option value="queryid">Statement</option>
          <option value="application_name">Application</option>
          <option value="user">User</option>
          <option value="pid">Session (backend)</option>
        </select>
      </label>
      <div class="spacer"></div>
      <span class="muted">{{ visible.length }} of {{ rows.length }} rows<span v-if="paused"> · paused</span></span>
    </div>

    <div class="filters">
      <input v-model="filters.text" type="search" placeholder="Find in statements and parameters…" aria-label="Find" />
      <input v-model="filters.app" placeholder="Application" aria-label="Application filter" />
      <input v-model="filters.user" placeholder="User" aria-label="User filter" />
      <input v-model="filters.database" placeholder="Database" aria-label="Database filter" />
      <select v-model="filters.cmd" aria-label="Type filter">
        <option value="">Any type</option>
        <option v-for="c in cmdTypes" :key="c" :value="c">{{ c }}</option>
      </select>
      <label class="inline">≥ <input v-model.number="filters.minMs" type="number" min="0" step="1" class="narrow" aria-label="Minimum duration in ms" /> ms</label>
      <label class="check"><input v-model="filters.errorsOnly" type="checkbox" /> errors only</label>
      <label class="check"><input v-model="filters.topLevelOnly" type="checkbox" /> top level only</label>
      <button v-if="hasFilters" class="link" @click="resetFilters">Reset filters</button>
    </div>

    <p v-if="lapped" class="notice">
      Some rows were overwritten before they could be read (the server's buffer wrapped). Raise
      <code>pg_profiler.buffer_size</code> or filter at the source with capture rules.
      <button class="link" @click="lapped = false">Dismiss</button>
    </p>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-if="loading" class="muted">Loading…</p>

    <div class="split" :class="{ open: selected }">
      <div class="grid-wrap">
        <!-- grouped -->
        <table v-if="groupBy !== 'none'" class="grid">
          <thead>
            <tr>
              <th>{{ groupBy === 'queryid' ? 'Statement' : groupBy === 'pid' ? 'Session' : groupBy === 'user' ? 'User' : 'Application' }}</th>
              <th class="r">Executions</th>
              <th class="r">Total (ms)</th>
              <th class="r">Average (ms)</th>
              <th class="r">Max (ms)</th>
              <th class="r">Rows</th>
              <th class="r">Errors</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="g in groups.slice(0, shown)" :key="g.key">
              <td class="stmt" :title="g.label">{{ g.label }}</td>
              <td class="r">{{ g.count }}</td>
              <td class="r">{{ formatMs(g.total_ms) }}</td>
              <td class="r">{{ formatMs(g.avg_ms) }}</td>
              <td class="r">{{ formatMs(g.max_ms) }}</td>
              <td class="r">{{ g.rows }}</td>
              <td class="r" :class="{ bad: g.errors }">{{ g.errors || '' }}</td>
            </tr>
          </tbody>
        </table>

        <!-- every execution -->
        <table v-else class="grid">
          <thead>
            <tr>
              <th v-for="c in activeColumns" :key="c.id" :class="{ r: c.right }">{{ c.label }}</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="r in page"
              :key="r.seq"
              :class="{ err: isError(r), sel: selected?.seq === r.seq }"
              tabindex="0"
              @click="select(r)"
              @keydown.enter="select(r)"
            >
              <td v-for="c in activeColumns" :key="c.id" :class="[{ r: c.right }, c.id === 'query' ? 'stmt' : '']">
                {{ c.cell(r, names) }}
              </td>
            </tr>
          </tbody>
        </table>
        <p v-if="!loading && !visible.length" class="muted pad">
          {{ rows.length ? 'No rows match the filters.' : 'Nothing recorded yet. Run some statements.' }}
        </p>
        <button v-if="(groupBy === 'none' ? visible.length : groups.length) > shown" class="more" @click="shown += PAGE">
          Show {{ PAGE }} more
        </button>
      </div>

      <aside v-if="selected" class="detail card" aria-label="Execution details">
        <header>
          <h2>Execution {{ selected.seq }}</h2>
          <button class="link" @click="selected = null" aria-label="Close details">✕</button>
        </header>
        <dl>
          <dt>Started</dt><dd>{{ formatDateTime(selected.start_time) }}</dd>
          <dt>User / database</dt><dd>{{ userName(selected, names) }} @ {{ databaseName(selected, names) }}</dd>
          <dt>Application</dt><dd>{{ selected.application_name || '–' }} <small v-if="selected.client_addr">from {{ selected.client_addr }}</small></dd>
          <dt>Session</dt><dd>pid {{ selected.pid }}<span v-if="selected.xid"> · xid {{ selected.xid }}</span> · level {{ selected.nesting }}</dd>
          <dt>Duration</dt><dd>{{ formatMs(selected.exec_time_ms) }} ms <small>(planning {{ formatMs(selected.plan_time_ms) }} ms, CPU {{ formatMs(selected.cpu_user_ms + selected.cpu_sys_ms) }} ms)</small></dd>
          <dt>Rows</dt><dd>{{ selected.rows }}</dd>
          <dt>Buffers</dt><dd>{{ selected.shared_blks_hit }} hit · {{ selected.shared_blks_read }} read · {{ selected.shared_blks_dirtied }} dirtied · {{ selected.shared_blks_written }} written<span v-if="selected.temp_blks_written"> · temp {{ selected.temp_blks_read }}/{{ selected.temp_blks_written }}</span></dd>
          <dt>WAL</dt><dd>{{ selected.wal_records }} records · {{ formatBytes(selected.wal_bytes) }}</dd>
          <dt>Query ID</dt><dd>{{ selected.queryid }}</dd>
          <dt>Plan ID</dt><dd>{{ selected.plan_id }}</dd>
          <template v-if="isError(selected)"><dt>Error</dt><dd class="bad">SQLSTATE {{ selected.sqlstate }}</dd></template>
        </dl>
        <h3>Statement</h3>
        <pre class="sql">{{ selected.query }}</pre>
        <template v-if="selected.params">
          <h3>Parameters</h3>
          <pre class="sql">{{ selected.params }}</pre>
        </template>
        <p v-if="String(selected.queryid) !== '0'">
          <RouterLink :to="`/store/query/${selected.dbid}/${selected.queryid}`">Open in Query Store (history, plans, trend) →</RouterLink>
        </p>
        <h3>Plan</h3>
        <button v-if="!plan && planState !== 'loading'" @click="loadPlan(selected)" :disabled="String(selected.plan_id) === '0'">Load plan</button>
        <p v-if="planState === 'loading'" class="muted">Loading…</p>
        <p v-if="planState === 'missing'" class="muted">No stored plan for this statement. Plans are kept in a small buffer and may have been overwritten.</p>
        <template v-if="plan">
          <p class="muted">{{ plan.reason }} · {{ formatDateTime(plan.ts) }}<span v-if="plan.has_actuals"> · with actual rows</span></p>
          <pre class="sql">{{ prettyPlan(plan) }}</pre>
        </template>
      </aside>
    </div>
  </section>
</template>
