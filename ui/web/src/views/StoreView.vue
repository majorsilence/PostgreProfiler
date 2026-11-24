<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import Chart from '../components/Chart.vue'
import RangePicker from '../components/RangePicker.vue'
import StoreNotice from '../components/StoreNotice.vue'
import { compact, palette, type Series } from '../lib/chart'
import { formatBytes, formatDuration, formatMs, oneLine } from '../lib/format'
import { store, type Consumer, type Overall } from '../lib/qs'
import { bucketSeconds } from '../lib/range'
import { range, startAutoRefresh } from '../lib/rangeState'
import { metricLabels, metricUnit, queryPath } from '../lib/storeCommon'

const router = useRouter()
const overall = ref<Overall[]>([])
const top = ref<Consumer[]>([])
const metric = ref('exec_time')
const chartMetric = ref<keyof Overall>('exec_total_ms')
const error = ref('')
const loading = ref(true)
const stop = startAutoRefresh()
onBeforeUnmount(stop)

const bucket = computed(() => bucketSeconds(range.value))

const chartChoices: { id: keyof Overall; label: string; unit?: string }[] = [
  { id: 'exec_total_ms', label: 'Execution time', unit: ' ms' },
  { id: 'calls', label: 'Executions' },
  { id: 'avg_exec_ms', label: 'Average time', unit: ' ms' },
  { id: 'errors', label: 'Errors' },
  { id: 'rows', label: 'Rows' },
  { id: 'shared_blks_read', label: 'Blocks read' },
  { id: 'temp_blks_written', label: 'Temp blocks written' },
  { id: 'wal_bytes', label: 'WAL', unit: ' B' },
  { id: 'cpu_ms', label: 'CPU time', unit: ' ms' },
  { id: 'distinct_queries', label: 'Distinct queries' },
]
const chosen = computed(() => chartChoices.find((c) => c.id === chartMetric.value) ?? chartChoices[0])

const series = computed<Series[]>(() => [
  {
    name: chosen.value.label,
    color: palette[0],
    points: overall.value
      .filter((o) => o[chartMetric.value] !== null)
      .map((o) => ({ t: new Date(o.bucket_start).getTime(), v: Number(o[chartMetric.value]) })),
  },
])

let seq = 0
async function load() {
  const mine = ++seq
  loading.value = true
  try {
    const [o, t] = await Promise.all([store.overall(range.value, bucket.value), store.top(range.value, metric.value)])
    if (mine !== seq) return
    overall.value = o
    top.value = t
    error.value = ''
  } catch (e) {
    if (mine === seq) error.value = e instanceof Error ? e.message : 'could not load'
  } finally {
    if (mine === seq) loading.value = false
  }
}
watch([range, metric], load, { immediate: true })

const totals = computed(() => ({
  calls: overall.value.reduce((s, o) => s + o.calls, 0),
  errors: overall.value.reduce((s, o) => s + o.errors, 0),
  ms: overall.value.reduce((s, o) => s + o.exec_total_ms, 0),
}))

function metricCell(c: Consumer): string {
  const v = c.metric_value
  if (v === null) return ''
  const unit = metricUnit[metric.value]
  if (unit === 'B') return formatBytes(v)
  if (unit === 'ms') return formatDuration(v)
  return compact(v)
}
</script>

<template>
  <section>
    <div class="toolbar">
      <h1>Overview</h1>
      <RangePicker />
    </div>
    <StoreNotice />
    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <div class="cards" v-if="overall.length">
      <div class="card stat"><h3>Executions</h3><p class="big">{{ compact(totals.calls) }}</p></div>
      <div class="card stat"><h3>Total execution time</h3><p class="big">{{ formatDuration(totals.ms) }}</p></div>
      <div class="card stat"><h3>Errors</h3><p class="big" :class="{ bad: totals.errors }">{{ compact(totals.errors) }}</p></div>
    </div>

    <div class="card">
      <div class="toolbar">
        <h2 class="inline-h">Overall consumption</h2>
        <select v-model="chartMetric" aria-label="Chart metric">
          <option v-for="c in chartChoices" :key="c.id" :value="c.id">{{ c.label }}</option>
        </select>
        <span class="muted">per {{ formatDuration(bucket * 1000) }}</span>
      </div>
      <Chart :series="series" :from="range.from.getTime()" :to="range.to.getTime()" :bucket-ms="bucket * 1000" kind="bars" :unit="chosen.unit" />
    </div>

    <div class="toolbar top-bar">
      <h2 class="inline-h">Top resource consumers</h2>
      <select v-model="metric" aria-label="Metric">
        <option v-for="(label, id) in metricLabels" :key="id" :value="id">{{ label }}</option>
      </select>
      <span v-if="loading" class="muted">Loading…</span>
    </div>
    <div class="grid-wrap" v-if="top.length">
      <table class="grid">
        <thead>
          <tr>
            <th>Statement</th>
            <th class="r">{{ metricLabels[metric] }}</th>
            <th class="r">Executions</th>
            <th class="r">Total</th>
            <th class="r">Average (ms)</th>
            <th class="r">Std dev</th>
            <th class="r">p95 (ms)</th>
            <th class="r">Rows</th>
            <th class="r">Reads</th>
            <th class="r">Plans</th>
            <th class="r">Errors</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="c in top" :key="c.dbid + ':' + c.queryid" tabindex="0" @click="router.push(queryPath(c.dbid, c.queryid))" @keydown.enter="router.push(queryPath(c.dbid, c.queryid))">
            <td class="stmt" :title="c.query ?? ''">{{ oneLine(c.query, 140) || '(text not available)' }}</td>
            <td class="r"><b>{{ metricCell(c) }}</b></td>
            <td class="r">{{ compact(c.calls) }}</td>
            <td class="r">{{ formatDuration(c.total_exec_ms) }}</td>
            <td class="r">{{ formatMs(c.avg_exec_ms) }}</td>
            <td class="r">{{ formatMs(c.stddev_exec_ms) }}</td>
            <td class="r">{{ formatMs(c.p95_exec_ms) }}</td>
            <td class="r">{{ compact(c.rows) }}</td>
            <td class="r">{{ compact(c.shared_blks_read) }}</td>
            <td class="r" :class="{ warn: c.plans > 1 }">{{ c.plans }}</td>
            <td class="r" :class="{ bad: c.errors }">{{ c.errors || '' }}</td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-else-if="!loading" class="muted pad">No statements were recorded in this period.</p>
  </section>
</template>
